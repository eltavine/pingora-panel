use crate::TlsCredentials;
use rustls_pki_types::CertificateDer;
use std::{
    fmt, io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tokio_rustls::{server::TlsStream, TlsAcceptor};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::server::Connected;

/// Where a connection came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PeerAddress {
    Tcp(SocketAddr),
    /// A local process, by the credentials of its Unix domain socket.
    Unix {
        uid: u32,
        pid: Option<i32>,
    },
}

impl fmt::Display for PeerAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp(address) => address.fmt(formatter),
            Self::Unix {
                uid,
                pid: Some(pid),
            } => write!(formatter, "uid {uid}, pid {pid}"),
            Self::Unix { uid, pid: None } => write!(formatter, "uid {uid}"),
        }
    }
}

/// The authenticated peer of a connection.
#[derive(Clone, Debug)]
pub struct PeerIdentity {
    pub(crate) remote: PeerAddress,
    pub(crate) certificate: Option<CertificateDer<'static>>,
}

impl PeerIdentity {
    pub fn remote(&self) -> PeerAddress {
        self.remote
    }

    /// The peer's end-entity certificate, verified against the trust bundle
    /// during the handshake.
    pub fn certificate(&self) -> Option<&CertificateDer<'static>> {
        self.certificate.as_ref()
    }
}

/// An established server-side TLS connection.
pub struct TlsConnection<S = TcpStream> {
    stream: TlsStream<S>,
    peer: PeerIdentity,
}

impl<S> Connected for TlsConnection<S> {
    type ConnectInfo = PeerIdentity;

    fn connect_info(&self) -> PeerIdentity {
        self.peer.clone()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for TlsConnection<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for TlsConnection<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
}

/// Accepts TLS connections on `listener` for a tonic server. Each handshake
/// runs in its own task under `handshake_timeout`, so a slow or failing
/// client never delays others; failed handshakes are dropped.
pub fn incoming(
    listener: TcpListener,
    credentials: Arc<TlsCredentials>,
    handshake_timeout: Duration,
) -> ReceiverStream<io::Result<TlsConnection>> {
    let (sender, receiver) = mpsc::channel(64);
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                () = sender.closed() => return,
            };
            let (tcp, remote) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(%error, "connection not accepted");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let _ = tcp.set_nodelay(true);
            handshake(
                tcp,
                PeerAddress::Tcp(remote),
                &credentials,
                handshake_timeout,
                sender.clone(),
            );
        }
    });
    ReceiverStream::new(receiver)
}

/// Accepts TLS connections on a Unix domain socket, from processes running
/// as one of `users` only; others are refused before their handshake and
/// logged as security events.
#[cfg(unix)]
pub fn incoming_unix(
    listener: tokio::net::UnixListener,
    users: Vec<u32>,
    credentials: Arc<TlsCredentials>,
    handshake_timeout: Duration,
) -> ReceiverStream<io::Result<TlsConnection<tokio::net::UnixStream>>> {
    let (sender, receiver) = mpsc::channel(64);
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                () = sender.closed() => return,
            };
            let stream = match accepted {
                Ok((stream, _)) => stream,
                Err(error) => {
                    tracing::warn!(%error, "connection not accepted");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let remote = match stream.peer_cred() {
                Ok(peer) => PeerAddress::Unix {
                    uid: peer.uid(),
                    pid: peer.pid(),
                },
                Err(error) => {
                    tracing::warn!(event = "peer_refused", %error, "peer credentials unavailable");
                    continue;
                }
            };
            if !matches!(remote, PeerAddress::Unix { uid, .. } if users.contains(&uid)) {
                tracing::warn!(event = "peer_refused", peer = %remote, "user may not connect");
                continue;
            }
            handshake(
                stream,
                remote,
                &credentials,
                handshake_timeout,
                sender.clone(),
            );
        }
    });
    ReceiverStream::new(receiver)
}

fn handshake<S>(
    stream: S,
    remote: PeerAddress,
    credentials: &TlsCredentials,
    handshake_timeout: Duration,
    sender: mpsc::Sender<io::Result<TlsConnection<S>>>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let acceptor = TlsAcceptor::from(credentials.server_config());
    tokio::spawn(async move {
        match tokio::time::timeout(handshake_timeout, acceptor.accept(stream)).await {
            Ok(Ok(stream)) => {
                let certificate = stream
                    .get_ref()
                    .1
                    .peer_certificates()
                    .and_then(|chain| chain.first())
                    .map(|certificate| certificate.clone().into_owned());
                let _ = sender
                    .send(Ok(TlsConnection {
                        stream,
                        peer: PeerIdentity {
                            remote,
                            certificate,
                        },
                    }))
                    .await;
            }
            Ok(Err(error)) => tracing::debug!(%remote, %error, "TLS handshake failed"),
            Err(_) => tracing::debug!(%remote, "TLS handshake timed out"),
        }
    });
}
