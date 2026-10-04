#![forbid(unsafe_code)]

//! Applying changes that approval policies cover.

use config_grpc_client::{ConfigClientConfig, ConfigPublicationClient};
use gateway_grpc::GatewayGrpcService;
use panel_application::{CommandContext, IdempotencyKey, RequestDeadline, RequestId, RequestScope};
use panel_config_api::{
    ApplyOutcome, ApplyRequest, ApprovalBypass, ApprovalChange, ApprovalQuery, ConfigurationChange,
    ConfigurationCommand, ConfigurationPort, ModelChange,
};
use panel_control_runtime::{ProcessSettings, RunningProcess, NATS_URL_ENV};
use panel_engine::{EngineCapability, FakeGatewayEngine};
use panel_errors::ErrorCode;
use panel_health::HealthStatus;
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_service::Environment;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    ffi::OsString,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;
use uuid::Uuid;

struct Harness {
    client: ConfigPublicationClient,
    _data: tempfile::TempDir,
    _process: RunningProcess,
    _broker: TestBroker,
}

async fn start() -> Option<Harness> {
    let broker = TestBroker::create().await?;
    let data = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway = listener.local_addr().unwrap();
    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    let engine = FakeGatewayEngine::new(
        [
            "action.respond",
            "activation.cas",
            "listener.http",
            "listener.http2",
        ]
        .into_iter()
        .map(|name| EngineCapability::new(name, "1")),
    );
    let service = GatewayGrpcService::new(Arc::new(engine));
    tokio::spawn(
        Server::builder()
            .add_service(health)
            .add_service(service.transport_policy().gateway_server(service))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    let values: HashMap<&str, OsString> = HashMap::from([
        (
            NATS_URL_ENV,
            std::env::var(TEST_NATS_URL_ENV).unwrap().into(),
        ),
        (
            config_service::GATEWAY_URL_ENV,
            format!("http://{gateway}").into(),
        ),
    ]);
    let mut env = Environment::from_lookup(move |name| values.get(name).cloned());
    let settings = ProcessSettings::read(&mut env, config_service::default_addresses())
        .unwrap()
        .with_listeners(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .with_health_interval(Duration::from_millis(50))
        .with_data_directory(data.path());
    let process = config_service::process(&mut env, settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();
    let mut health = process.health();
    tokio::time::timeout(Duration::from_secs(20), async {
        while health.current().status() != HealthStatus::Pass {
            assert!(health.changed().await);
        }
    })
    .await
    .expect("config-service becomes ready");
    let client = ConfigPublicationClient::connect_lazy(
        format!("http://{}", process.grpc_address().unwrap()),
        ConfigClientConfig::default(),
    )
    .unwrap();
    Some(Harness {
        client,
        _data: data,
        _process: process,
        _broker: broker,
    })
}

static REQUESTS: AtomicU32 = AtomicU32::new(0);

/// A command by `actor` with a key of its own.
fn by(actor: &str) -> CommandContext {
    let n = REQUESTS.fetch_add(1, Ordering::Relaxed);
    CommandContext::new(
        RequestId::new(format!("request-{n}")).unwrap(),
        RequestId::new(format!("flow-{n}")).unwrap(),
        actor,
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new(format!("key-{n}")).unwrap(),
    )
    .unwrap()
}

/// A model input, written as JSON.
fn input<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

fn id(value: &Value) -> Uuid {
    value.as_str().unwrap().parse().unwrap()
}

fn json(content: &[u8]) -> Value {
    serde_json::from_slice(content).unwrap()
}

impl Harness {
    async fn change(&self, actor: &str, command: impl Into<ConfigurationCommand>) -> Value {
        let command = command.into();
        let operation = command.operation();
        json(
            &self
                .client
                .change(by(actor), ConfigurationChange::new(command))
                .await
                .unwrap_or_else(|error| panic!("{operation}: {error:?}"))
                .content,
        )
    }

    async fn refused(&self, actor: &str, command: impl Into<ConfigurationCommand>) -> String {
        self.client
            .change(by(actor), ConfigurationChange::new(command))
            .await
            .unwrap_err()
            .code
            .as_str()
            .to_owned()
    }

    async fn apply(&self, actor: &str, request: ApplyRequest) -> ApplyOutcome {
        self.client.apply(by(actor), request).await.unwrap()
    }

    async fn awaiting(&self, actor: &str) -> Value {
        match self.apply(actor, ApplyRequest::new(0)).await {
            ApplyOutcome::AwaitingApproval { request, .. } => {
                serde_json::to_value(request).unwrap()
            }
            other => panic!("expected to wait for approval, got {other:?}"),
        }
    }

    async fn get(&self, id: Uuid) -> Value {
        json(
            &self
                .client
                .read(
                    RequestScope::new(RequestId::new("read").unwrap()),
                    ApprovalQuery::Request { id }.into(),
                )
                .await
                .unwrap()
                .content,
        )
    }

    async fn events(&self, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE event_type LIKE '%.' || ?1 || '.v1'")
            .bind(event_type)
            .fetch_one(self._process.database().pool())
            .await
            .unwrap()
    }
}

fn applied(outcome: &ApplyOutcome) -> bool {
    matches!(outcome, ApplyOutcome::Applied { .. })
}

#[tokio::test]
async fn covered_changes_wait_for_someone_else_to_approve_them() {
    let Some(harness) = start().await else { return };
    let put = harness
        .change(
            "admin",
            ApprovalChange::PutPolicy {
                id: "production".into(),
                policy: input(
                    json!({"description": "Production needs a second person", "site_tags": ["production"]}),
                ),
            },
        )
        .await;
    assert_eq!(put["created"], true);
    assert_eq!(put["policy"]["version"], 1);

    harness
        .change(
            "alice",
            ModelChange::PutListener {
                listener: input(json!({"id": "http", "address": "0.0.0.0:80"})),
            },
        )
        .await;
    harness
        .change(
            "alice",
            ModelChange::CreateSite {
                site: input(
                    json!({"name": "intranet", "action": {"type": "respond"}, "domains": [{"host": "intranet.example"}]}),
                ),
            },
        )
        .await;
    let outcome = harness.apply("alice", ApplyRequest::new(0)).await;
    assert!(
        applied(&outcome),
        "changes no policy covers apply at once: {outcome:?}"
    );

    let shop = harness
        .change(
            "alice",
            ModelChange::CreateSite {
                site: input(
                    json!({"name": "shop", "action": {"type": "respond"}, "tags": ["production"], "domains": [{"host": "shop.example"}]}),
                ),
            },
        )
        .await;
    let shop = id(&shop["id"]);
    let request = harness.awaiting("alice").await;
    let request_id = id(&request["id"]);
    assert_eq!(request["state"], "pending");
    assert_eq!(request["requested_by"], "alice");
    assert_eq!(
        request["policies"],
        json!([{"id": "production", "version": 1}])
    );
    assert_eq!(
        request["changes"],
        json!([{"resource": format!("sites/{shop}"), "change": "added"}])
    );
    assert_eq!(
        harness.awaiting("alice").await["id"],
        request["id"],
        "the same content waits on one request"
    );

    let approve = || ApprovalChange::Approve { id: request_id };
    assert_eq!(
        harness.refused("alice", approve()).await,
        ErrorCode::PERMISSION_DENIED,
        "nobody approves their own request"
    );
    let approved = harness.change("bob", approve()).await;
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["approvals"][0]["approver"], "bob");
    assert_eq!(harness.refused("bob", approve()).await, ErrorCode::CONFLICT);
    let outcome = harness.apply("alice", ApplyRequest::new(0)).await;
    let ApplyOutcome::Applied { revision, .. } = outcome else {
        panic!("approved content applies, got {outcome:?}");
    };
    let done = harness.get(request_id).await;
    assert_eq!(
        (done["state"].clone(), done["revision"].clone()),
        (json!("applied"), json!(revision))
    );

    // Editing the policy outdates approvals given under it.
    harness
        .change("alice", ModelChange::DisableSite { id: shop })
        .await;
    let second_id = id(&harness.awaiting("alice").await["id"]);
    harness
        .change("bob", ApprovalChange::Approve { id: second_id })
        .await;
    let edited = harness
        .change(
            "admin",
            ApprovalChange::PutPolicy {
                id: "production".into(),
                policy: input(json!({"site_tags": ["production"], "valid_minutes": 30})),
            },
        )
        .await;
    assert_eq!(
        (
            edited["created"].clone(),
            edited["policy"]["version"].clone()
        ),
        (json!(false), json!(2))
    );
    let third_id = id(&harness.awaiting("alice").await["id"]);
    assert_ne!(third_id, second_id);
    assert_eq!(harness.get(second_id).await["state"], "outdated");

    // Approvals can be revoked, and only the requester withdraws.
    harness
        .change("bob", ApprovalChange::Approve { id: third_id })
        .await;
    let revoked = harness
        .change("bob", ApprovalChange::Revoke { id: third_id })
        .await;
    assert_eq!(revoked["state"], "pending");
    assert!(revoked["approvals"][0]["revoked_at"].is_string());
    assert_eq!(
        harness
            .refused("carol", ApprovalChange::Withdraw { id: third_id })
            .await,
        ErrorCode::PERMISSION_DENIED
    );
    let withdrawn = harness
        .change("alice", ApprovalChange::Withdraw { id: third_id })
        .await;
    assert_eq!(withdrawn["state"], "withdrawn");

    let fourth_id = id(&harness.awaiting("alice").await["id"]);
    let rejected = harness
        .change(
            "bob",
            ApprovalChange::Reject {
                id: fourth_id,
                reason: Some("not during the sale".into()),
            },
        )
        .await;
    assert_eq!(
        (rejected["state"].clone(), rejected["reason"].clone()),
        (json!("rejected"), json!("not during the sale"))
    );

    // An emergency bypass needs a reason and is recorded.
    let short = harness
        .client
        .apply(
            by("admin"),
            ApplyRequest::new(0).bypassing(ApprovalBypass::new("down", "INC-42")),
        )
        .await
        .unwrap_err();
    assert_eq!(short.code.as_str(), ErrorCode::INVALID_ARGUMENT);
    let bypassed = harness
        .apply(
            "admin",
            ApplyRequest::new(0).bypassing(ApprovalBypass::new(
                "checkout is down for every customer",
                "INC-42",
            )),
        )
        .await;
    assert!(applied(&bypassed));
    assert_eq!(harness.events("config.approval.bypassed").await, 1);
    for event in [
        "config.approval_policy.created",
        "config.approval.requested",
        "config.approval.approved",
        "config.approval.applied",
        "config.approval.revoked",
        "config.approval.withdrawn",
        "config.approval.rejected",
        "config.approval.outdated",
    ] {
        assert!(harness.events(event).await >= 1, "{event}");
    }

    let list = json(
        &harness
            .client
            .read(
                RequestScope::new(RequestId::new("list").unwrap()),
                ApprovalQuery::Requests {
                    before: None,
                    limit: Some(2),
                }
                .into(),
            )
            .await
            .unwrap()
            .content,
    );
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    assert!(list["next_before"].is_string(), "{list}");
}
