#![forbid(unsafe_code)]

use async_trait::async_trait;
use gateway_grpc::{GatewayGrpcService, GatewayTransportPolicy};
use gateway_grpc_client::GatewayGrpcClient;
use panel_application::{GatewayPort, GatewayStatus};
use panel_engine::{
    AbortReceipt, ActivateRequest, ActivationReceipt, EngineCapabilities, EngineCapability,
    GatewayEngine as EnginePort, GatewayRuntimeInfo, GatewayRuntimeInfoProvider, PrepareReceipt,
    PrepareRequest, PrepareToken,
};
use panel_errors::{PanelError, Result, ValidationReport};
use panel_ir::RuntimeSnapshot;
use std::{collections::BTreeSet, sync::Arc};
use tokio::{net::TcpListener, sync::oneshot};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

struct ContractEngine;

#[async_trait]
impl EnginePort for ContractEngine {
    async fn capabilities(&self) -> Result<EngineCapabilities> {
        Ok(EngineCapabilities {
            protocol_version: "protocol-1".into(),
            build_version: "build-123".into(),
            schema_version: "schema-789".into(),
            adapter_version: "adapter-456".into(),
            capabilities: BTreeSet::from([EngineCapability::new("test", "1")]),
        })
    }

    async fn validate(&self, _snapshot: RuntimeSnapshot) -> Result<ValidationReport> {
        Err(PanelError::unsupported_capability("not used"))
    }
    async fn prepare(&self, _request: PrepareRequest) -> Result<PrepareReceipt> {
        Err(PanelError::unsupported_capability("not used"))
    }
    async fn activate(&self, _request: ActivateRequest) -> Result<ActivationReceipt> {
        Err(PanelError::unsupported_capability("not used"))
    }
    async fn abort(&self, _token: PrepareToken) -> Result<AbortReceipt> {
        Err(PanelError::unsupported_capability("not used"))
    }
    async fn status(&self) -> Result<panel_engine::GatewayStatus> {
        Ok(panel_engine::GatewayStatus {
            ready: false,
            message: Some("recovering".into()),
            active_revision_id: None,
            active_hash: None,
            prepared_count: 0,
            adapter_version: "engine-status-adapter".into(),
            schema_version: "engine-status-schema".into(),
        })
    }
}

struct RuntimeInfo;

impl GatewayRuntimeInfoProvider for RuntimeInfo {
    fn snapshot(&self) -> GatewayRuntimeInfo {
        GatewayRuntimeInfo {
            gateway_version: "gateway-1".into(),
            started_at_unix_seconds: 1,
            uptime_seconds: 2,
            worker_count: 3,
        }
    }
}

#[tokio::test]
async fn status_round_trip_preserves_cross_adapter_semantics() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, requested) = oneshot::channel();
    let service =
        GatewayGrpcService::with_runtime_info(Arc::new(ContractEngine), Arc::new(RuntimeInfo));
    let task = tokio::spawn(async move {
        Server::builder()
            .add_service(GatewayTransportPolicy::default().gateway_server(service))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = requested.await;
            })
            .await
            .unwrap();
    });

    let client = GatewayGrpcClient::connect(format!("http://{address}"))
        .await
        .unwrap();
    let status = client.status().await.unwrap();
    assert_status(status);

    shutdown.send(()).unwrap();
    task.await.unwrap();
}

fn assert_status(status: GatewayStatus) {
    assert!(!status.ready());
    assert_eq!(status.message(), Some("recovering"));
    assert_eq!(status.adapter_version(), "adapter-456");
    assert_eq!(status.schema_version(), "schema-789");
}
