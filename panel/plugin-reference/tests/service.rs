#![forbid(unsafe_code)]

//! The plugins module against the reference plugin, a real process: its
//! publisher trusted, versions found, granted, configured with secrets,
//! enabled, upgraded, rolled back and limited; refused changes leave
//! nothing changed, and every change and refusal is recorded.

use base64::Engine;
use gateway_grpc_client::GatewayGrpcClient;
use panel_application::GatewayPort;
use panel_application::{
    CommandContext, ContainerAction, ContainerFilter, ContainersPort, IdempotencyKey,
    RequestDeadline, RequestId, RequestScope,
};
use panel_errors::{ErrorCode, PanelError};
use panel_platform::ServiceName;
use panel_plugin_api::{
    NewTrustedKey, PluginChange, PluginCommand, PluginLimits, PluginList, PluginQuery, PluginState,
    PluginView, PluginsPort, Secret,
};
use panel_secrets::{EnvelopeVault, SecretVault};
use panel_sqlite::{testing::TestDatabase, EventLog, ServiceDatabase};
use plugin_contracts::v1::{secret_provider_client::SecretProviderClient, ResolveRequest};
use plugin_contracts::PLUGIN_METADATA;
use plugin_host::{
    proxy::{Containers, GatewayEngine, PortProxy, Secrets, PLUGIN_HEADER},
    runtime::Runtime,
};
use plugin_reference::package::{self, Publisher};
use plugins_grpc_client::ContainerEngines;
use plugins_service::{Paths, PluginService, Store, MIGRATIONS};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

const EXECUTABLE: &str = env!("CARGO_BIN_EXE_pingora-panel-reference-plugin");

static REQUESTS: AtomicU64 = AtomicU64::new(0);

fn context() -> CommandContext {
    let n = REQUESTS.fetch_add(1, Ordering::Relaxed);
    CommandContext::new(
        RequestId::new(format!("request-{n}")).unwrap(),
        RequestId::new(format!("flow-{n}")).unwrap(),
        "admin",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new(format!("key-{n}")).unwrap(),
    )
    .unwrap()
}

fn scope() -> RequestScope {
    RequestScope::new(RequestId::new("read").unwrap())
}

fn vault() -> Arc<dyn SecretVault> {
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    Arc::new(EnvelopeVault::from_keys(&key).unwrap())
}

struct Module {
    root: PathBuf,
    _root: tempfile::TempDir,
    _database: TestDatabase,
    database: ServiceDatabase,
    plugins: PluginService,
    publisher: Publisher,
}

impl Module {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let database = TestDatabase::migrated(MIGRATIONS).await;
        let service = database.database().clone();
        let plugins = Self::plugins(root.path(), &service);
        plugins.start().await;
        Self {
            root: root.path().to_owned(),
            _root: root,
            _database: database,
            database: service,
            plugins,
            publisher: Publisher::from_seed([9; 32], [9; 8]),
        }
    }

    fn plugins(root: &Path, database: &ServiceDatabase) -> PluginService {
        let (runtime, changes) = Runtime::new();
        let plugins = PluginService::new(
            Store::new(database),
            EventLog::new(database, ServiceName::new("plugins-service").unwrap()),
            runtime,
            Paths {
                packages: root.join("plugins"),
                data: root.join("data"),
                sockets: root.join("run"),
            },
            Some(vault()),
        );
        tokio::spawn({
            let plugins = plugins.clone();
            async move { plugins.record_changes(changes).await }
        });
        plugins
    }

    /// The module as it starts again on the same database and directory.
    async fn restart(mut self) -> Self {
        self.plugins.runtime().stop_all().await;
        self.plugins = Self::plugins(&self.root, &self.database);
        self.plugins.start().await;
        self
    }

    fn install(&self, name: &str, version: &str) {
        let mut manifest = package::manifest(version);
        manifest.name = name.into();
        package::install(
            &self.root.join("plugins"),
            Path::new(EXECUTABLE),
            &manifest,
            &self.publisher,
        )
        .unwrap();
    }

    async fn change(&self, command: PluginCommand) -> Result<Value, PanelError> {
        self.change_if(command, None).await
    }

    async fn change_if(
        &self,
        command: PluginCommand,
        if_match: Option<&str>,
    ) -> Result<Value, PanelError> {
        let mut change = PluginChange::new(command);
        change.if_match = if_match.map(str::to_owned);
        let output = self.plugins.change(context(), change).await?;
        Ok(if output.content.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&output.content).unwrap()
        })
    }

    async fn plugin(&self, name: &str) -> PluginView {
        let output = self
            .plugins
            .read(scope(), PluginQuery::Plugin { name: name.into() })
            .await
            .unwrap();
        serde_json::from_slice(&output.content).unwrap()
    }

    async fn list(&self) -> PluginList {
        let output = self
            .plugins
            .read(scope(), PluginQuery::Plugins)
            .await
            .unwrap();
        serde_json::from_slice(&output.content).unwrap()
    }

    async fn events(&self) -> Vec<String> {
        let types: Vec<String> =
            sqlx::query_scalar("SELECT event_type FROM outbox ORDER BY position")
                .fetch_all(self.database.pool())
                .await
                .unwrap();
        types
            .into_iter()
            .map(|kind| {
                kind.trim_start_matches("io.github.eltavine.pingora-panel.plugins.")
                    .trim_end_matches(".v1")
                    .to_owned()
            })
            .collect()
    }

    async fn resolve(&self, plugin: &str, path: &str) -> Result<Vec<u8>, tonic::Status> {
        let mut client = SecretProviderClient::new(PortProxy::<Secrets>::new(Arc::clone(
            self.plugins.runtime(),
        )));
        let mut request = tonic::Request::new(ResolveRequest { path: path.into() });
        request
            .metadata_mut()
            .insert(PLUGIN_HEADER, plugin.parse().unwrap());
        Ok(client.resolve(request).await?.into_inner().value)
    }

    fn trust(&self) -> PluginCommand {
        PluginCommand::PutKey {
            key: NewTrustedKey {
                id: "reference".into(),
                public_key: self.publisher.public_key(),
                comment: "the reference publisher".into(),
            },
        }
    }
}

fn code(result: Result<Value, PanelError>) -> &'static str {
    match result {
        Ok(value) => panic!("the change was made: {value}"),
        Err(error) => {
            let code = error.code.as_str();
            [
                ErrorCode::NOT_FOUND,
                ErrorCode::CONFLICT,
                ErrorCode::VALIDATION_FAILED,
                ErrorCode::PRECONDITION_FAILED,
                ErrorCode::PERMISSION_DENIED,
                ErrorCode::ACTIVATE_FAILED,
                ErrorCode::INVALID_ARGUMENT,
            ]
            .into_iter()
            .find(|known| *known == code)
            .unwrap_or_else(|| panic!("unexpected error {error:?}"))
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plugin_is_trusted_granted_configured_enabled_upgraded_and_rolled_back() {
    let module = Module::new().await;
    module.install("reference", "1.0.0");
    module.change(PluginCommand::Discover).await.unwrap();

    let untrusted = module.plugin("reference").await;
    assert_eq!(untrusted.state, PluginState::Disabled);
    assert!(!untrusted.versions[0].problems.is_empty());
    let enable = || PluginCommand::Enable {
        name: "reference".into(),
        version: None,
    };
    assert_eq!(code(module.change(enable()).await), ErrorCode::NOT_FOUND);
    assert_eq!(
        code(
            module
                .change(PluginCommand::Enable {
                    name: "reference".into(),
                    version: Some("1.0.0".into()),
                })
                .await
        ),
        ErrorCode::VALIDATION_FAILED
    );
    assert_eq!(module.plugin("reference").await.etag, "\"0\"");

    module.change(module.trust()).await.unwrap();
    let trusted = module.plugin("reference").await;
    assert!(trusted.versions[0].problems.is_empty());
    assert_eq!(trusted.versions[0].signed_by.as_deref(), Some("reference"));
    assert!(trusted.versions[0].compatible);
    assert!(trusted.versions[0].config_schema.is_some());

    let enabled = module.change(enable()).await.unwrap();
    assert_eq!(enabled["state"], "enabled");
    assert_eq!(enabled["active_version"], "1.0.0");
    assert_eq!(enabled["health"]["status"], "serving");
    assert_eq!(enabled["grants"], json!([]));

    // Settings that name a secret need the grant to receive it, and the
    // refused change leaves the plugin as it was.
    let configure = |settings: Value| PluginCommand::Configure {
        name: "reference".into(),
        settings,
    };
    module
        .change(PluginCommand::PutSecret {
            name: "dns-token".into(),
            value: Secret::new("s3cret"),
        })
        .await
        .unwrap();
    assert_eq!(
        code(
            module
                .change(configure(json!({"token": "vault:dns-token"})))
                .await
        ),
        ErrorCode::PERMISSION_DENIED
    );
    assert_eq!(module.plugin("reference").await.settings, json!({}));
    assert_eq!(
        code(
            module
                .change(PluginCommand::Grant {
                    name: "reference".into(),
                    capabilities: vec!["telepathy".into()],
                })
                .await
        ),
        ErrorCode::INVALID_ARGUMENT
    );
    module
        .change(PluginCommand::Grant {
            name: "reference".into(),
            capabilities: vec!["secrets".into(), "secret-references".into()],
        })
        .await
        .unwrap();
    assert_eq!(
        code(module.change(configure(json!({"delay_ms": "slow"}))).await),
        ErrorCode::VALIDATION_FAILED
    );
    assert_eq!(
        code(
            module
                .change(configure(json!({"token": "vault:missing"})))
                .await
        ),
        ErrorCode::NOT_FOUND
    );
    let configured = module
        .change(configure(json!({"token": "vault:dns-token"})))
        .await
        .unwrap();
    assert_eq!(configured["settings"], json!({"token": "vault:dns-token"}));
    assert_eq!(
        module.resolve("reference", "token").await.unwrap(),
        b"s3cret"
    );
    assert_eq!(
        code(
            module
                .change(PluginCommand::DeleteSecret {
                    name: "dns-token".into()
                })
                .await
        ),
        ErrorCode::CONFLICT
    );

    assert_eq!(
        code(module.change_if(configure(json!({})), Some("\"1\"")).await),
        ErrorCode::PRECONDITION_FAILED
    );

    assert_eq!(
        code(
            module
                .change(PluginCommand::Limit {
                    name: "reference".into(),
                    limits: PluginLimits {
                        concurrency: 5000,
                        ..PluginLimits::default()
                    },
                })
                .await
        ),
        ErrorCode::VALIDATION_FAILED
    );
    let limited = module
        .change(PluginCommand::Limit {
            name: "reference".into(),
            limits: PluginLimits {
                concurrency: 4,
                call_timeout_ms: 2000,
                ..PluginLimits::default()
            },
        })
        .await
        .unwrap();
    assert_eq!(limited["effective_limits"]["concurrency"], 4);
    assert_eq!(limited["effective_limits"]["call_timeout_ms"], 2000);
    assert_eq!(limited["effective_limits"]["open_files"], 256);

    module.install("reference", "1.1.0");
    module.change(PluginCommand::Discover).await.unwrap();
    let upgraded = module
        .change(PluginCommand::Upgrade {
            name: "reference".into(),
            version: "1.1.0".into(),
        })
        .await
        .unwrap();
    assert_eq!(upgraded["active_version"], "1.1.0");
    assert_eq!(upgraded["previous_version"], "1.0.0");
    assert_eq!(upgraded["health"]["version"], "1.1.0");
    assert_eq!(
        module.resolve("reference", "token").await.unwrap(),
        b"s3cret"
    );
    let rolled_back = module
        .change(PluginCommand::Rollback {
            name: "reference".into(),
        })
        .await
        .unwrap();
    assert_eq!(rolled_back["active_version"], "1.0.0");
    assert_eq!(rolled_back["previous_version"], "1.1.0");
    assert_eq!(rolled_back["health"]["version"], "1.0.0");

    assert_eq!(
        code(
            module
                .change(PluginCommand::DeleteKey {
                    id: "reference".into()
                })
                .await
        ),
        ErrorCode::CONFLICT
    );
    let disabled = module
        .change(PluginCommand::Disable {
            name: "reference".into(),
        })
        .await
        .unwrap();
    assert_eq!(disabled["state"], "disabled");
    assert!(disabled.get("health").is_none());
    assert!(module.plugins.runtime().get("reference").is_none());
    module
        .change(PluginCommand::DeleteKey {
            id: "reference".into(),
        })
        .await
        .unwrap();
    assert!(!module.plugin("reference").await.versions[0]
        .problems
        .is_empty());

    let events = module.events().await;
    for expected in [
        "catalog.discovered",
        "change.refused",
        "key.trusted",
        "plugin.enabled",
        "secret.sealed",
        "plugin.granted",
        "plugin.configured",
        "plugin.limited",
        "plugin.upgraded",
        "plugin.rolled_back",
        "plugin.disabled",
        "key.removed",
    ] {
        assert!(
            events.iter().any(|kind| kind == expected),
            "{expected} in {events:?}"
        );
    }
    let refusals = events
        .iter()
        .filter(|kind| *kind == "change.refused")
        .count();
    assert_eq!(refusals, 10, "{events:?}");
    module.plugins.runtime().stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_the_plugin_refuses_keeps_what_ran() {
    let module = Module::new().await;
    module.install("reference", "1.0.0");
    module.change(module.trust()).await.unwrap();
    let enabled = module
        .change(PluginCommand::Enable {
            name: "reference".into(),
            version: None,
        })
        .await
        .unwrap();
    let started_at = enabled["health"]["started_at"].clone();

    let refused = module
        .change(PluginCommand::Configure {
            name: "reference".into(),
            settings: json!({"refuse": true}),
        })
        .await;
    assert_eq!(code(refused), ErrorCode::ACTIVATE_FAILED);
    let kept = module.plugin("reference").await;
    assert_eq!(kept.state, PluginState::Enabled);
    assert_eq!(kept.settings, json!({}));
    assert_eq!(kept.etag, enabled["etag"].as_str().unwrap());
    assert_eq!(
        serde_json::to_value(kept.health.unwrap().started_at).unwrap(),
        started_at
    );
    let events = module.events().await;
    assert_eq!(events.last().map(String::as_str), Some("change.refused"));
    module.plugins.runtime().stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabled_plugins_start_with_the_module_secret_providers_first() {
    let module = Module::new().await;
    module.install("keeper", "1.0.0");
    module.install("reference", "1.0.0");
    module.change(module.trust()).await.unwrap();
    for (name, capabilities) in [
        ("keeper", vec!["secrets"]),
        ("reference", vec!["secrets", "secret-references"]),
    ] {
        module
            .change(PluginCommand::Grant {
                name: name.into(),
                capabilities: capabilities.into_iter().map(str::to_owned).collect(),
            })
            .await
            .unwrap();
    }
    module
        .change(PluginCommand::Configure {
            name: "keeper".into(),
            settings: json!({"secrets": {"api": "from-keeper"}}),
        })
        .await
        .unwrap();
    module
        .change(PluginCommand::Enable {
            name: "keeper".into(),
            version: None,
        })
        .await
        .unwrap();
    module
        .change(PluginCommand::Configure {
            name: "reference".into(),
            settings: json!({"token": "keeper:api"}),
        })
        .await
        .unwrap();
    module
        .change(PluginCommand::Enable {
            name: "reference".into(),
            version: None,
        })
        .await
        .unwrap();
    assert_eq!(
        module.resolve("reference", "token").await.unwrap(),
        b"from-keeper"
    );
    assert_eq!(
        code(
            module
                .change(PluginCommand::Configure {
                    name: "keeper".into(),
                    settings: json!({"token": "keeper:api"}),
                })
                .await
        ),
        ErrorCode::PERMISSION_DENIED
    );

    let module = module.restart().await;
    let list = module.list().await;
    assert_eq!(list.protocol_versions, vec![1]);
    assert!(list.discovered_at.is_some());
    let states: Vec<(String, PluginState)> = list
        .plugins
        .iter()
        .map(|plugin| (plugin.name.clone(), plugin.state))
        .collect();
    assert_eq!(
        states,
        vec![
            ("keeper".to_owned(), PluginState::Enabled),
            ("reference".to_owned(), PluginState::Enabled),
        ]
    );
    assert_eq!(
        module.resolve("reference", "token").await.unwrap(),
        b"from-keeper"
    );
    module.plugins.runtime().stop_all().await;
}

/// The plugins module's container engine port, as the API reaches it.
fn engines_channel(module: &Module) -> tonic::transport::Channel {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = PortProxy::<Containers>::new(Arc::clone(module.plugins.runtime()));
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        tonic::transport::Server::builder()
            .add_service(proxy)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
    });
    tonic::transport::Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect_lazy()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engines_plugins_provide_run_their_containers() {
    let module = Module::new().await;
    module.install("reference", "1.0.0");
    module.change(module.trust()).await.unwrap();
    module
        .change(PluginCommand::Configure {
            name: "reference".into(),
            settings: json!({"containers": [
                {"name": "web", "image": "nginx:1.29"},
                {"name": "jobs", "image": "busybox:1.37", "running": false}
            ]}),
        })
        .await
        .unwrap();
    module
        .change(PluginCommand::Enable {
            name: "reference".into(),
            version: None,
        })
        .await
        .unwrap();
    let engines = ContainerEngines::new(
        None,
        Arc::new(module.plugins.clone()),
        engines_channel(&module),
    );
    assert!(
        engines.engines(scope()).await.unwrap().is_empty(),
        "an engine needs its grant"
    );
    module
        .change(PluginCommand::Grant {
            name: "reference".into(),
            capabilities: vec!["containers".into()],
        })
        .await
        .unwrap();

    let listed = engines.engines(scope()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "reference.main");
    assert!(listed[0].reachable);
    assert_eq!(listed[0].info.as_ref().unwrap().running, 1);
    let containers = engines
        .containers(scope(), "reference.main".into(), ContainerFilter::default())
        .await
        .unwrap();
    let names: Vec<&[String]> = containers
        .containers
        .iter()
        .map(|container| container.names.as_slice())
        .collect();
    assert_eq!(names, [["web".to_owned()], ["jobs".to_owned()]]);
    let started = engines
        .act(
            context(),
            "reference.main".into(),
            "jobs".into(),
            ContainerAction::Start,
        )
        .await
        .unwrap();
    assert_eq!(started.name, "jobs");
    assert_eq!(
        engines.engines(scope()).await.unwrap()[0]
            .info
            .as_ref()
            .unwrap()
            .running,
        2
    );
    let missing = engines
        .containers(scope(), "docker".into(), ContainerFilter::default())
        .await
        .unwrap_err();
    assert_eq!(missing.code.as_str(), ErrorCode::NOT_FOUND);
    module.plugins.runtime().stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gateway_engine_a_plugin_provides_answers_the_gateway_client() {
    let module = Module::new().await;
    module.install("reference", "1.0.0");
    module.change(module.trust()).await.unwrap();
    module
        .change(PluginCommand::Grant {
            name: "reference".into(),
            capabilities: vec!["gateway".into()],
        })
        .await
        .unwrap();
    module
        .change(PluginCommand::Enable {
            name: "reference".into(),
            version: None,
        })
        .await
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let engine = PortProxy::<GatewayEngine>::new(Arc::clone(module.plugins.runtime()));
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        tonic::transport::Server::builder()
            .add_service(engine)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
    });
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect_lazy();
    let gateway = GatewayGrpcClient::from_channel(channel.clone())
        .with_metadata(PLUGIN_METADATA, "reference")
        .unwrap();
    let status = GatewayPort::status(&gateway).await.unwrap();
    assert!(status.active_hash().is_none(), "{status:?}");
    let unnamed = GatewayPort::status(&GatewayGrpcClient::from_channel(channel))
        .await
        .unwrap_err();
    assert!(
        unnamed.message.contains(PLUGIN_METADATA),
        "{}",
        unnamed.message
    );
    module
        .change(PluginCommand::Grant {
            name: "reference".into(),
            capabilities: vec![],
        })
        .await
        .unwrap();
    let refused = GatewayPort::status(&gateway).await.unwrap_err();
    assert!(refused.message.contains("gateway"), "{}", refused.message);
    module.plugins.runtime().stop_all().await;
}
