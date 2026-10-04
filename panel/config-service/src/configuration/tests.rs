//! The configuration use cases on stores kept in memory, without a database
//! or a transport.

use super::ConfigurationService;
use crate::memory::{IdleGateway, MemoryApprovals, MemoryDrafts, MemoryEvents, MemoryRevisions};
use panel_application::{CommandContext, IdempotencyKey, RequestDeadline, RequestId, RequestScope};
use panel_config_api::{
    ApplyOutcome, ApplyRequest, ApprovalChange, ConfigurationChange, ConfigurationPort,
    ModelChange, ModelQuery, RevisionQuery,
};
use panel_config_model::{ApprovalRequest, SiteQuery};
use panel_errors::ErrorCode;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::Arc;

struct Harness {
    service: ConfigurationService,
    events: Arc<MemoryEvents>,
}

fn harness() -> Harness {
    let events = Arc::new(MemoryEvents::default());
    let service = ConfigurationService::new(
        Arc::new(MemoryDrafts::default()),
        Arc::new(MemoryRevisions::default()),
        Arc::new(MemoryApprovals::default()),
        Arc::new(IdleGateway),
        events.clone(),
    );
    Harness { service, events }
}

fn by(actor: &str, key: &str) -> CommandContext {
    CommandContext::new(
        RequestId::new(format!("request-{key}")).unwrap(),
        RequestId::new("flow").unwrap(),
        actor,
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new(key).unwrap(),
    )
    .unwrap()
}

fn reading() -> RequestScope {
    RequestScope::new(RequestId::new("read").unwrap())
}

/// A model input, written as JSON.
fn input<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

fn json(content: &[u8]) -> Value {
    serde_json::from_slice(content).unwrap()
}

fn site(name: &str) -> ConfigurationChange {
    ConfigurationChange::new(ModelChange::CreateSite {
        site: input(json!({
            "name": name,
            "action": {"type": "respond"},
            "domains": [{"host": format!("{name}.example")}]
        })),
    })
}

#[tokio::test]
async fn changes_replay_by_key_and_respect_entity_tags() {
    let Harness { service, events } = harness();
    let created = service
        .change(by("alice", "site"), site("shop"))
        .await
        .unwrap();
    assert_eq!(created.draft.version, 1);
    let replayed = service
        .change(by("alice", "site"), site("shop"))
        .await
        .unwrap();
    assert_eq!(
        (replayed.content, replayed.draft.version),
        (created.content.clone(), 1)
    );
    let reused = service
        .change(by("alice", "site"), site("other"))
        .await
        .unwrap_err();
    assert_eq!(reused.code.as_str(), ErrorCode::CONFLICT);

    let id = input(json(&created.content)["id"].clone());
    let stale = service
        .change(
            by("alice", "stale"),
            ConfigurationChange::new(ModelChange::DisableSite { id }).if_match("\"stale\""),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code.as_str(), ErrorCode::PRECONDITION_FAILED);
    assert_eq!(
        events.types(),
        ["config.change.refused", "config.change.refused"]
    );

    let listed = service
        .read(
            reading(),
            ModelQuery::Sites {
                query: SiteQuery::default(),
            }
            .into(),
        )
        .await
        .unwrap();
    assert_eq!(json(&listed.content)["total"], 1);
}

#[tokio::test]
async fn covered_changes_wait_for_someone_else_to_approve_them() {
    let Harness { service, .. } = harness();
    let policy = ApprovalChange::PutPolicy {
        id: "everything".into(),
        policy: input(json!({})),
    };
    service
        .change(by("admin", "policy"), ConfigurationChange::new(policy))
        .await
        .unwrap();
    let listener = ModelChange::PutListener {
        listener: input(json!({"id": "http", "address": "0.0.0.0:80"})),
    };
    service
        .change(by("alice", "listener"), ConfigurationChange::new(listener))
        .await
        .unwrap();
    service
        .change(by("alice", "site"), site("shop"))
        .await
        .unwrap();

    let waiting = |outcome: ApplyOutcome| -> ApprovalRequest {
        match outcome {
            ApplyOutcome::AwaitingApproval { request, .. } => *request,
            other => panic!("expected to wait for approval, got {other:?}"),
        }
    };
    let apply = |key: &str| service.apply(by("alice", key), ApplyRequest::new(0));
    let first = waiting(apply("apply-1").await.unwrap());
    let again = waiting(apply("apply-2").await.unwrap());
    assert_eq!(first.id, again.id, "the same content waits on one request");

    let approve = || ConfigurationChange::new(ApprovalChange::Approve { id: first.id });
    let own = service
        .change(by("alice", "own"), approve())
        .await
        .unwrap_err();
    assert_eq!(own.code.as_str(), ErrorCode::PERMISSION_DENIED);
    let approved = service
        .change(by("bob", "approve"), approve())
        .await
        .unwrap();
    assert_eq!(json(&approved.content)["state"], "approved");

    let refused = apply("apply-3").await.unwrap_err();
    assert_eq!(
        refused.code.as_str(),
        ErrorCode::UNAVAILABLE,
        "the approved draft reaches the gateway, which takes nothing"
    );
    let revisions = service
        .read(
            reading(),
            RevisionQuery::Revisions {
                before: None,
                limit: None,
            }
            .into(),
        )
        .await
        .unwrap();
    assert_eq!(json(&revisions.content)["items"][0]["outcome"], "failed");
}
