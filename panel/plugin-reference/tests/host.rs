#![forbid(unsafe_code)]

//! The plugin host against the reference plugin, a real process: found,
//! validated, started, called through every port with its grants, deadline
//! and room, replaced, refused and restarted.

use panel_contracts::pingora::panel::{
    common::v1::RequestContext,
    gateway::v1::{gateway_engine_client::GatewayEngineClient, GetCapabilitiesRequest},
    ops::v1::{
        containers_client::ContainersClient, ContainerAction, ContainerState, ContainersActRequest,
        ContainersListRequest,
    },
};
use plugin_contracts::{
    v1::{
        backup_target_client::BackupTargetClient, backup_target_put_request::Part,
        dns01_provider_client::Dns01ProviderClient,
        notification_provider_client::NotificationProviderClient,
        secret_provider_client::SecretProviderClient, AddTxtRequest, Alert, ArchiveInfo,
        BackupTargetDeleteRequest, BackupTargetGetRequest, BackupTargetListRequest,
        BackupTargetPutRequest, NotifyRequest, RemoveTxtRequest, ResolveRequest,
    },
    PORTS, SECRETS_CAPABILITY,
};
use plugin_host::{
    catalog::{self, Found},
    limits::Limits,
    process::Launch,
    proxy::{self, PortProxy, PLUGIN_HEADER},
    runtime::{Change, Runtime, State},
    signature::TrustedKey,
};
use plugin_reference::package::{self, Publisher};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};
use tokio_stream::StreamExt;
use tonic::{Code, Request};

const EXECUTABLE: &str = env!("CARGO_BIN_EXE_pingora-panel-reference-plugin");

struct Host {
    _root: tempfile::TempDir,
    root: std::path::PathBuf,
    publisher: Publisher,
}

impl Host {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        Self {
            root: root.path().to_owned(),
            _root: root,
            publisher: Publisher::from_seed([5; 32], [5; 8]),
        }
    }

    fn keys(&self) -> Vec<TrustedKey> {
        vec![TrustedKey {
            id: "reference".into(),
            public_key: self.publisher.public_key(),
            comment: String::new(),
        }]
    }

    fn install(&self, version: &str) -> Found {
        let plugins = self.root.join("plugins");
        package::install(
            &plugins,
            Path::new(EXECUTABLE),
            &package::manifest(version),
            &self.publisher,
        )
        .unwrap();
        catalog::discover(&plugins, &self.keys())
            .unwrap()
            .into_iter()
            .find(|found| found.version == version)
            .unwrap()
    }

    fn launch(&self, found: &Found, settings: Value, granted: &[&str], limits: Limits) -> Launch {
        let manifest = found.manifest.clone().unwrap();
        Launch {
            limits: Limits::effective(&manifest, &limits),
            executable: found.executable().unwrap(),
            directory: found.directory.clone(),
            data: self.root.join("data"),
            runtime: self.root.join("run"),
            settings: settings.to_string(),
            granted: granted
                .iter()
                .map(|granted| (*granted).to_owned())
                .collect(),
            manifest,
        }
    }
}

fn every_grant() -> Vec<&'static str> {
    let mut granted = PORTS.to_vec();
    granted.push(SECRETS_CAPABILITY);
    granted
}

fn named<T>(message: T, plugin: &str) -> Request<T> {
    let mut request = Request::new(message);
    request
        .metadata_mut()
        .insert(PLUGIN_HEADER, plugin.parse().unwrap());
    request
}

async fn next(changes: &mut tokio::sync::mpsc::UnboundedReceiver<Change>) -> Change {
    tokio::time::timeout(Duration::from_secs(15), changes.recv())
        .await
        .unwrap()
        .unwrap()
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signed_plugin_runs_and_answers_every_port_through_the_host() {
    let host = Host::new();
    let found = host.install("1.0.0");
    assert!(found.is_valid(), "{found:#?}");
    assert_eq!(found.signed_by.as_deref(), Some("reference"));
    let (runtime, _changes) = Runtime::new();
    let settings = json!({
        "token": "s3cret",
        "secrets": {"api": "key"},
        "containers": [{"name": "web", "image": "nginx:1.29"}]
    });
    runtime
        .run(host.launch(&found, settings, &every_grant(), Limits::default()))
        .await
        .unwrap();
    let instance = runtime.get("reference").unwrap();
    assert_eq!(instance.health().state, State::Running);

    let mut dns = Dns01ProviderClient::new(PortProxy::<proxy::Dns01>::new(Arc::clone(&runtime)));
    let record = || AddTxtRequest {
        name: "_acme-challenge.shop.example.".into(),
        value: "token-1".into(),
    };
    dns.add_txt(named(record(), "reference")).await.unwrap();
    let records = host.root.join("data/records.json");
    assert_eq!(
        read(&records)["_acme-challenge.shop.example."],
        json!(["token-1"])
    );
    dns.remove_txt(named(
        RemoveTxtRequest {
            name: "_acme-challenge.shop.example.".into(),
            value: "token-1".into(),
        },
        "reference",
    ))
    .await
    .unwrap();
    assert_eq!(read(&records), json!({}));

    let mut secrets =
        SecretProviderClient::new(PortProxy::<proxy::Secrets>::new(Arc::clone(&runtime)));
    let resolve = |path: &str| named(ResolveRequest { path: path.into() }, "reference");
    assert_eq!(
        secrets
            .resolve(resolve("token"))
            .await
            .unwrap()
            .into_inner()
            .value,
        b"s3cret"
    );
    assert_eq!(
        secrets
            .resolve(resolve("missing"))
            .await
            .unwrap_err()
            .code(),
        Code::NotFound
    );
    #[cfg(target_os = "linux")]
    {
        let limits = secrets
            .resolve(resolve("process-limits"))
            .await
            .unwrap()
            .into_inner()
            .value;
        let limits = String::from_utf8(limits).unwrap();
        let line = |name: &str| {
            limits
                .lines()
                .find(|line| line.starts_with(name))
                .unwrap()
                .split_whitespace()
                .rev()
                .nth(1)
                .unwrap()
                .to_owned()
        };
        assert_eq!(
            line("Max address space"),
            (512u64 << 20).to_string(),
            "{limits}"
        );
        assert_eq!(line("Max open files"), "256", "{limits}");
    }

    NotificationProviderClient::new(PortProxy::<proxy::Notifications>::new(Arc::clone(&runtime)))
        .notify(named(
            NotifyRequest {
                channel: "ops".into(),
                alert: Some(Alert {
                    status: "firing".into(),
                    labels: [("alertname".to_owned(), "Errors".to_owned())].into(),
                    ..Alert::default()
                }),
                external_url: String::new(),
            },
            "reference",
        ))
        .await
        .unwrap();
    let notified = std::fs::read_to_string(host.root.join("data/notifications.jsonl")).unwrap();
    assert!(notified.contains(r#""alertname":"Errors""#), "{notified}");

    let mut backups =
        BackupTargetClient::new(PortProxy::<proxy::Backups>::new(Arc::clone(&runtime)));
    let archive = vec![7u8; 200_000];
    let parts = vec![
        BackupTargetPutRequest {
            part: Some(Part::Archive(ArchiveInfo {
                name: "backup-1.tar.zst".into(),
                size: archive.len() as u64,
                ..ArchiveInfo::default()
            })),
        },
        BackupTargetPutRequest {
            part: Some(Part::Chunk(archive[..100_000].to_vec())),
        },
        BackupTargetPutRequest {
            part: Some(Part::Chunk(archive[100_000..].to_vec())),
        },
    ];
    backups
        .put(named(tokio_stream::iter(parts), "reference"))
        .await
        .unwrap();
    let listed = backups
        .list(named(BackupTargetListRequest {}, "reference"))
        .await
        .unwrap()
        .into_inner()
        .archives;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].size, 200_000);
    let mut fetched = Vec::new();
    let mut chunks = backups
        .get(named(
            BackupTargetGetRequest {
                name: "backup-1.tar.zst".into(),
            },
            "reference",
        ))
        .await
        .unwrap()
        .into_inner();
    while let Some(chunk) = chunks.next().await {
        fetched.extend(chunk.unwrap().chunk);
    }
    assert_eq!(fetched, archive);
    backups
        .delete(named(
            BackupTargetDeleteRequest {
                name: "backup-1.tar.zst".into(),
            },
            "reference",
        ))
        .await
        .unwrap();

    let mut containers =
        ContainersClient::new(PortProxy::<proxy::Containers>::new(Arc::clone(&runtime)));
    let listed = containers
        .list(named(ContainersListRequest::default(), "reference"))
        .await
        .unwrap()
        .into_inner()
        .containers;
    assert_eq!(listed[0].names, ["web"]);
    let stopped = containers
        .act(named(
            ContainersActRequest {
                container: "web".into(),
                action: ContainerAction::Stop as i32,
                ..ContainersActRequest::default()
            },
            "reference",
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        stopped.container.unwrap().state,
        ContainerState::Exited as i32
    );

    let capabilities =
        GatewayEngineClient::new(PortProxy::<proxy::GatewayEngine>::new(Arc::clone(&runtime)))
            .get_capabilities(named(
                GetCapabilitiesRequest {
                    context: Some(RequestContext {
                        request_id: "reference-capabilities".into(),
                        correlation_id: "reference-capabilities".into(),
                        actor: "test".into(),
                        schema_version: panel_contracts::PROTOCOL_VERSION.into(),
                        ..RequestContext::default()
                    }),
                },
                "reference",
            ))
            .await
            .unwrap()
            .into_inner();
    assert!(
        capabilities
            .capabilities
            .iter()
            .any(|capability| capability.name == "listener.http"),
        "{capabilities:?}"
    );

    runtime.stop_all().await;
    assert!(runtime.get("reference").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calls_need_a_grant_and_room_and_end_at_their_deadline() {
    let host = Host::new();
    let found = host.install("1.0.0");
    let (runtime, _changes) = Runtime::new();
    runtime
        .run(host.launch(
            &found,
            json!({"delay_ms": 1_000}),
            &["dns01"],
            Limits {
                call_timeout_ms: 200,
                ..Limits::default()
            },
        ))
        .await
        .unwrap();
    let mut dns = Dns01ProviderClient::new(PortProxy::<proxy::Dns01>::new(Arc::clone(&runtime)));
    let record = || AddTxtRequest {
        name: "_acme-challenge.shop.example.".into(),
        value: "token".into(),
    };
    let late = dns.add_txt(named(record(), "reference")).await.unwrap_err();
    assert_eq!(late.code(), Code::DeadlineExceeded, "{late:?}");
    assert_eq!(
        dns.add_txt(named(record(), "other"))
            .await
            .unwrap_err()
            .code(),
        Code::FailedPrecondition
    );
    assert_eq!(
        dns.add_txt(Request::new(record()))
            .await
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );
    let ungranted =
        SecretProviderClient::new(PortProxy::<proxy::Secrets>::new(Arc::clone(&runtime)))
            .resolve(named(
                ResolveRequest {
                    path: "token".into(),
                },
                "reference",
            ))
            .await
            .unwrap_err();
    assert_eq!(ungranted.code(), Code::PermissionDenied);

    runtime
        .run(host.launch(
            &found,
            json!({"delay_ms": 800}),
            &["dns01"],
            Limits {
                concurrency: 1,
                ..Limits::default()
            },
        ))
        .await
        .unwrap();
    let first = {
        let mut dns = dns.clone();
        tokio::spawn(async move { dns.add_txt(named(record(), "reference")).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let busy = dns.add_txt(named(record(), "reference")).await.unwrap_err();
    assert_eq!(busy.code(), Code::ResourceExhausted, "{busy:?}");
    first.await.unwrap().unwrap();
    runtime.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_fails_to_start_leaves_what_ran_in_place() {
    let host = Host::new();
    let first = host.install("1.0.0");
    let second = host.install("2.0.0");
    let (runtime, _changes) = Runtime::new();

    let refused = runtime
        .run(host.launch(
            &first,
            json!({"refuse": true}),
            &["dns01"],
            Limits::default(),
        ))
        .await
        .unwrap_err();
    assert!(refused.contains("refused its settings"), "{refused}");
    assert!(runtime.get("reference").is_none());

    let mut impostor = host.launch(&first, json!({}), &["dns01"], Limits::default());
    impostor.manifest.version = "9.9.9".into();
    let mismatch = runtime.run(impostor).await.unwrap_err();
    assert!(mismatch.contains("not reference 9.9.9"), "{mismatch}");

    runtime
        .run(host.launch(&first, json!({}), &["dns01"], Limits::default()))
        .await
        .unwrap();
    assert!(runtime
        .run(host.launch(
            &second,
            json!({"refuse": true}),
            &["dns01"],
            Limits::default()
        ))
        .await
        .is_err());
    assert_eq!(runtime.get("reference").unwrap().version(), "1.0.0");

    runtime
        .run(host.launch(&second, json!({}), &["dns01"], Limits::default()))
        .await
        .unwrap();
    assert_eq!(runtime.get("reference").unwrap().version(), "2.0.0");
    Dns01ProviderClient::new(PortProxy::<proxy::Dns01>::new(Arc::clone(&runtime)))
        .add_txt(named(
            AddTxtRequest {
                name: "_acme-challenge.shop.example.".into(),
                value: "upgraded".into(),
            },
            "reference",
        ))
        .await
        .unwrap();
    runtime.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plugin_that_exits_is_degraded_and_started_again() {
    let host = Host::new();
    let found = host.install("1.0.0");
    let (runtime, mut changes) = Runtime::new();
    runtime
        .run(host.launch(
            &found,
            json!({"exit_after_ms": 300}),
            &["dns01"],
            Limits::default(),
        ))
        .await
        .unwrap();
    match next(&mut changes).await {
        Change::Degraded { reason, .. } => assert!(reason.contains("exited"), "{reason}"),
        other => panic!("{other:?}"),
    }
    let refused = Dns01ProviderClient::new(PortProxy::<proxy::Dns01>::new(Arc::clone(&runtime)))
        .add_txt(named(
            AddTxtRequest {
                name: "_acme-challenge.shop.example.".into(),
                value: "while-down".into(),
            },
            "reference",
        ))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::Unavailable);
    assert!(matches!(next(&mut changes).await, Change::Recovered { .. }));
    assert!(runtime.get("reference").unwrap().health().restarts >= 1);
    runtime.stop_all().await;
}
