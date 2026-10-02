#![forbid(unsafe_code)]

use gateway_grpc_client::GatewayGrpcClient;
use gatewayd::{build_gateway_runtime_with_options, GatewaydServiceOptions, ProcessRuntimeInfo};
use panel_application::{
    CommandContext, GatewayPort, IdempotencyKey, RequestDeadline, RequestId, RequestScope,
    TraceContext,
};
use panel_domain::RevisionId;
use panel_engine::{
    GatewayEvent, GatewayEventSink, GatewayRequestMetadata, GatewayRequestOperation,
};
use panel_ir::RuntimeSnapshot;
use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::oneshot};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

#[derive(Default)]
struct StartedRequests(Mutex<Vec<(GatewayRequestOperation, GatewayRequestMetadata)>>);

impl GatewayEventSink for StartedRequests {
    fn emit(&self, event: &GatewayEvent) {
        if let GatewayEvent::RequestStarted {
            operation,
            metadata,
        } = event
        {
            self.0.lock().unwrap().push((*operation, metadata.clone()));
        }
    }
}

impl StartedRequests {
    async fn wait_for(
        &self,
        count: usize,
    ) -> Vec<(GatewayRequestOperation, GatewayRequestMetadata)> {
        for _ in 0..200 {
            let started = self.0.lock().unwrap().clone();
            if started.len() >= count {
                return started;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("gateway did not report {count} started requests");
    }
}

fn trace() -> Option<TraceContext> {
    TraceContext::parse(TRACEPARENT, Some("rojo=00f067aa0ba902b7"))
}

#[tokio::test]
async fn remote_gateway_requests_keep_the_callers_identity_and_trace() {
    let state = tempfile::tempdir().unwrap();
    let started = Arc::new(StartedRequests::default());
    let runtime = build_gateway_runtime_with_options(
        state.path(),
        Arc::new(ProcessRuntimeInfo::new("test", NonZeroU32::MIN)),
        GatewaydServiceOptions::default()
            .with_event_sink(Arc::clone(&started) as Arc<dyn GatewayEventSink>),
    )
    .await
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway = runtime.services.gateway();
    let (shutdown, requested) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let policy = gateway.transport_policy();
        Server::builder()
            .add_service(policy.gateway_server(gateway))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = requested.await;
            })
            .await
    });
    let client = GatewayGrpcClient::connect(format!("http://{address}"))
        .await
        .unwrap();

    let query = RequestScope::new(RequestId::new("status-1").unwrap())
        .with_correlation_id(RequestId::new("flow-1").unwrap())
        .with_trace_context(trace());
    client.status_with_scope(query.clone()).await.unwrap();
    let snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    assert!(
        client
            .validate_with_scope(query, snapshot.clone())
            .await
            .unwrap()
            .valid
    );
    let command = CommandContext::new(
        RequestId::new("prepare-1").unwrap(),
        RequestId::new("flow-1").unwrap(),
        "operator",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new("prepare-1").unwrap(),
    )
    .unwrap()
    .with_trace_context(trace());
    client
        .prepare_with_context(command, snapshot)
        .await
        .unwrap();
    client.status().await.unwrap();

    let started = started.wait_for(4).await;
    let identity = |index: usize| {
        let (operation, metadata) = &started[index];
        (
            *operation,
            metadata.request_id.as_str(),
            metadata.correlation_id.as_str(),
            metadata.trace_id.as_str(),
        )
    };
    assert_eq!(
        identity(0),
        (
            GatewayRequestOperation::Status,
            "status-1",
            "flow-1",
            TRACE_ID
        )
    );
    assert_eq!(
        identity(1),
        (
            GatewayRequestOperation::Validate,
            "status-1",
            "flow-1",
            TRACE_ID
        )
    );
    assert_eq!(
        identity(2),
        (
            GatewayRequestOperation::Prepare,
            "prepare-1",
            "flow-1",
            TRACE_ID
        )
    );
    let (operation, standalone) = &started[3];
    assert_eq!(*operation, GatewayRequestOperation::Status);
    assert_eq!(standalone.correlation_id, standalone.request_id);
    assert!(standalone.trace_id.is_empty());

    shutdown.send(()).unwrap();
    server.await.unwrap().unwrap();
}
