//! gRPC between the modules of one process over in-memory streams, so no
//! network listener carries it (ADR 0032).

use hyper_util::rt::TokioIo;
use panel_platform::ServiceName;
use std::{
    collections::{HashMap, HashSet},
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf},
    sync::mpsc,
};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tonic::{
    codegen::http::Uri,
    transport::{server::Connected, Channel, Endpoint},
};

/// How many bytes a connection buffers in each direction.
const BUFFER: usize = 64 * 1024;
/// Connections a module has been offered but not yet taken up.
const BACKLOG: usize = 64;

/// The modules a process hosts, and how to reach the gRPC services each one
/// serves.
#[derive(Clone)]
pub struct InProcessHub {
    inner: Arc<Inner>,
}

struct Inner {
    hosted: HashSet<String>,
    listeners: Mutex<HashMap<String, mpsc::Sender<InProcessStream>>>,
}

impl InProcessHub {
    /// A hub for the modules named; each serves through it once started.
    pub fn new(hosted: impl IntoIterator<Item = ServiceName>) -> Self {
        Self {
            inner: Arc::new(Inner {
                hosted: hosted
                    .into_iter()
                    .map(|service| service.as_str().to_owned())
                    .collect(),
                listeners: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Whether `service` is one of the modules this process hosts.
    pub fn hosts(&self, service: &ServiceName) -> bool {
        self.inner.hosted.contains(service.as_str())
    }

    /// A channel to `service`. It connects on first use, so a module can hold
    /// one before its peer has started.
    pub fn channel(&self, service: &ServiceName) -> Channel {
        let inner = Arc::clone(&self.inner);
        let service = service.as_str().to_owned();
        Endpoint::from_static("http://in-process.invalid").connect_with_connector_lazy(
            tower::service_fn(move |_: Uri| {
                let inner = Arc::clone(&inner);
                let service = service.clone();
                async move {
                    let listener = inner
                        .listeners
                        .lock()
                        .map_err(|_| refused(&service, "has no usable listener"))?
                        .get(&service)
                        .cloned()
                        .ok_or_else(|| refused(&service, "is not serving"))?;
                    let (client, server) = tokio::io::duplex(BUFFER);
                    listener
                        .send(InProcessStream(server))
                        .await
                        .map_err(|_| refused(&service, "has stopped"))?;
                    Ok::<_, io::Error>(TokioIo::new(client))
                }
            }),
        )
    }

    /// The connections other modules open to `service`, for its server.
    pub(crate) fn listen(
        &self,
        service: &ServiceName,
    ) -> impl Stream<Item = Result<InProcessStream, io::Error>> + use<> {
        let (sender, receiver) = mpsc::channel(BACKLOG);
        if let Ok(mut listeners) = self.inner.listeners.lock() {
            listeners.insert(service.as_str().to_owned(), sender);
        }
        ReceiverStream::new(receiver).map(Ok)
    }

    /// Stops offering connections to `service`.
    pub(crate) fn close(&self, service: &ServiceName) {
        if let Ok(mut listeners) = self.inner.listeners.lock() {
            listeners.remove(service.as_str());
        }
    }
}

fn refused(service: &str, why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionRefused, format!("{service} {why}"))
}

/// The server's end of an in-process connection.
pub(crate) struct InProcessStream(DuplexStream);

impl Connected for InProcessStream {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

impl AsyncRead for InProcessStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(context, buffer)
    }
}

impl AsyncWrite for InProcessStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(context)
    }
}
