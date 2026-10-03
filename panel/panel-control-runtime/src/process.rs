use crate::{
    tasks::{self, SchemaCheck},
    ProcessSettings,
};
use async_nats::jetstream::Context;
use axum::response::IntoResponse;
use chrono::Utc;
use panel_contracts::{platform::v1::service_info_server, PLATFORM_V1};
use panel_errors::{PanelError, Result};
use panel_health::{
    HealthCheck, HealthMonitor, HealthRegistry, HealthWatch, Impact, ServiceIdentity,
};
use panel_jetstream::{
    JetStreamHealthCheck, JetStreamPublisher, JetStreamServiceRegistry, JetStreamSettings,
};
use panel_outbox::RelayOptions;
use panel_pki::{CredentialFiles, WorkloadIdentity};
use panel_platform::{
    Capability, ProtocolRange, RegistrationPolicy, ServiceDescriptor, ServiceName,
};
use panel_platform_codec::protocol_range;
use panel_postgres::{
    PgHealthCheck, PgOutbox, SchemaMigration, ServiceDatabase, ServiceDatabaseConfig, SqlIdentifier,
};
use panel_service::{ops_router, publish_grpc_health, ServiceInfoService};
use panel_tls::{PeerPolicy, TlsCredentials};
use std::{convert::Infallible, future::Future, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::watch, task::JoinHandle};
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tonic::{
    body::Body,
    codegen::{http::Request, Service},
    server::NamedService,
    service::Routes,
    transport::Server,
};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CREDENTIAL_RELOAD_INTERVAL: Duration = Duration::from_secs(30);
const PEER_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PEER_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

type StartHook = Box<dyn FnOnce(&RunningProcess) -> Result<()> + Send>;

/// A control-plane service process before it starts.
pub struct ControlPlaneProcess {
    descriptor: ServiceDescriptor,
    settings: ProcessSettings,
    database: ServiceDatabase,
    migrations: &'static [SchemaMigration],
    database_impact: Impact,
    broker_impact: Impact,
    checks: Vec<(Arc<dyn HealthCheck>, Impact)>,
    routes: Routes,
    grpc_services: Vec<&'static str>,
    jetstream: JetStreamSettings,
    registration_ttl: Duration,
    registration: RegistrationPolicy,
    relay: RelayOptions,
    start_hooks: Vec<StartHook>,
    tls: Option<Arc<TlsCredentials>>,
    peer_policy: PeerPolicy,
}

impl ControlPlaneProcess {
    /// A process for `service` built as `release` that owns `schema`.
    ///
    /// By default a failing database makes the service unavailable, while a
    /// failing broker is only reported: events wait in the outbox.
    pub fn new(
        service: ServiceName,
        release: impl Into<String>,
        settings: ProcessSettings,
        schema: SqlIdentifier,
    ) -> Result<Self> {
        let mut database_config =
            ServiceDatabaseConfig::new(settings.database_url(), service.as_str(), schema)?;
        if let Some(secret) = settings.database_password() {
            database_config = database_config.with_secret(secret);
        }
        let trust_domain = settings
            .tls()
            .map(|tls| tls.trust_domain.clone())
            .unwrap_or_default();
        let tls = settings
            .tls()
            .map(|tls| {
                TlsCredentials::load(
                    CredentialFiles::new(&tls.directory),
                    WorkloadIdentity::new(service.clone(), tls.trust_domain.clone()),
                )
            })
            .transpose()?;
        Ok(Self {
            descriptor: ServiceDescriptor::new(service, release, Utc::now())
                .with_protocol(protocol_range(PLATFORM_V1)),
            database: ServiceDatabase::connect_lazy(database_config),
            settings,
            migrations: &[],
            database_impact: Impact::Required,
            broker_impact: Impact::Informational,
            checks: Vec::new(),
            routes: Routes::default(),
            grpc_services: Vec::new(),
            jetstream: JetStreamSettings::default(),
            registration_ttl: JetStreamServiceRegistry::DEFAULT_TTL,
            registration: RegistrationPolicy::default(),
            relay: RelayOptions::default(),
            start_hooks: Vec::new(),
            tls,
            peer_policy: PeerPolicy::new(trust_domain),
        })
    }

    /// The mutual TLS credentials, when the process serves and calls peers
    /// over mutual TLS.
    pub fn tls(&self) -> Option<&Arc<TlsCredentials>> {
        self.tls.as_ref()
    }

    /// A mutual TLS channel to `peer` at `url` when the process has
    /// credentials; `None` when peers are reached over plaintext loopback.
    pub fn peer_channel(
        &self,
        url: &str,
        peer: ServiceName,
    ) -> Result<Option<tonic::transport::Channel>> {
        let Some(credentials) = &self.tls else {
            return Ok(None);
        };
        let identity = WorkloadIdentity::new(peer, credentials.identity().trust_domain().clone());
        panel_tls::channel(
            &panel_tls::address_of(url)?,
            &identity,
            Arc::clone(credentials),
            PEER_CONNECT_TIMEOUT,
            PEER_REQUEST_TIMEOUT,
        )
        .map(Some)
    }

    /// Lets `peers` call `grpc_service` once mutual TLS is enabled.
    pub fn with_peer_access(
        mut self,
        grpc_service: &str,
        peers: impl IntoIterator<Item = ServiceName>,
    ) -> Self {
        self.peer_policy = self.peer_policy.allow(grpc_service, peers);
        self
    }

    /// Runs once the process has started, to serve service-specific
    /// listeners or tasks through [`RunningProcess::spawn`]. A failing hook
    /// stops the process and fails the start.
    pub fn on_start(
        mut self,
        hook: impl FnOnce(&RunningProcess) -> Result<()> + Send + 'static,
    ) -> Self {
        self.start_hooks.push(Box::new(hook));
        self
    }

    /// The service database. It connects on first use, so adapters can be
    /// built on it before the process starts.
    pub fn database(&self) -> &ServiceDatabase {
        &self.database
    }

    /// Service migrations, at versions from
    /// [`SchemaMigration::SERVICE_VERSION_FLOOR`].
    pub fn with_migrations(mut self, migrations: &'static [SchemaMigration]) -> Self {
        self.migrations = migrations;
        self
    }

    pub fn with_protocol(mut self, range: ProtocolRange) -> Self {
        self.descriptor = self.descriptor.with_protocol(range);
        self
    }

    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.descriptor = self.descriptor.with_capability(capability);
        self
    }

    pub fn with_database_impact(mut self, impact: Impact) -> Self {
        self.database_impact = impact;
        self
    }

    pub fn with_broker_impact(mut self, impact: Impact) -> Self {
        self.broker_impact = impact;
        self
    }

    /// An additional dependency, such as a peer service.
    pub fn with_check(mut self, check: Arc<dyn HealthCheck>, impact: Impact) -> Self {
        self.checks.push((check, impact));
        self
    }

    /// A gRPC service served next to health and service description; its
    /// health status follows the process readiness.
    pub fn with_grpc_service<S>(mut self, service: S) -> Self
    where
        S: Service<Request<Body>, Error = Infallible>
            + NamedService
            + Clone
            + Send
            + Sync
            + 'static,
        S::Response: IntoResponse,
        S::Future: Send + 'static,
    {
        self.routes = self.routes.add_service(service);
        self.grpc_services.push(S::NAME);
        self
    }

    pub fn with_jetstream_settings(mut self, settings: JetStreamSettings) -> Self {
        self.jetstream = settings;
        self
    }

    /// Registrations expire `ttl` after the last refresh made under `policy`.
    pub fn with_registration(mut self, ttl: Duration, policy: RegistrationPolicy) -> Self {
        self.registration_ttl = ttl;
        self.registration = policy;
        self
    }

    pub fn with_relay_options(mut self, options: RelayOptions) -> Self {
        self.relay = options;
        self
    }

    /// Binds the listeners, failing if either is taken, and starts the
    /// process. Dependencies are reached in the background.
    pub async fn start(self) -> Result<RunningProcess> {
        let service = self.descriptor.service().clone();
        let ops_listener = bind(self.settings.ops_address(), "operational").await?;
        let grpc_listener = bind(self.settings.grpc_address(), "gRPC").await?;
        let ops_address = local_address(&ops_listener)?;
        let grpc_address = local_address(&grpc_listener)?;
        let descriptor = self
            .descriptor
            .with_schema_version(SchemaMigration::latest(self.migrations).to_string());

        let database = self.database;
        let client = async_nats::ConnectOptions::new()
            .name(service.as_str())
            .retry_on_initial_connect()
            .connect(self.settings.nats_url())
            .await
            .map_err(|error| {
                PanelError::invalid_argument(format!("invalid broker address: {error}"))
            })?;
        let context = async_nats::jetstream::new(client);
        let jetstream = Arc::new(self.jetstream);

        let (migrated, migration_state) = watch::channel(false);
        let registry = self.checks.into_iter().fold(
            HealthRegistry::new(ServiceIdentity::new(
                service.as_str(),
                descriptor.build_version(),
            ))
            .register(
                Arc::new(SchemaCheck(migration_state.clone())),
                Impact::Required,
            )
            .register(
                Arc::new(PgHealthCheck::new(database.pool().clone())),
                self.database_impact,
            )
            .register(
                Arc::new(JetStreamHealthCheck::new(context.clone())),
                self.broker_impact,
            ),
            |registry, (check, impact)| registry.register(check, impact),
        );
        let (health, monitor) =
            HealthMonitor::new(Arc::new(registry), self.settings.health_interval()).spawn();

        let cancel = CancellationToken::new();
        let tasks = TaskTracker::new();
        tasks.spawn(tasks::migrate(
            database.clone(),
            self.migrations,
            migrated,
            cancel.clone(),
        ));
        tasks.spawn(tasks::register(
            context.clone(),
            Arc::clone(&jetstream),
            descriptor.clone(),
            self.registration_ttl,
            self.registration,
            cancel.clone(),
        ));
        tasks.spawn(tasks::relay(
            PgOutbox::new(&database),
            JetStreamPublisher::new(context.clone(), Arc::clone(&jetstream)),
            self.relay,
            migration_state.clone(),
            cancel.clone(),
        ));

        let ops = axum::serve(ops_listener, ops_router(health.clone()))
            .with_graceful_shutdown(cancel.clone().cancelled_owned());
        tasks.spawn(async move {
            if let Err(error) = ops.await {
                tracing::error!(%error, "operational listener failed");
            }
        });

        let (reporter, health_service) = tonic_health::server::health_reporter();
        let mut names = vec![String::new(), service_info_server::SERVICE_NAME.to_owned()];
        names.extend(self.grpc_services.iter().map(|name| (*name).to_owned()));
        let publication = publish_grpc_health(health.clone(), reporter, names);
        let publication_cancel = cancel.clone();
        tasks.spawn(async move {
            tokio::select! {
                () = publication_cancel.cancelled() => {}
                () = publication => {}
            }
        });
        let routes = self
            .routes
            .add_service(health_service)
            .add_service(ServiceInfoService::new(&descriptor).into_server());
        match &self.tls {
            Some(credentials) => {
                let grpc = Server::builder()
                    .layer(self.peer_policy)
                    .add_routes(routes)
                    .serve_with_incoming_shutdown(
                        panel_tls::incoming(
                            grpc_listener,
                            Arc::clone(credentials),
                            HANDSHAKE_TIMEOUT,
                        ),
                        cancel.clone().cancelled_owned(),
                    );
                tasks.spawn(async move {
                    if let Err(error) = grpc.await {
                        tracing::error!(%error, "gRPC listener failed");
                    }
                });
                tasks.spawn(
                    Arc::clone(credentials).watch(CREDENTIAL_RELOAD_INTERVAL, cancel.clone()),
                );
            }
            None => {
                let grpc = Server::builder()
                    .add_routes(routes)
                    .serve_with_incoming_shutdown(
                        TcpListenerStream::new(grpc_listener),
                        cancel.clone().cancelled_owned(),
                    );
                tasks.spawn(async move {
                    if let Err(error) = grpc.await {
                        tracing::error!(%error, "gRPC listener failed");
                    }
                });
            }
        }
        tracing::info!(
            service = %service,
            instance_id = %descriptor.instance_id(),
            %ops_address,
            %grpc_address,
            "service started"
        );

        let running = RunningProcess {
            descriptor,
            health,
            migrated: migration_state,
            database,
            context,
            jetstream,
            ops_address,
            grpc_address,
            cancel,
            tasks,
            monitor,
        };
        for hook in self.start_hooks {
            if let Err(error) = hook(&running) {
                running.stop().await;
                return Err(error);
            }
        }
        Ok(running)
    }
}

/// A started control-plane process.
pub struct RunningProcess {
    descriptor: ServiceDescriptor,
    health: HealthWatch,
    migrated: watch::Receiver<bool>,
    database: ServiceDatabase,
    context: Context,
    jetstream: Arc<JetStreamSettings>,
    ops_address: SocketAddr,
    grpc_address: SocketAddr,
    cancel: CancellationToken,
    tasks: TaskTracker,
    monitor: JoinHandle<()>,
}

impl RunningProcess {
    pub fn descriptor(&self) -> &ServiceDescriptor {
        &self.descriptor
    }

    pub fn health(&self) -> HealthWatch {
        self.health.clone()
    }

    pub fn database(&self) -> &ServiceDatabase {
        &self.database
    }

    /// Resolves once the service schema is migrated, or with `false` when
    /// the process stops first; service tasks that use the schema wait for it.
    pub fn migrated(&self) -> impl Future<Output = bool> + Send + 'static {
        let mut state = self.migrated.clone();
        async move { state.wait_for(|migrated| *migrated).await.is_ok() }
    }

    pub fn jetstream(&self) -> &Context {
        &self.context
    }

    pub fn jetstream_settings(&self) -> &Arc<JetStreamSettings> {
        &self.jetstream
    }

    pub fn ops_address(&self) -> SocketAddr {
        self.ops_address
    }

    pub fn grpc_address(&self) -> SocketAddr {
        self.grpc_address
    }

    /// A token cancelled when the process stops, for service-specific tasks.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.cancel.child_token()
    }

    /// Spawns a service-specific task that `stop` waits for.
    pub fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.tasks.spawn(task);
    }

    pub async fn run_until(self, shutdown: impl Future<Output = ()>) {
        shutdown.await;
        self.stop().await;
    }

    /// Stops listeners and background work, deregisters the instance and
    /// closes the database pool.
    pub async fn stop(self) {
        tracing::info!(service = %self.descriptor.service(), "service stopping");
        self.cancel.cancel();
        self.tasks.close();
        if tokio::time::timeout(SHUTDOWN_TIMEOUT, self.tasks.wait())
            .await
            .is_err()
        {
            tracing::warn!("background work did not stop in time");
        }
        self.monitor.abort();
        if tokio::time::timeout(SHUTDOWN_TIMEOUT, self.database.close())
            .await
            .is_err()
        {
            tracing::warn!("database connections did not close in time");
        }
    }
}

async fn bind(address: SocketAddr, purpose: &str) -> Result<TcpListener> {
    TcpListener::bind(address).await.map_err(|error| {
        PanelError::precondition_failed(format!(
            "cannot bind the {purpose} listener on {address}: {error}"
        ))
    })
}

fn local_address(listener: &TcpListener) -> Result<SocketAddr> {
    listener
        .local_addr()
        .map_err(|error| PanelError::internal(format!("listener has no address: {error}")))
}
