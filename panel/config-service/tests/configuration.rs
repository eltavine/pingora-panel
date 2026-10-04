#![forbid(unsafe_code)]

//! Edits the draft through the configuration API and applies it to a gateway.

use config_grpc_client::{ConfigClientConfig, ConfigPublicationClient};
use gateway_grpc::GatewayGrpcService;
use panel_application::{CommandContext, IdempotencyKey, RequestDeadline, RequestId, RequestScope};
use panel_config_api::{
    ApplyOutcome, ApplyRequest, ConfigurationChange, ConfigurationCommand, ConfigurationPort,
    DiffBase, Files, LanguageChange, LanguageQuery, ModelChange, ModelQuery, RevisionChange,
    RevisionQuery,
};
use panel_config_model::SiteQuery;
use panel_control_runtime::{ProcessSettings, RunningProcess, NATS_URL_ENV};
use panel_engine::{EngineCapability, FakeGatewayEngine};
use panel_errors::ErrorCode;
use panel_health::HealthStatus;
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_service::Environment;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{collections::HashMap, ffi::OsString, net::SocketAddr, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;
use uuid::Uuid;

const CAPABILITIES: &[&str] = &[
    "action.redirect",
    "action.respond",
    "action.static",
    "activation.cas",
    "listener.http",
    "listener.http2",
    "listener.https",
    "route.exact-path",
    "route.glob",
    "route.host",
    "route.path-prefix",
    "route.regex",
    "site.redirect",
    "upstream.backup",
    "upstream.balancing",
    "upstream.health-check",
    "upstream.http",
    "upstream.http2",
    "upstream.https",
    "upstream.passive-health",
];

async fn gateway() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    let engine = FakeGatewayEngine::new(
        CAPABILITIES
            .iter()
            .map(|name| EngineCapability::new(*name, "1")),
    );
    let gateway = GatewayGrpcService::new(Arc::new(engine));
    tokio::spawn(
        Server::builder()
            .add_service(health)
            .add_service(gateway.transport_policy().gateway_server(gateway))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    address
}

fn command(key: &str) -> CommandContext {
    CommandContext::new(
        RequestId::new(format!("request-{key}")).unwrap(),
        RequestId::new("flow-1").unwrap(),
        "operator",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new(key).unwrap(),
    )
    .unwrap()
}

fn change(command: impl Into<ConfigurationCommand>) -> ConfigurationChange {
    ConfigurationChange::new(command)
}

/// A model input, written as JSON.
fn input<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

fn id(value: &Value) -> Uuid {
    value.as_str().unwrap().parse().unwrap()
}

/// Files with only the entry file.
fn main(text: impl Into<String>) -> Files {
    Files::from([("main.conf".to_owned(), text.into())])
}

fn json(content: &[u8]) -> Value {
    serde_json::from_slice(content).unwrap()
}

/// A running config-service with its database, broker and gateway.
struct Harness {
    client: ConfigPublicationClient,
    process: RunningProcess,
    _broker: TestBroker,
    _data: tempfile::TempDir,
}

async fn start() -> Option<Harness> {
    let broker = TestBroker::create().await?;
    let data = tempfile::tempdir().unwrap();
    let gateway = gateway().await;
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
        process,
        _broker: broker,
        _data: data,
    })
}

fn scope() -> RequestScope {
    RequestScope::new(RequestId::new("read").unwrap())
}

#[tokio::test]
async fn draft_changes_are_idempotent_conditional_and_applied() {
    let Some(harness) = start().await else { return };
    let client = &harness.client;

    let summary = client
        .read(scope(), ModelQuery::SiteSummary.into())
        .await
        .unwrap();
    assert_eq!(json(&summary.content)["total"], 0);
    assert_eq!(summary.draft.version, 0);
    assert_eq!(
        client
            .apply(command("apply-empty"), ApplyRequest::new(0))
            .await
            .unwrap_err()
            .code
            .as_str(),
        ErrorCode::PRECONDITION_FAILED
    );

    let upstream = client
        .change(
            command("upstream"),
            change(ModelChange::CreateUpstream {
                upstream: input(
                    json!({"name": "app", "nodes": [{"host": "127.0.0.1", "port": 8080}]}),
                ),
            }),
        )
        .await
        .unwrap();
    let upstream_id = json(&upstream.content)["id"].as_str().unwrap().to_owned();
    let listener = change(ModelChange::PutListener {
        listener: input(json!({"id": "http", "address": "0.0.0.0:80"})),
    });
    client.change(command("listener"), listener).await.unwrap();
    let create = change(ModelChange::CreateSite {
        site: input(json!({
            "name": "shop",
            "action": {"type": "proxy", "upstream_id": upstream_id},
            "domains": [{"host": "shop.example.com"}]
        })),
    });
    let site = client
        .change(command("site"), create.clone())
        .await
        .unwrap();
    assert_eq!(site.draft.version, 3);
    let replayed = client.change(command("site"), create).await.unwrap();
    assert_eq!(replayed.content, site.content);
    assert_eq!(replayed.draft.version, 3);
    let reused = client
        .change(
            command("site"),
            change(ModelChange::CreateSite {
                site: input(json!({"name": "other", "action": {"type": "respond"}})),
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(reused.code.as_str(), ErrorCode::CONFLICT);

    let site = json(&site.content);
    let site_id = id(&site["id"]);
    let mut stale = change(ModelChange::DisableSite { id: site_id });
    stale.if_match = Some("\"stale\"".into());
    assert_eq!(
        client
            .change(command("stale"), stale)
            .await
            .unwrap_err()
            .code
            .as_str(),
        ErrorCode::PRECONDITION_FAILED
    );
    let duplicate = client
        .change(
            command("duplicate"),
            change(ModelChange::CreateSite { site: input(json!({"name": "copy", "action": {"type": "respond"}, "domains": [{"host": "SHOP.example.com"}]})) }),
        )
        .await
        .unwrap_err();
    assert_eq!(duplicate.code.as_str(), ErrorCode::VALIDATION_FAILED);
    assert!(duplicate.diagnostics[0].message.contains("already bound"));

    match client
        .apply(command("apply-1"), ApplyRequest::new(3))
        .await
        .unwrap()
    {
        ApplyOutcome::Applied {
            draft,
            deployment,
            revision,
            ..
        } => {
            assert_eq!(draft.applied_version, Some(3));
            assert!(!draft.pending());
            assert_eq!(deployment.revision_id().get(), 3);
            assert_eq!(revision, 1);
        }
        other => panic!("expected an applied draft, got {other:?}"),
    }
    assert_eq!(
        client
            .apply(command("apply-2"), ApplyRequest::new(2))
            .await
            .unwrap_err()
            .code
            .as_str(),
        ErrorCode::CONFLICT
    );
    let listed = client
        .read(
            scope(),
            ModelQuery::Sites {
                query: input(json!({"q": "shop"})),
            }
            .into(),
        )
        .await
        .unwrap();
    assert_eq!(json(&listed.content)["items"][0]["status"], "running");
    assert_eq!(listed.draft.applied_version, Some(3));
}

const SHOP: &str = "\
language_version 1;

http {
    listener http {
        address 127.0.0.1:18080;
    }

    # The storefront's servers
    upstream app {
        server 10.0.0.11:8080;
    }

    server shop {
        server_name shop.example;
        proxy app;
    }
}
";

#[tokio::test]
async fn the_draft_is_text_and_every_apply_is_a_revision() {
    let Some(harness) = start().await else { return };
    let client = &harness.client;

    let source = client
        .read(scope(), LanguageQuery::Source.into())
        .await
        .unwrap();
    assert_eq!(
        json(&source.content)["files"]["main.conf"],
        "language_version 1;\n\nhttp {\n}\n"
    );
    assert_eq!(source.etag.as_deref(), Some("\"draft-0\""));
    assert_eq!(json(&source.content)["etag"], "\"draft-0\"");

    let invalid = client
        .read(
            scope(),
            LanguageQuery::Check {
                files: main("language_version 1;\nhttp {\n    server s { proxy nowhere; }\n}\n"),
            }
            .into(),
        )
        .await
        .unwrap();
    let invalid = json(&invalid.content);
    assert_eq!(invalid["valid"], false);
    assert!(
        invalid["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["message"] == "no upstream is named \"nowhere\""
                && item["source_span"] == "main.conf:3.22-28"),
        "{invalid}"
    );

    let formatted = client
        .read(
            scope(),
            LanguageQuery::Format {
                files: main("language_version  1 ;"),
            }
            .into(),
        )
        .await
        .unwrap();
    assert_eq!(
        json(&formatted.content)["files"]["main.conf"],
        "language_version 1;\n"
    );
    let schema = client
        .read(scope(), LanguageQuery::Schema.into())
        .await
        .unwrap();
    assert!(
        json(&schema.content)["directives"]
            .as_array()
            .unwrap()
            .len()
            > 40
    );

    let mut stale = change(LanguageChange::ReplaceSource { files: main(SHOP) });
    stale.if_match = Some("\"draft-7\"".into());
    let error = client
        .change(command("text-stale"), stale)
        .await
        .unwrap_err();
    assert_eq!(error.code.as_str(), ErrorCode::PRECONDITION_FAILED);
    let mut saved = change(LanguageChange::ReplaceSource { files: main(SHOP) });
    saved.if_match = Some("\"draft-0\"".into());
    let saved = client.change(command("text"), saved).await.unwrap();
    assert_eq!(saved.etag.as_deref(), Some("\"draft-1\""));
    assert_eq!(json(&saved.content)["version"], 1);
    let text = json(&saved.content)["files"]["main.conf"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.contains("    # The storefront's servers\n    upstream app {\n        id "),
        "{text}"
    );

    let tree = client
        .read(
            scope(),
            LanguageQuery::Syntax {
                files: main(SHOP),
                file: None,
            }
            .into(),
        )
        .await
        .unwrap();
    let tree = json(&tree.content);
    assert_eq!(tree["directives"][1]["name"], "http");
    assert_eq!(
        tree["directives"][1]["block"][1]["comments"][0],
        "The storefront's servers"
    );
    let missing = client
        .read(
            scope(),
            LanguageQuery::Syntax {
                files: main(SHOP),
                file: Some("sites/none.conf".into()),
            }
            .into(),
        )
        .await
        .unwrap_err();
    assert_eq!(missing.code.as_str(), ErrorCode::NOT_FOUND);

    let explained = client
        .read(
            scope(),
            LanguageQuery::Explain {
                files: main(SHOP),
                file: "main.conf".into(),
                line: 15,
                column: 9,
            }
            .into(),
        )
        .await
        .unwrap();
    let explained = json(&explained.content);
    assert_eq!(explained["block"], "server");
    assert_eq!(explained["name"], "shop");
    let listen = explained["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|setting| setting["name"] == "listen")
        .unwrap();
    assert_eq!(listen["value"], "http");
    assert_eq!(listen["source"], "default");
    assert!(listen["rule"].as_str().unwrap().contains("every listener"));
    let nowhere = client
        .read(
            scope(),
            LanguageQuery::Explain {
                files: main(SHOP),
                file: "main.conf".into(),
                line: 1,
                column: 1,
            }
            .into(),
        )
        .await
        .unwrap_err();
    assert_eq!(nowhere.code.as_str(), ErrorCode::NOT_FOUND);

    let ir = client
        .read(scope(), LanguageQuery::Ir.into())
        .await
        .unwrap();
    let ir = json(&ir.content);
    assert_eq!(ir["listeners"][0]["address"], "127.0.0.1:18080");
    assert_eq!(ir["sites"].as_array().unwrap().len(), 1);

    let upstreams = client
        .read(scope(), ModelQuery::Upstreams.into())
        .await
        .unwrap();
    let upstream = json(&upstreams.content)[0].clone();
    let mut edited = upstream.clone();
    edited["note"] = json!("primary pool");
    for field in ["id", "etag", "used_by", "created_at", "updated_at"] {
        edited.as_object_mut().unwrap().remove(field);
    }
    let mut replace = change(ModelChange::ReplaceUpstream {
        id: id(&upstream["id"]),
        upstream: input(edited),
    });
    replace.if_match = Some(upstream["etag"].as_str().unwrap().to_owned());
    client.change(command("note"), replace).await.unwrap();
    let source = client
        .read(scope(), LanguageQuery::Source.into())
        .await
        .unwrap();
    let text = json(&source.content)["files"]["main.conf"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.contains("    # The storefront's servers\n")
            && text.contains("note \"primary pool\";"),
        "{text}"
    );

    let plan = client
        .read(scope(), LanguageQuery::Plan.into())
        .await
        .unwrap();
    let resources = json(&plan.content)["resources"].as_array().unwrap().len();
    assert_eq!(resources, 3);

    match client
        .apply(command("dry"), ApplyRequest::new(2).dry_run())
        .await
        .unwrap()
    {
        ApplyOutcome::Checked { draft, .. } => assert_eq!(draft.applied_version, None),
        other => panic!("expected a passed dry run, got {other:?}"),
    }
    let first = match client
        .apply(command("first"), ApplyRequest::new(2).with_note("launch"))
        .await
        .unwrap()
    {
        ApplyOutcome::Applied { revision, .. } => revision,
        other => panic!("expected an applied draft, got {other:?}"),
    };

    let mut replaced = change(LanguageChange::ReplaceSource {
        files: main(text.replace("shop.example", "store.example")),
    });
    replaced.if_match = Some("\"draft-2\"".into());
    client.change(command("rename"), replaced).await.unwrap();
    let second = match client
        .apply(command("second"), ApplyRequest::new(3))
        .await
        .unwrap()
    {
        ApplyOutcome::Applied { revision, .. } => revision,
        other => panic!("expected an applied draft, got {other:?}"),
    };
    assert_eq!(second, first + 1);

    let listed = client
        .read(
            scope(),
            RevisionQuery::Revisions {
                before: None,
                limit: None,
            }
            .into(),
        )
        .await
        .unwrap();
    let items = json(&listed.content)["items"].clone();
    assert_eq!(items[0]["id"], second);
    assert_eq!(items[0]["outcome"], "active");
    assert_eq!(items[1]["outcome"], "superseded");
    assert_eq!(items[1]["note"], "launch");
    assert_eq!(items[1]["author"], "operator");

    let diff = client
        .read(
            scope(),
            RevisionQuery::Diff {
                id: second,
                against: DiffBase::Previous,
            }
            .into(),
        )
        .await
        .unwrap();
    let diff = json(&diff.content);
    assert_eq!(diff["resources"].as_array().unwrap().len(), 1);
    assert!(diff["files"][0]["diff"]
        .as_str()
        .unwrap()
        .contains("+        server_name store.example;"));

    client
        .change(
            command("restore"),
            change(RevisionChange::Restore { id: first }),
        )
        .await
        .unwrap();
    let restored = client
        .read(scope(), LanguageQuery::Source.into())
        .await
        .unwrap();
    let detail = client
        .read(scope(), RevisionQuery::Revision { id: first }.into())
        .await
        .unwrap();
    assert_eq!(
        json(&restored.content)["files"],
        json(&detail.content)["files"]
    );
    let plan = client
        .read(scope(), LanguageQuery::Plan.into())
        .await
        .unwrap();
    assert_eq!(
        json(&plan.content)["resources"].as_array().unwrap().len(),
        1
    );

    let noted = client
        .change(
            command("note-1"),
            change(RevisionChange::Note {
                id: first,
                note: Some("first launch".into()),
            }),
        )
        .await
        .unwrap();
    assert_eq!(json(&noted.content)["note"], "first launch");

    let reserved = text.replace(
        "address 127.0.0.1:18080;",
        "address 127.0.0.1:18080;\n        protocols http1 http3;",
    );
    let mut saved = change(LanguageChange::ReplaceSource {
        files: main(reserved),
    });
    saved.if_match = Some("*".into());
    client.change(command("http3"), saved).await.unwrap();
    let error = client
        .apply(command("refused"), ApplyRequest::new(0))
        .await
        .unwrap_err();
    assert_eq!(error.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);
    let listed = client
        .read(
            scope(),
            RevisionQuery::Revisions {
                before: None,
                limit: Some(1),
            }
            .into(),
        )
        .await
        .unwrap();
    let latest = json(&listed.content);
    assert_eq!(latest["items"][0]["outcome"], "failed");
    assert_eq!(latest["next_before"], latest["items"][0]["id"]);
    let active = client
        .read(
            scope(),
            RevisionQuery::Revisions {
                before: None,
                limit: None,
            }
            .into(),
        )
        .await
        .unwrap();
    assert_eq!(json(&active.content)["items"][1]["outcome"], "active");

    let recorded: Vec<String> =
        sqlx::query_scalar("SELECT event_type FROM outbox ORDER BY position")
            .fetch_all(harness.process.database().pool())
            .await
            .unwrap();
    for expected in [
        "config.change.refused",
        "config.draft.changed",
        "config.apply.checked",
        "config.draft.applied",
        "config.revision.noted",
        "config.apply.failed",
        "gateway.snapshot.prepared",
        "gateway.snapshot.aborted",
        "gateway.snapshot.activated",
        "gateway.snapshot.refused",
    ] {
        let qualified = format!("io.github.eltavine.pingora-panel.{expected}.v1");
        assert!(recorded.contains(&qualified), "{expected}: {recorded:?}");
    }
}

/// A caller who may read, change and apply only the shop's sites.
fn shop_keeper() -> panel_application::SiteScope {
    let access = |permission: &str| panel_application::SiteAccess {
        permission: permission.into(),
        groups: vec!["shop".into()],
        sites: Vec::new(),
    };
    panel_application::SiteScope {
        unrestricted: Vec::new(),
        limited: vec![
            access("config.read"),
            access("config.write"),
            access("config.apply"),
        ],
    }
}

#[tokio::test]
async fn callers_limited_to_a_site_group_act_only_on_its_sites() {
    let Some(harness) = start().await else { return };
    let client = &harness.client;
    let scoped = |key: &str| command(key).with_site_scope(Some(shop_keeper()));
    let scoped_read = || scope().with_site_scope(Some(shop_keeper()));
    client
        .change(
            command("listener"),
            change(ModelChange::PutListener {
                listener: input(json!({"id": "http", "address": "0.0.0.0:80"})),
            }),
        )
        .await
        .unwrap();
    let site = |name: &str, group: &str| {
        change(ModelChange::CreateSite {
            site: input(json!({
                "name": name,
                "group": group,
                "action": {"type": "respond"},
                "domains": [{"host": format!("{name}.example.com")}]
            })),
        })
    };
    let shop = json(
        &client
            .change(command("shop"), site("shop", "shop"))
            .await
            .unwrap()
            .content,
    );
    let corp = json(
        &client
            .change(command("corp"), site("intranet", "corp"))
            .await
            .unwrap()
            .content,
    );
    let shop = id(&shop["id"]);
    let corp = id(&corp["id"]);
    let ApplyOutcome::Applied { .. } = client
        .apply(command("apply-1"), ApplyRequest::new(0))
        .await
        .unwrap()
    else {
        panic!("the unscoped operator applies");
    };

    let listed = json(
        &client
            .read(
                scoped_read(),
                ModelQuery::Sites {
                    query: SiteQuery::default(),
                }
                .into(),
            )
            .await
            .unwrap()
            .content,
    );
    let names: Vec<&str> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|site| site["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["shop"]);
    let hidden = client
        .read(scoped_read(), ModelQuery::Site { id: corp }.into())
        .await
        .unwrap_err();
    assert_eq!(hidden.code.as_str(), ErrorCode::NOT_FOUND);
    let whole = client
        .read(scoped_read(), LanguageQuery::Source.into())
        .await
        .unwrap_err();
    assert_eq!(whole.code.as_str(), ErrorCode::PERMISSION_DENIED);

    let refused = |error: panel_errors::PanelError| error.code.as_str().to_owned();
    assert_eq!(
        refused(
            client
                .change(
                    scoped("theirs"),
                    change(ModelChange::DisableSite { id: corp })
                )
                .await
                .unwrap_err()
        ),
        ErrorCode::PERMISSION_DENIED
    );
    assert_eq!(
        refused(
            client
                .change(
                    scoped("shared"),
                    change(ModelChange::PutListener {
                        listener: input(json!({"id": "https", "address": "0.0.0.0:443"}))
                    }),
                )
                .await
                .unwrap_err()
        ),
        ErrorCode::PERMISSION_DENIED
    );
    let moved = client
        .change(
            scoped("move"),
            change(ModelChange::ReplaceSite {
                id: shop,
                site: input(json!({
                    "name": "shop",
                    "group": "corp",
                    "action": {"type": "respond"},
                    "domains": [{"host": "shop.example.com"}]
                })),
            }),
        )
        .await;
    assert!(moved.is_err(), "a site cannot be moved out of reach");
    client
        .change(
            scoped("mine"),
            change(ModelChange::DisableSite { id: shop }),
        )
        .await
        .unwrap();
    let ApplyOutcome::Applied { .. } = client
        .apply(scoped("apply-2"), ApplyRequest::new(0))
        .await
        .unwrap()
    else {
        panic!("the shop keeper applies the shop's change");
    };

    client
        .change(
            command("theirs-2"),
            change(ModelChange::DisableSite { id: corp }),
        )
        .await
        .unwrap();
    let mixed = client
        .apply(scoped("apply-3"), ApplyRequest::new(0))
        .await
        .unwrap_err();
    assert_eq!(
        mixed.code.as_str(),
        ErrorCode::PERMISSION_DENIED,
        "the draft holds someone else's change"
    );
}
