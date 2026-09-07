use async_trait::async_trait;
use chrono::DateTime;
use panel_api::{router_with_config, ApiConfig, ApiState};
use panel_application::{
    ActivatedDeployment, CommandContext, ConfigCompiler, GatewayPort, GatewayService,
    GatewayStatus as ApplicationGatewayStatus, IdempotencyRepository, IdempotentGatewayUseCases,
    PreparedDeployment,
};
use panel_domain::ContentHash;
use panel_engine::{ActivateRequest, GatewayEngine, PrepareRequest, PrepareToken};
use panel_errors::{Result, ValidationReport};
use panel_ir::RuntimeSnapshot;
use std::{
    future::Future,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpListener;

/// Adapts the engine port to the application-owned gateway port.
///
/// Keeping this adapter in the composition-root crate prevents engine request
/// types from leaking into `panel-application` and leaves room for a future
/// remote gRPC client to replace it without changing HTTP handlers.
pub struct EngineGatewayPort<E: ?Sized> {
    engine: Arc<E>,
}

impl<E: ?Sized> EngineGatewayPort<E> {
    pub fn new(engine: Arc<E>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl<E> GatewayPort for EngineGatewayPort<E>
where
    E: GatewayEngine + ?Sized + 'static,
{
    async fn validate(&self, snapshot: RuntimeSnapshot) -> Result<ValidationReport> {
        self.engine.validate(snapshot).await
    }

    async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<PreparedDeployment> {
        let receipt = self.engine.prepare(PrepareRequest { snapshot }).await?;
        PreparedDeployment::new(
            receipt.revision_id,
            receipt.content_hash,
            receipt.prepare_token.as_str(),
        )
    }

    async fn activate(
        &self,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        let receipt = self
            .engine
            .activate(ActivateRequest {
                prepare_token: PrepareToken::new(prepare_token),
                expected_active_hash,
            })
            .await?;
        Ok(ActivatedDeployment::new(
            receipt.revision_id,
            receipt.content_hash,
            receipt.previous_active_hash,
        ))
    }

    async fn status(&self) -> Result<ApplicationGatewayStatus> {
        let status = self.engine.status().await?;
        Ok(ApplicationGatewayStatus::new(
            status.ready,
            status.message,
            status.active_revision_id,
            status.active_hash,
            status.prepared_count,
            status.adapter_version,
            status.schema_version,
        ))
    }

    async fn prepare_with_context(
        &self,
        context: CommandContext,
        snapshot: RuntimeSnapshot,
    ) -> Result<PreparedDeployment> {
        let budget = deadline_budget(&context)?;
        let receipt =
            tokio::time::timeout(budget, self.engine.prepare(PrepareRequest { snapshot }))
                .await
                .map_err(|_| {
                    panel_errors::PanelError::deadline_exceeded(
                        "request deadline elapsed during prepare",
                    )
                })??;
        PreparedDeployment::new(
            receipt.revision_id,
            receipt.content_hash,
            receipt.prepare_token.as_str(),
        )
    }

    async fn activate_with_context(
        &self,
        context: CommandContext,
        prepare_token: String,
        expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        let budget = deadline_budget(&context)?;
        let receipt = tokio::time::timeout(
            budget,
            self.engine.activate(ActivateRequest {
                prepare_token: PrepareToken::new(prepare_token),
                expected_active_hash,
            }),
        )
        .await
        .map_err(|_| {
            panel_errors::PanelError::deadline_exceeded("request deadline elapsed during activate")
        })??;
        Ok(ActivatedDeployment::new(
            receipt.revision_id,
            receipt.content_hash,
            receipt.previous_active_hash,
        ))
    }
}

fn deadline_budget(context: &CommandContext) -> Result<Duration> {
    let deadline = DateTime::parse_from_rfc3339(context.deadline().as_str())
        .map_err(|error| panel_errors::PanelError::invalid_argument(error.to_string()))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| panel_errors::PanelError::internal(error.to_string()))?;
    let now_parts = (now.as_secs() as i64, now.subsec_nanos());
    let deadline_parts = (deadline.timestamp(), deadline.timestamp_subsec_nanos());
    if deadline_parts <= now_parts {
        return Err(panel_errors::PanelError::deadline_exceeded(
            "request deadline has elapsed",
        ));
    }
    let mut seconds = deadline_parts
        .0
        .checked_sub(now_parts.0)
        .ok_or_else(|| panel_errors::PanelError::invalid_argument("deadline is out of range"))?
        as u64;
    let nanos = if deadline_parts.1 >= now_parts.1 {
        deadline_parts.1 - now_parts.1
    } else {
        seconds = seconds.saturating_sub(1);
        1_000_000_000 + deadline_parts.1 - now_parts.1
    };
    Ok(Duration::new(seconds, nanos))
}

/// Builds the management router from the same engine instance used by gRPC.
///
/// This is deliberately a library function: the process decides whether and
/// where to bind it, while tests can exercise the exact production graph with
/// an in-memory router.
pub fn management_router<E>(
    engine: Arc<E>,
    compiler: Arc<dyn ConfigCompiler>,
    idempotency: Arc<dyn IdempotencyRepository>,
) -> axum::Router
where
    E: GatewayEngine + ?Sized + 'static,
{
    management_router_with_config(engine, compiler, idempotency, ApiConfig::default())
}

/// Variant that keeps HTTP resource policy injectable at the composition root.
pub fn management_router_with_config<E>(
    engine: Arc<E>,
    compiler: Arc<dyn ConfigCompiler>,
    idempotency: Arc<dyn IdempotencyRepository>,
    api_config: ApiConfig,
) -> axum::Router
where
    E: GatewayEngine + ?Sized + 'static,
{
    let gateway = Arc::new(EngineGatewayPort::new(engine));
    let core = Arc::new(GatewayService::new(gateway, compiler));
    let use_cases = Arc::new(IdempotentGatewayUseCases::new(core, idempotency));
    router_with_config(ApiState::new(use_cases), api_config)
}

/// Binds a management listener after applying the explicit exposure policy.
///
/// Binding is kept separate from router construction so callers can run the
/// same HTTP graph in tests, embedded processes, or a dedicated supervisor.
pub async fn bind_management_listener(
    address: SocketAddr,
    bind_policy: &dyn crate::ManagementBindPolicy,
) -> Result<TcpListener> {
    bind_policy.validate(address)?;
    TcpListener::bind(address)
        .await
        .map_err(|error| panel_errors::PanelError::internal(error.to_string()))
}

/// Serves a management router with graceful shutdown on an injected listener.
///
/// The listener is intentionally supplied by the composition root, keeping
/// authentication, bind policy and process lifecycle independent from the
/// transport-neutral HTTP adapter.
pub async fn serve_management(
    listener: TcpListener,
    router: axum::Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_application::{IdempotencyKey, RequestDeadline, RequestId};
    use tokio::sync::oneshot;

    fn context(deadline: &str) -> CommandContext {
        CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("correlation-1").unwrap(),
            "tester",
            RequestDeadline::new(deadline).unwrap(),
            IdempotencyKey::new("idempotency-1").unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn direct_engine_adapter_rejects_expired_mutation_deadlines() {
        let error = deadline_budget(&context("2000-01-01T00:00:00Z")).unwrap_err();
        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::DEADLINE_EXCEEDED
        );
    }

    #[tokio::test]
    async fn listener_binding_applies_exposure_policy_before_socket_creation() {
        let error = bind_management_listener(
            "192.0.2.10:0".parse().unwrap(),
            &crate::LoopbackOnlyManagementBindPolicy,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );

        let listener = bind_management_listener(
            "127.0.0.1:0".parse().unwrap(),
            &crate::LoopbackOnlyManagementBindPolicy,
        )
        .await
        .unwrap();
        assert!(listener.local_addr().unwrap().ip().is_loopback());
    }

    #[tokio::test]
    async fn management_server_uses_injected_listener_and_graceful_shutdown() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = axum::Router::new().route(
            "/healthz",
            axum::routing::get(|| async { axum::http::StatusCode::NO_CONTENT }),
        );
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let task = tokio::spawn(serve_management(listener, router, async move {
            let _ = shutdown_receiver.await;
        }));

        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        use tokio::io::AsyncWriteExt;
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        use tokio::io::AsyncReadExt;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 204"));
        shutdown_sender.send(()).unwrap();
        task.await.unwrap().unwrap();
    }
}
