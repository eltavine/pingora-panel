//! Pebble, Let's Encrypt's test CA, for integration tests.
//!
//! `panel/scripts/dev-services.sh up` starts Pebble with a DNS test server
//! that resolves every name to 127.0.0.1, and `env` prints the variables
//! below. Without them the tests are skipped, unless [`REQUIRE_ENV`] is set,
//! in which case a missing server fails them so CI cannot silently lose
//! coverage.

use crate::{challenges::DnsProvider, client::Directory};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{Mutex, MutexGuard},
    task::JoinHandle,
};

pub const DIRECTORY_ENV: &str = "PANEL_TEST_ACME_DIRECTORY";
/// The PEM root Pebble's HTTPS listener chains to.
pub const CA_ENV: &str = "PANEL_TEST_ACME_CA";
/// The port of 127.0.0.1 Pebble validates HTTP-01 challenges on.
pub const HTTP_PORT_ENV: &str = "PANEL_TEST_ACME_HTTP_PORT";
/// The management API of the DNS test server Pebble resolves through.
pub const DNS_ENV: &str = "PANEL_TEST_ACME_DNS";
pub const REQUIRE_ENV: &str = "PANEL_REQUIRE_INTEGRATION_SERVICES";

/// Only one test at a time can answer on the HTTP-01 port.
static HTTP01_PORT: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// A running Pebble.
#[derive(Clone, Debug)]
pub struct Pebble {
    pub directory: Directory,
    pub http_port: u16,
    dns: String,
}

impl Pebble {
    pub fn from_env() -> Option<Self> {
        let variables = [DIRECTORY_ENV, CA_ENV, HTTP_PORT_ENV, DNS_ENV].map(std::env::var);
        let [Ok(url), Ok(ca), Ok(port), Ok(dns)] = variables else {
            assert!(
                std::env::var_os(REQUIRE_ENV).is_none(),
                "{REQUIRE_ENV} is set but {DIRECTORY_ENV}, {CA_ENV}, {HTTP_PORT_ENV} or {DNS_ENV} is not"
            );
            eprintln!("skipping: {DIRECTORY_ENV} is not set");
            return None;
        };
        Some(Self {
            directory: Directory {
                url,
                ca_bundle: Some(std::fs::read_to_string(&ca).expect("the Pebble root is readable")),
            },
            http_port: port.parse().expect("the HTTP-01 port is a port"),
            dns,
        })
    }

    /// Answers HTTP-01 challenges with the files in `directory`, as the
    /// gateway does, until the returned server is dropped.
    pub async fn serve_http01(&self, directory: &Path) -> ChallengeServer {
        let guard = HTTP01_PORT.lock().await;
        let mut attempts = 0;
        let listener = loop {
            match TcpListener::bind(("127.0.0.1", self.http_port)).await {
                Ok(listener) => break listener,
                Err(error) if attempts < 50 => {
                    attempts += 1;
                    tracing::debug!(%error, "the HTTP-01 port is still taken");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => panic!("port {} is not free: {error}", self.http_port),
            }
        };
        let directory: PathBuf = directory.to_owned();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let directory = directory.clone();
                tokio::spawn(async move {
                    let mut request = vec![0; 4096];
                    let read = stream.read(&mut request).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..read]);
                    let answer = request
                        .split_whitespace()
                        .nth(1)
                        .and_then(|path| path.strip_prefix("/.well-known/acme-challenge/"))
                        .and_then(|token| std::fs::read(directory.join(token)).ok());
                    let response = match answer {
                        Some(body) => [
                            format!(
                                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                                body.len()
                            )
                            .into_bytes(),
                            body,
                        ]
                        .concat(),
                        None => {
                            b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                                .to_vec()
                        }
                    };
                    let _ = stream.write_all(&response).await;
                });
            }
        });
        ChallengeServer { task, _port: guard }
    }

    /// TXT records in the DNS test server Pebble resolves through.
    pub fn dns(&self) -> TestDns {
        TestDns {
            management: self.dns.trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
        }
    }
}

/// Serves HTTP-01 challenges until dropped.
pub struct ChallengeServer {
    task: JoinHandle<()>,
    _port: MutexGuard<'static, ()>,
}

impl Drop for ChallengeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Publishes TXT records through the DNS test server's management API.
#[derive(Clone, Debug)]
pub struct TestDns {
    management: String,
    client: reqwest::Client,
}

impl TestDns {
    async fn post(&self, path: &str, body: serde_json::Value) -> Result<()> {
        let response = self
            .client
            .post(format!("{}/{path}", self.management))
            .json(&body)
            .send()
            .await
            .map_err(|error| PanelError::unavailable(format!("the DNS test server: {error}")))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(PanelError::unavailable(format!(
                "the DNS test server answered {}",
                response.status()
            )))
        }
    }
}

#[async_trait]
impl DnsProvider for TestDns {
    async fn add_txt(&self, name: &str, value: &str) -> Result<()> {
        self.post(
            "set-txt",
            serde_json::json!({ "host": name, "value": value }),
        )
        .await
    }

    async fn remove_txt(&self, name: &str, _value: &str) -> Result<()> {
        self.post("clear-txt", serde_json::json!({ "host": name }))
            .await
    }
}
