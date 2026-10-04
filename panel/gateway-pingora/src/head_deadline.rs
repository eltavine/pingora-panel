//! A deadline for request heads, so a client cannot hold a connection by
//! sending its request a byte at a time (Slowloris). Pingora bounds each
//! read; this bounds the whole head, counted from the connection's start
//! for its first request and from the end of the previous request for later
//! ones, which also bounds how long a kept-alive connection idles. A first
//! head that is late gets 408; a later one closes the connection, as it may
//! also be the rest of a body the previous request left unread. The HTTP/2
//! connection preface ends the watch, as HTTP/2 frames are not request
//! heads.

use async_trait::async_trait;
use pingora_core::{
    apps::ServerApp,
    protocols::{
        raw_connect::ProxyDigest,
        tls::{SslDigest, TlsRef},
        GetProxyDigest, GetSocketDigest, GetTimingDigest, Peek, Shutdown, SocketDigest, Ssl,
        Stream, TimingDigest, UniqueID, UniqueIDType, ALPN,
    },
    server::ShutdownWatch,
};
use prometheus_client::metrics::gauge::Gauge;
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
        Arc, Mutex, PoisonError,
    },
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    time::Sleep,
};

/// How long a client may take to send a request head when its listener does
/// not say.
pub(crate) const DEFAULT_HEAD_TIMEOUT: Duration = Duration::from_secs(30);

/// RFC 9110 §15.5.9, sent when part of a head arrived but not all of it.
const TIMED_OUT: &[u8] =
    b"HTTP/1.1 408 Request Timeout\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

const SHARDS: usize = 32;

/// The watched connections of a listener, by their socket, so the proxy can
/// tell one that a request on it started or is done.
#[derive(Default)]
pub(crate) struct Connections {
    shards: [Mutex<HashMap<usize, Arc<Watch>>>; SHARDS],
    /// Counts the open connections, when the gateway is measured.
    open: Option<Gauge>,
    /// What the listener's generation owes before it may close: a count each
    /// connection holds until its first request starts.
    owed: Arc<AtomicUsize>,
}

impl Connections {
    pub(crate) fn counted(open: Option<Gauge>, owed: Arc<AtomicUsize>) -> Self {
        Self {
            open,
            owed,
            ..Self::default()
        }
    }

    fn shard(&self, key: usize) -> &Mutex<HashMap<usize, Arc<Watch>>> {
        &self.shards[(key >> 4) % SHARDS]
    }

    fn watch(&self, key: usize, watch: Arc<Watch>) {
        let replaced = self
            .shard(key)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, watch);
        if let (None, Some(open)) = (replaced, &self.open) {
            open.inc();
        }
    }

    fn forget(&self, key: usize) {
        let removed = self
            .shard(key)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
        if let (Some(_), Some(open)) = (removed, &self.open) {
            open.dec();
        }
    }

    fn find(&self, socket: &Arc<SocketDigest>) -> Option<Arc<Watch>> {
        let key = Arc::as_ptr(socket) as usize;
        self.shard(key)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
    }

    /// Releases what the connection of `socket` owed its generation: from
    /// here on its request counts as in flight itself.
    pub(crate) fn request_started(&self, socket: &Arc<SocketDigest>) {
        if let Some(watch) = self.find(socket) {
            watch.settle();
        }
    }

    /// Starts the deadline for the next request head on the connection of
    /// `socket`.
    pub(crate) fn request_done(&self, socket: &Arc<SocketDigest>) {
        if let Some(watch) = self.find(socket) {
            watch.due.store(true, Relaxed);
        }
    }
}

/// What a listener knows of one of its connections.
pub(crate) struct Watch {
    /// Set when a request is done and the next head is due.
    due: AtomicBool,
    /// Whether the connection still holds its generation open, which it
    /// does from its acceptance until its first request starts, so that a
    /// generation being replaced serves what it accepted.
    owing: AtomicBool,
    owed: Arc<AtomicUsize>,
}

impl Watch {
    fn new(owed: &Arc<AtomicUsize>) -> Self {
        owed.fetch_add(1, Relaxed);
        Self {
            due: AtomicBool::new(false),
            owing: AtomicBool::new(true),
            owed: Arc::clone(owed),
        }
    }

    fn settle(&self) {
        if self.owing.swap(false, Relaxed) {
            self.owed.fetch_sub(1, Relaxed);
        }
    }
}

/// Wraps an application so that every connection it serves is watched.
pub(crate) struct HeadDeadline<A> {
    app: Arc<A>,
    timeout: Duration,
    connections: Arc<Connections>,
}

impl<A> HeadDeadline<A> {
    pub(crate) fn new(app: A, timeout: Duration, connections: Arc<Connections>) -> Self {
        Self {
            app: Arc::new(app),
            timeout,
            connections,
        }
    }
}

#[async_trait]
impl<A: ServerApp + Send + Sync + 'static> ServerApp for HeadDeadline<A> {
    async fn process_new(
        self: &Arc<Self>,
        stream: Stream,
        shutdown: &ShutdownWatch,
    ) -> Option<Stream> {
        let watched = if stream.as_any().is::<Watched>() {
            let mut watched = stream
                .into_any()
                .downcast::<Watched>()
                .expect("the stream was checked to be watched");
            watched.await_next_head();
            watched
        } else {
            Box::new(Watched::new(
                stream,
                self.timeout,
                Arc::clone(&self.connections),
            ))
        };
        self.app.process_new(watched, shutdown).await
    }

    async fn cleanup(&self) {
        self.app.cleanup().await;
    }
}

enum State {
    /// The head must be complete by the deadline.
    Reading(Pin<Box<Sleep>>),
    /// Answering a head that did not arrive in time.
    Refusing { written: usize },
    /// The head arrived; reads pass through.
    Arrived,
}

/// A connection whose request heads are watched.
pub(crate) struct Watched {
    inner: Stream,
    timeout: Duration,
    state: State,
    /// The last four bytes read, to find the empty line ending a head.
    tail: u32,
    /// Whether any byte of the current head arrived.
    started: bool,
    /// Bytes read by peeking and not yet read.
    peeked: Vec<u8>,
    /// Whether a late head is answered, which only the first one is.
    answer: bool,
    watch: Arc<Watch>,
    /// Where the connection is registered, by its socket.
    registration: Option<(Arc<Connections>, usize)>,
}

impl Watched {
    fn new(inner: Stream, timeout: Duration, connections: Arc<Connections>) -> Self {
        let watch = Arc::new(Watch::new(&connections.owed));
        let registration = inner.get_socket_digest().map(|socket| {
            let key = Arc::as_ptr(&socket) as usize;
            connections.watch(key, Arc::clone(&watch));
            (connections, key)
        });
        let mut watched = Self {
            inner,
            timeout,
            state: State::Arrived,
            tail: 0,
            started: false,
            peeked: Vec::new(),
            answer: true,
            watch,
            registration,
        };
        watched.await_next_head();
        watched
    }

    fn await_next_head(&mut self) {
        self.state = State::Reading(Box::pin(tokio::time::sleep(self.timeout)));
        self.tail = 0;
        self.started = false;
    }

    /// Follows the bytes of a head until the empty line that ends it.
    fn scan(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.started = true;
            self.tail = (self.tail << 8) | u32::from(byte);
            if self.tail == u32::from_be_bytes(*b"\r\n\r\n") || self.tail & 0xffff == 0x0a0a {
                self.state = State::Arrived;
                return;
            }
        }
    }

    fn timed_out() -> io::Error {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "the request head did not arrive in time",
        )
    }

    /// Reads from the connection while the deadline holds.
    fn poll_watched(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if matches!(self.state, State::Arrived) && self.watch.due.swap(false, Relaxed) {
            self.answer = false;
            self.await_next_head();
        }
        loop {
            match &mut self.state {
                State::Arrived => return Pin::new(&mut self.inner).poll_read(cx, buf),
                State::Refusing { written } => {
                    while self.answer && self.started && *written < TIMED_OUT.len() {
                        let rest = &TIMED_OUT[*written..];
                        match ready!(Pin::new(&mut self.inner).poll_write(cx, rest)) {
                            Ok(0) | Err(_) => *written = TIMED_OUT.len(),
                            Ok(sent) => *written += sent,
                        }
                    }
                    let _ = ready!(Pin::new(&mut self.inner).poll_flush(cx));
                    return Poll::Ready(Err(Self::timed_out()));
                }
                State::Reading(deadline) => {
                    if deadline.as_mut().poll(cx).is_ready() {
                        self.state = State::Refusing { written: 0 };
                        continue;
                    }
                    let before = buf.filled().len();
                    let read = ready!(Pin::new(&mut self.inner).poll_read(cx, buf));
                    if read.is_ok() {
                        let arrived = buf.filled()[before..].to_vec();
                        self.scan(&arrived);
                    }
                    return Poll::Ready(read);
                }
            }
        }
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        self.watch.settle();
        if let Some((connections, key)) = self.registration.take() {
            connections.forget(key);
        }
    }
}

impl fmt::Debug for Watched {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Watched")
            .field("inner", &self.inner)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl AsyncRead for Watched {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.peeked.is_empty() {
            return this.poll_watched(cx, buf);
        }
        let served = this.peeked.len().min(buf.remaining());
        buf.put_slice(&this.peeked[..served]);
        this.peeked.drain(..served);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Watched {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[async_trait]
impl Shutdown for Watched {
    async fn shutdown(&mut self) {
        self.inner.shutdown().await;
    }
}

impl UniqueID for Watched {
    fn id(&self) -> UniqueIDType {
        self.inner.id()
    }
}

impl Ssl for Watched {
    fn get_ssl(&self) -> Option<&TlsRef> {
        self.inner.get_ssl()
    }

    fn get_ssl_digest(&self) -> Option<Arc<SslDigest>> {
        self.inner.get_ssl_digest()
    }

    fn selected_alpn_proto(&self) -> Option<ALPN> {
        self.inner.selected_alpn_proto()
    }
}

impl GetTimingDigest for Watched {
    fn get_timing_digest(&self) -> Vec<Option<TimingDigest>> {
        self.inner.get_timing_digest()
    }

    fn get_read_pending_time(&self) -> Duration {
        self.inner.get_read_pending_time()
    }

    fn get_write_pending_time(&self) -> Duration {
        self.inner.get_write_pending_time()
    }
}

impl GetProxyDigest for Watched {
    fn get_proxy_digest(&self) -> Option<Arc<ProxyDigest>> {
        self.inner.get_proxy_digest()
    }

    fn set_proxy_digest(&mut self, digest: ProxyDigest) {
        self.inner.set_proxy_digest(digest);
    }
}

impl GetSocketDigest for Watched {
    fn get_socket_digest(&self) -> Option<Arc<SocketDigest>> {
        self.inner.get_socket_digest()
    }

    fn set_socket_digest(&mut self, digest: SocketDigest) {
        self.inner.set_socket_digest(digest);
    }
}

#[async_trait]
impl Peek for Watched {
    /// Peeks through the watched reads, so the deadline holds while a head's
    /// first bytes are awaited, and what is peeked is read again later.
    async fn try_peek(&mut self, buf: &mut [u8]) -> io::Result<bool> {
        while self.peeked.len() < buf.len() {
            let mut chunk = vec![0; buf.len() - self.peeked.len()];
            let read = std::future::poll_fn(|cx| {
                let mut chunk = ReadBuf::new(&mut chunk);
                ready!(self.poll_watched(cx, &mut chunk))?;
                Poll::Ready(Ok::<_, io::Error>(chunk.filled().len()))
            })
            .await?;
            if read == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.peeked.extend_from_slice(&chunk[..read]);
        }
        buf.copy_from_slice(&self.peeked[..buf.len()]);
        Ok(true)
    }
}
