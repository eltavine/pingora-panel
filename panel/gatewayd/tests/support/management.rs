use gatewayd::{
    bind_management_listener, management_router_with_config, serve_management,
    LoopbackOnlyManagementBindPolicy,
};
use panel_api::ApiConfig;
use panel_config_json::JsonRuntimeSnapshotCompiler;
use panel_engine::GatewayEngine;
use panel_persistence_memory::MemoryIdempotencyRepository;
use reqwest::{Client, RequestBuilder, Response};
use serde_json::Value;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

pub struct ManagementServer {
    address: SocketAddr,
    client: Client,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl ManagementServer {
    pub async fn start(engine: Arc<dyn GatewayEngine>, max_body_bytes: usize) -> Self {
        let router = management_router_with_config(
            engine,
            Arc::new(JsonRuntimeSnapshotCompiler::default()),
            Arc::new(MemoryIdempotencyRepository::new()),
            ApiConfig::new(max_body_bytes).unwrap(),
        );
        let listener = bind_management_listener(
            "127.0.0.1:0".parse().unwrap(),
            &LoopbackOnlyManagementBindPolicy,
        )
        .await
        .unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown, receiver) = oneshot::channel();
        let task = tokio::spawn(serve_management(listener, router, async {
            let _ = receiver.await;
        }));
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .unwrap();
        Self {
            address,
            client,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    pub fn get(&self, path: &str) -> RequestBuilder {
        self.client.get(format!("http://{}{path}", self.address))
    }
    pub fn post(&self, path: &str, body: &Value) -> RequestBuilder {
        self.client
            .post(format!("http://{}{path}", self.address))
            .json(body)
    }
    pub fn mutation(&self, path: &str, body: &Value, key: &str) -> RequestBuilder {
        self.mutation_with_deadline(path, body, key, "2099-01-01T00:00:00Z")
    }

    pub fn mutation_with_deadline(
        &self,
        path: &str,
        body: &Value,
        key: &str,
        deadline: &str,
    ) -> RequestBuilder {
        self.post(path, body)
            .header("x-actor", "operator")
            .header("x-request-id", format!("request-{key}"))
            .header("x-deadline", deadline)
            .header("Idempotency-Key", key)
    }

    pub async fn stop(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        let mut task = self.task.take().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
        if result.is_err() {
            task.abort();
        }
        result
            .expect("management server must drain")
            .unwrap()
            .unwrap();
        let rebound = TcpListener::bind(self.address)
            .await
            .expect("listener must be released");
        drop(rebound);
    }
}

impl Drop for ManagementServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub async fn json(response: Response, expected: u16) -> Value {
    assert_eq!(response.status().as_u16(), expected);
    response.json().await.unwrap()
}

pub async fn problem(response: Response, expected: u16, code: &str) -> Value {
    assert_eq!(response.status().as_u16(), expected);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    let request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let value = json(response, expected).await;
    assert_eq!(value["request_id"], request_id);
    assert_eq!(value["code"], code);
    value
}
