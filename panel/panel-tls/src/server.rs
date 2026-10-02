use crate::TlsCredentials;
use rustls_pki_types::CertificateDer;
use std::{
    io,
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

/// The authenticated peer of a connection.
#[derive(Clone, Debug)]
pub struct PeerIdentity {
    pub(crate) remote: SocketAddr,
    pub(crate) certificate: Option<CertificateDer<'static>>,
}

impl PeerIdentity {
    pub fn remote(&self) -> SocketAddr {
        self.remote
    }

    /// The peer's end-entity certificate, verified against the trust bundle
    /// during the handshake.
    pub fn certificate(&self) -> Option<&CertificateDer<'static>> {
        self.certificate.as_ref()
    }
}

/// An established server-side TLS connection.
pub struct TlsConnection {
    stream: TlsStream<TcpStream>,
    peer: PeerIdentity,
}

impl Connected for TlsConnection {
    type ConnectInfo = PeerIdentity;

    fn connect_info(&self) -> PeerIdentity {
        self.peer.clone()
    }
}

impl AsyncRead for TlsConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for TlsConnection {
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
            let acceptor = TlsAcceptor::from(credentials.server_config());
            let sender = sender.clone();
            tokio::spawn(async move {
                let _ = tcp.set_nodelay(true);
                match tokio::time::timeout(handshake_timeout, acceptor.accept(tcp)).await {
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
    });
    ReceiverStream::new(receiver)
}
