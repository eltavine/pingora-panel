use crate::TlsCredentials;
use hyper_util::rt::TokioIo;
use panel_errors::{PanelError, Result};
use panel_pki::WorkloadIdentity;
use rustls_pki_types::ServerName;
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};
use tokio_rustls::{client::TlsStream, Connect, TlsConnector};
use tonic::transport::{Channel, Endpoint, Uri};

/// Dials the address in a URI and authenticates the server as one peer
/// identity, independent of the host name used to reach it.
#[derive(Clone)]
pub struct MtlsConnector {
    credentials: Arc<TlsCredentials>,
    server_name: ServerName<'static>,
}

impl MtlsConnector {
    pub fn new(credentials: Arc<TlsCredentials>, peer: &WorkloadIdentity) -> Result<Self> {
        let server_name = ServerName::try_from(peer.dns_name())
            .map_err(|_| PanelError::invalid_argument("invalid peer identity"))?;
        Ok(Self {
            credentials,
            server_name,
        })
    }

    fn handshake<S: AsyncRead + AsyncWrite + Unpin>(&self, stream: S) -> Connect<S> {
        TlsConnector::from(self.credentials.client_config())
            .connect(self.server_name.clone(), stream)
    }
}

impl tower::Service<Uri> for MtlsConnector {
    type Response = TokioIo<TlsStream<TcpStream>>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let tls = self.clone();
        Box::pin(async move {
            let host = uri
                .host()
                .map(|host| {
                    host.trim_start_matches('[')
                        .trim_end_matches(']')
                        .to_owned()
                })
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "the address has no host")
                })?;
            let port = uri.port_u16().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "the address has no port")
            })?;
            let tcp = TcpStream::connect((host.as_str(), port)).await?;
            tcp.set_nodelay(true)?;
            tls.handshake(tcp).await.map(TokioIo::new)
        })
    }
}

/// Dials one Unix domain socket, whatever the URI says, and authenticates
/// the server as one peer identity.
#[cfg(unix)]
#[derive(Clone)]
pub struct UnixMtlsConnector {
    path: Arc<std::path::Path>,
    tls: MtlsConnector,
}

#[cfg(unix)]
impl UnixMtlsConnector {
    pub fn new(
        path: impl AsRef<std::path::Path>,
        credentials: Arc<TlsCredentials>,
        peer: &WorkloadIdentity,
    ) -> Result<Self> {
        Ok(Self {
            path: Arc::from(path.as_ref()),
            tls: MtlsConnector::new(credentials, peer)?,
        })
    }
}

#[cfg(unix)]
impl tower::Service<Uri> for UnixMtlsConnector {
    type Response = TokioIo<TlsStream<tokio::net::UnixStream>>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: Uri) -> Self::Future {
        let (path, tls) = (Arc::clone(&self.path), self.tls.clone());
        Box::pin(async move {
            let stream = tokio::net::UnixStream::connect(&*path).await?;
            tls.handshake(stream).await.map(TokioIo::new)
        })
    }
}

/// The `host:port` a URL such as `https://config-service:50061` names.
pub fn address_of(url: &str) -> Result<String> {
    let uri: Uri = url
        .parse()
        .map_err(|_| PanelError::invalid_argument(format!("invalid peer address `{url}`")))?;
    match (uri.host(), uri.port_u16()) {
        (Some(host), Some(port)) => Ok(format!("{host}:{port}")),
        _ => Err(PanelError::invalid_argument(format!(
            "peer address `{url}` needs a host and a port"
        ))),
    }
}

/// A channel to `peer` at `address` (`host:port`) over mutual TLS. It
/// connects on first use.
pub fn channel(
    address: &str,
    peer: &WorkloadIdentity,
    credentials: Arc<TlsCredentials>,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<Channel> {
    let endpoint = Endpoint::from_shared(format!("http://{address}"))
        .map_err(|error| PanelError::invalid_argument(format!("invalid peer address: {error}")))?
        .connect_timeout(connect_timeout)
        .timeout(request_timeout);
    Ok(endpoint.connect_with_connector_lazy(MtlsConnector::new(credentials, peer)?))
}

/// A channel to `peer` on the Unix domain socket at `path`, over mutual TLS.
/// It connects on first use.
#[cfg(unix)]
pub fn unix_channel(
    path: impl AsRef<std::path::Path>,
    peer: &WorkloadIdentity,
    credentials: Arc<TlsCredentials>,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<Channel> {
    let endpoint = Endpoint::from_static("http://localhost")
        .connect_timeout(connect_timeout)
        .timeout(request_timeout);
    Ok(endpoint.connect_with_connector_lazy(UnixMtlsConnector::new(path, credentials, peer)?))
}
