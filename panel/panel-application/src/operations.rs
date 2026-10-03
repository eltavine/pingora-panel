//! Records of operations whose effect lives outside the recording service,
//! such as calls to the gateway, for the audit trail.

use crate::{
    CommandContext, DataPlaneState, FileChecks, GatewayRuntimePort, RequestScope, UpstreamHealth,
    UpstreamHealthReport,
};
use async_trait::async_trait;
use panel_errors::Result;
use serde_json::{json, Value};
use std::sync::Arc;

/// Where operations are recorded.
#[async_trait]
pub trait OperationLog: Send + Sync {
    /// Records `event_type` about `target`, an aggregate type and ID. What
    /// it describes already happened, so recording does not fail the caller.
    async fn record(
        &self,
        context: &CommandContext,
        event_type: &str,
        target: (&str, &str),
        data: Value,
    );
}

/// The data plane as a whole, the target of its operations.
const DATA_PLANE: (&str, &str) = ("gateway", "data-plane");

/// A runtime port that records each change it makes, and each refusal, as
/// `gateway.<change>` or `gateway.operation.refused`.
pub struct RecordedRuntime {
    inner: Arc<dyn GatewayRuntimePort>,
    log: Arc<dyn OperationLog>,
}

impl RecordedRuntime {
    pub fn new(inner: Arc<dyn GatewayRuntimePort>, log: Arc<dyn OperationLog>) -> Self {
        Self { inner, log }
    }

    async fn record<T>(
        &self,
        context: &CommandContext,
        operation: &str,
        target: (&str, &str),
        result: &Result<T>,
        data: impl FnOnce(&T) -> Value,
    ) {
        match result {
            Ok(value) => {
                self.log
                    .record(
                        context,
                        &format!("gateway.{operation}"),
                        target,
                        data(value),
                    )
                    .await;
            }
            Err(error) => {
                self.log
                    .record(
                        context,
                        "gateway.operation.refused",
                        target,
                        json!({
                            "operation": operation,
                            "code": error.code.as_str(),
                            "message": error.message,
                        }),
                    )
                    .await;
            }
        }
    }
}

fn generation(state: &DataPlaneState) -> Value {
    json!({ "generation": state.generation, "workers": state.worker_count })
}

#[async_trait]
impl GatewayRuntimePort for RecordedRuntime {
    async fn data_plane(&self, scope: RequestScope) -> Result<DataPlaneState> {
        self.inner.data_plane(scope).await
    }

    async fn reload(&self, context: CommandContext) -> Result<DataPlaneState> {
        let result = self.inner.reload(context.clone()).await;
        self.record(&context, "reloaded", DATA_PLANE, &result, generation)
            .await;
        result
    }

    async fn set_worker_count(
        &self,
        context: CommandContext,
        workers: u32,
    ) -> Result<DataPlaneState> {
        let result = self.inner.set_worker_count(context.clone(), workers).await;
        self.record(&context, "workers.changed", DATA_PLANE, &result, generation)
            .await;
        result
    }

    async fn shutdown(&self, context: CommandContext) -> Result<()> {
        let result = self.inner.shutdown(context.clone()).await;
        self.record(&context, "shutdown.requested", DATA_PLANE, &result, |()| {
            json!({})
        })
        .await;
        result
    }

    async fn upstream_health(&self, scope: RequestScope) -> Result<UpstreamHealthReport> {
        self.inner.upstream_health(scope).await
    }

    async fn file_checks(&self, scope: RequestScope) -> Result<FileChecks> {
        self.inner.file_checks(scope).await
    }

    async fn set_endpoint_drained(
        &self,
        context: CommandContext,
        upstream: String,
        endpoint: String,
        drained: bool,
    ) -> Result<UpstreamHealth> {
        let result = self
            .inner
            .set_endpoint_drained(context.clone(), upstream.clone(), endpoint.clone(), drained)
            .await;
        let operation = if drained {
            "endpoint.drained"
        } else {
            "endpoint.restored"
        };
        self.record(
            &context,
            operation,
            ("upstream", &upstream),
            &result,
            |_| json!({ "upstream": upstream.as_str(), "endpoint": endpoint.as_str() }),
        )
        .await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IdempotencyKey, RequestDeadline, RequestId};
    use panel_errors::PanelError;
    use std::sync::Mutex;

    struct Runtime;

    #[async_trait]
    impl GatewayRuntimePort for Runtime {
        async fn data_plane(&self, _scope: RequestScope) -> Result<DataPlaneState> {
            Ok(DataPlaneState::default())
        }

        async fn reload(&self, _context: CommandContext) -> Result<DataPlaneState> {
            Ok(DataPlaneState {
                generation: 3,
                worker_count: 2,
                ..DataPlaneState::default()
            })
        }

        async fn set_worker_count(
            &self,
            _context: CommandContext,
            _workers: u32,
        ) -> Result<DataPlaneState> {
            Err(PanelError::invalid_argument(
                "workers must be between 1 and 64",
            ))
        }

        async fn shutdown(&self, _context: CommandContext) -> Result<()> {
            Ok(())
        }

        async fn upstream_health(&self, _scope: RequestScope) -> Result<UpstreamHealthReport> {
            Ok(UpstreamHealthReport::default())
        }

        async fn set_endpoint_drained(
            &self,
            _context: CommandContext,
            upstream: String,
            _endpoint: String,
            _drained: bool,
        ) -> Result<UpstreamHealth> {
            Ok(UpstreamHealth {
                upstream_id: upstream,
                ..UpstreamHealth::default()
            })
        }
    }

    #[derive(Default)]
    struct Log(Mutex<Vec<(String, String, String, Value)>>);

    #[async_trait]
    impl OperationLog for Log {
        async fn record(
            &self,
            context: &CommandContext,
            event_type: &str,
            target: (&str, &str),
            data: Value,
        ) {
            self.0.lock().unwrap().push((
                context.actor().to_owned(),
                event_type.to_owned(),
                format!("{}/{}", target.0, target.1),
                data,
            ));
        }
    }

    fn context() -> CommandContext {
        CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("request-1").unwrap(),
            "ops",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn changes_and_refusals_are_recorded_and_reads_are_not() {
        let log = Arc::new(Log::default());
        let runtime = RecordedRuntime::new(Arc::new(Runtime), log.clone());
        runtime.data_plane(context().scope()).await.unwrap();
        runtime.reload(context()).await.unwrap();
        runtime.set_worker_count(context(), 0).await.unwrap_err();
        runtime.shutdown(context()).await.unwrap();
        runtime
            .set_endpoint_drained(context(), "pool".into(), "node".into(), true)
            .await
            .unwrap();
        let recorded = log.0.lock().unwrap().clone();
        assert_eq!(
            recorded,
            [
                (
                    "ops".into(),
                    "gateway.reloaded".into(),
                    "gateway/data-plane".into(),
                    json!({ "generation": 3, "workers": 2 })
                ),
                (
                    "ops".into(),
                    "gateway.operation.refused".into(),
                    "gateway/data-plane".into(),
                    json!({
                        "operation": "workers.changed",
                        "code": "INVALID_ARGUMENT",
                        "message": "workers must be between 1 and 64"
                    })
                ),
                (
                    "ops".into(),
                    "gateway.shutdown.requested".into(),
                    "gateway/data-plane".into(),
                    json!({})
                ),
                (
                    "ops".into(),
                    "gateway.endpoint.drained".into(),
                    "upstream/pool".into(),
                    json!({ "upstream": "pool", "endpoint": "node" })
                ),
            ]
        );
    }
}
