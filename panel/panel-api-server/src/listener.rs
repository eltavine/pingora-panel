//! The public listener: a loopback TCP address, or a Unix domain socket that
//! only a local reverse proxy in the socket's group can reach.

use axum::{extract::ConnectInfo, Extension, Router};
use panel_errors::{PanelError, Result};
use std::net::{Ipv4Addr, SocketAddr};
#[cfg(unix)]
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

/// The prefix that names a Unix domain socket instead of an address.
pub const UNIX_PREFIX: &str = "unix:";

/// Where the API is served, bound before anything else starts so a taken
/// address fails the start.
#[derive(Debug)]
pub(crate) enum PublicListener {
    Tcp(std::net::TcpListener),
    #[cfg(unix)]
    Unix {
        listener: std::os::unix::net::UnixListener,
        path: PathBuf,
    },
}

impl PublicListener {
    /// Binds `spec`, a loopback `ip:port` or `unix:/absolute/path`.
    pub(crate) fn bind(name: &str, spec: &str) -> Result<Self> {
        match spec.strip_prefix(UNIX_PREFIX) {
            Some(path) => Self::bind_unix(name, path),
            None => {
                let address: SocketAddr = spec.parse().map_err(|_| {
                    PanelError::invalid_argument(format!(
                        "{name} must be a loopback ip:port or {UNIX_PREFIX}/path"
                    ))
                })?;
                let address = panel_service::require_loopback(name, address)?;
                let listener = std::net::TcpListener::bind(address).map_err(|error| {
                    PanelError::precondition_failed(format!(
                        "cannot bind the public listener on {address}: {error}"
                    ))
                })?;
                listener.set_nonblocking(true).map_err(configure)?;
                Ok(Self::Tcp(listener))
            }
        }
    }

    #[cfg(unix)]
    fn bind_unix(name: &str, path: &str) -> Result<Self> {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        let path = Path::new(path);
        if !path.is_absolute() {
            return Err(PanelError::invalid_argument(format!(
                "{name} must name the socket by an absolute path"
            )));
        }
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                std::fs::remove_file(path).map_err(|error| {
                    PanelError::precondition_failed(format!(
                        "cannot replace the stale socket {}: {error}",
                        path.display()
                    ))
                })?;
            }
            Ok(_) => {
                return Err(PanelError::precondition_failed(format!(
                    "{} exists and is not a socket",
                    path.display()
                )))
            }
            Err(_) => {}
        }
        let listener = std::os::unix::net::UnixListener::bind(path).map_err(|error| {
            PanelError::precondition_failed(format!(
                "cannot bind the public socket {}: {error}",
                path.display()
            ))
        })?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
            .map_err(configure)?;
        listener.set_nonblocking(true).map_err(configure)?;
        Ok(Self::Unix {
            listener,
            path: path.to_owned(),
        })
    }

    #[cfg(not(unix))]
    fn bind_unix(name: &str, _path: &str) -> Result<Self> {
        Err(PanelError::invalid_argument(format!(
            "{name} cannot name a Unix socket on this platform"
        )))
    }

    /// Serves `app` until `shutdown`, then removes a socket file.
    pub(crate) async fn serve(self, app: Router, shutdown: CancellationToken) -> Result<()> {
        match self {
            Self::Tcp(listener) => {
                let listener = tokio::net::TcpListener::from_std(listener).map_err(configure)?;
                tracing::info!(address = ?listener.local_addr().ok(), "public API listening");
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .map_err(|error| PanelError::internal(format!("public listener failed: {error}")))
            }
            #[cfg(unix)]
            Self::Unix { listener, path } => {
                let listener = tokio::net::UnixListener::from_std(listener).map_err(configure)?;
                tracing::info!(socket = %path.display(), "public API listening");
                // A local proxy is the only peer, as on loopback, so the
                // address it forwards is trusted the same way.
                let local = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
                let served = axum::serve(listener, app.layer(Extension(ConnectInfo(local))))
                    .with_graceful_shutdown(shutdown.cancelled_owned())
                    .await;
                let _ = std::fs::remove_file(&path);
                served.map_err(|error| {
                    PanelError::internal(format!("public listener failed: {error}"))
                })
            }
        }
    }
}

fn configure(error: std::io::Error) -> PanelError {
    PanelError::internal(format!("cannot configure the public listener: {error}"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn peer(ConnectInfo(peer): ConnectInfo<SocketAddr>) -> String {
        peer.ip().to_string()
    }

    #[tokio::test]
    async fn unix_sockets_serve_local_proxies_only() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("api.sock");
        let spec = format!("{UNIX_PREFIX}{}", path.display());
        std::fs::write(&path, "not a socket").unwrap();
        assert!(PublicListener::bind("ADDR", &spec).is_err());
        std::fs::remove_file(&path).unwrap();
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        let listener = PublicListener::bind("ADDR", &spec).expect("a stale socket is replaced");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o660);

        let shutdown = CancellationToken::new();
        let served =
            tokio::spawn(listener.serve(Router::new().route("/peer", get(peer)), shutdown.clone()));
        let mut stream = tokio::net::UnixStream::connect(&path).await.unwrap();
        stream
            .write_all(b"GET /peer HTTP/1.1\r\nhost: panel\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("127.0.0.1"), "{response}");
        shutdown.cancel();
        served.await.unwrap().unwrap();
        assert!(!path.exists(), "the socket is removed at shutdown");

        for spec in ["unix:relative.sock", "0.0.0.0:8080", "panel:8080"] {
            assert!(PublicListener::bind("ADDR", spec).is_err(), "{spec}");
        }
    }
}
