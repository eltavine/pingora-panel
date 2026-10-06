//! `ngx.socket.tcp`: cosockets, connections a script opens to other
//! services with lua-nginx-module's methods, granted by the network
//! permission. Waits count against the run's time limit; idle connections
//! kept with `setkeepalive` are reused by later requests of the same VM.
//! TLS verifies the certificate unless the script says `ssl_verify` false.

use super::{
    cell, require_permission, results,
    ssl::{Chain, Key},
    Api,
};
use crate::{
    exchange::{LogLevel, Sockets},
    vm::{refused, HostCall, HostReply, Slot},
};
use bytes::{Bytes, BytesMut};
use mlua::{
    AnyUserData, Lua, LuaString, MultiValue, Table, UserData, UserDataMethods, UserDataRef,
    UserDataRefMut, Value,
};
use parking_lot::Mutex;
use rustls::{
    client::Resumption,
    pki_types::{CertificateDer, PrivateKeyDer, ServerName},
    sign::{CertifiedKey, SingleCertAndKey},
};
use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpSocket as LocalSocket, TcpStream},
    sync::oneshot,
};
use tokio_rustls::{client::TlsStream, TlsConnector};

/// lua-nginx-module's default for connecting, sending and reading.
/// The most a single receive holds, as the VM's memory limit does not see it.
const MOST_RECEIVED: usize = 16 << 20;

/// The cosocket defaults of the run under way.
pub(super) fn defaults(slot: &Slot) -> Sockets {
    slot.cell()
        .map(|cell| cell.run.lock().sockets)
        .unwrap_or_default()
}

/// Writes a cosocket's failure to the error log, as
/// `lua_socket_log_errors on` does.
pub(super) fn log_failure(slot: &Slot, kind: &str, action: &str, error: &str) {
    let Some(cell) = slot.cell() else {
        return;
    };
    if cell.run.lock().sockets.log_errors {
        cell.exchange.lock().log(
            LogLevel::Err,
            format!("lua {kind} socket {action} failed: {error}"),
        );
    }
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
    /// The body of the request, which the host hands over piece by piece.
    Request(Arc<Slot>),
    /// The client's connection, once the response header went out.
    Raw(Arc<Slot>),
}

/// Makes `call` on the host of the run under way and waits for its reply.
async fn host_call(slot: &Slot, call: HostCall) -> std::io::Result<HostReply> {
    let Some(cell) = slot.cell() else {
        return Err(std::io::Error::other("closed"));
    };
    let (reply, answer) = oneshot::channel();
    cell.run.lock().call = Some((call, reply));
    answer
        .await
        .map_err(|_| std::io::Error::other("the client connection could not be used"))
}

impl Stream {
    async fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.write_all(data).await,
            Self::Tls(stream) => {
                stream.write_all(data).await?;
                stream.flush().await
            }
            Self::Request(_) => Err(std::io::Error::other("the request socket is read-only")),
            Self::Raw(slot) => {
                let data = Bytes::copy_from_slice(data);
                match host_call(slot, HostCall::WriteRaw(data)).await? {
                    HostReply::Written(Ok(())) => Ok(()),
                    HostReply::Written(Err(error)) => Err(std::io::Error::other(error)),
                    _ => Err(std::io::Error::other(
                        "the client connection could not be used",
                    )),
                }
            }
        }
    }

    async fn read_into(&mut self, buffer: &mut BytesMut, size: usize) -> std::io::Result<usize> {
        buffer.reserve(size);
        match self {
            Self::Plain(stream) => stream.read_buf(buffer).await,
            Self::Tls(stream) => stream.read_buf(buffer).await,
            Self::Request(slot) => {
                let Some(cell) = slot.cell() else {
                    return Err(std::io::Error::other("closed"));
                };
                let (reply, answer) = oneshot::channel();
                cell.run.lock().call = Some((HostCall::ReadBodyChunk, reply));
                match answer.await {
                    Ok(HostReply::Chunk(Ok(Some(chunk)))) => {
                        buffer.extend_from_slice(&chunk);
                        Ok(chunk.len())
                    }
                    Ok(HostReply::Chunk(Ok(None))) => Ok(0),
                    Ok(HostReply::Chunk(Err(error))) => Err(std::io::Error::other(error)),
                    _ => Err(std::io::Error::other("the request body could not be read")),
                }
            }
            Self::Raw(slot) => match host_call(slot, HostCall::ReadRaw).await? {
                HostReply::Chunk(Ok(Some(chunk))) => {
                    buffer.extend_from_slice(&chunk);
                    Ok(chunk.len())
                }
                HostReply::Chunk(Ok(None)) => Ok(0),
                HostReply::Chunk(Err(error)) => Err(std::io::Error::other(error)),
                _ => Err(std::io::Error::other(
                    "the client connection could not be used",
                )),
            },
        }
    }
}

struct Idle {
    stream: Stream,
    until: Instant,
    reused: u32,
}

/// Connections kept for later requests of one VM, by pool name.
#[derive(Default)]
pub(crate) struct Pool {
    idle: Mutex<HashMap<String, Vec<Idle>>>,
}

impl Pool {
    fn take(&self, key: &str) -> Option<(Stream, u32)> {
        let mut idle = self.idle.lock();
        let kept = idle.get_mut(key)?;
        let now = Instant::now();
        kept.retain(|connection| connection.until > now);
        let connection = kept.pop()?;
        Some((connection.stream, connection.reused + 1))
    }

    fn put(&self, key: String, stream: Stream, idle_for: Duration, size: usize, reused: u32) {
        let mut idle = self.idle.lock();
        let kept = idle.entry(key).or_default();
        let now = Instant::now();
        kept.retain(|connection| connection.until > now);
        if kept.len() < size {
            kept.push(Idle {
                stream,
                until: now + idle_for,
                reused,
            });
        }
    }
}

/// An I/O error as lua-nginx-module words it.
pub(super) fn reason(error: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionRefused => "connection refused".into(),
        ErrorKind::ConnectionReset => "connection reset by peer".into(),
        ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof => "closed".into(),
        ErrorKind::TimedOut => "timeout".into(),
        _ => error.to_string(),
    }
}

pub(super) async fn within<T>(
    limit: Duration,
    work: impl Future<Output = std::io::Result<T>>,
) -> Result<T, String> {
    match tokio::time::timeout(limit, work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(reason(&error)),
        Err(_) => Err("timeout".into()),
    }
}

pub(super) fn failed(lua: &Lua, error: &str) -> mlua::Result<MultiValue> {
    Ok(results([
        Value::Nil,
        Value::String(lua.create_string(error)?),
    ]))
}

fn partial(lua: &Lua, error: &str, data: &[u8]) -> mlua::Result<MultiValue> {
    Ok(results([
        Value::Nil,
        Value::String(lua.create_string(error)?),
        Value::String(lua.create_string(data)?),
    ]))
}

/// A connection of a script, and what was read past what it asked for.
pub(crate) struct TcpSocket {
    slot: Arc<Slot>,
    pool: Arc<Pool>,
    stream: Option<Stream>,
    key: String,
    host: String,
    buffer: BytesMut,
    connect_timeout: Duration,
    send_timeout: Duration,
    read_timeout: Duration,
    read_size: usize,
    reused: u32,
    /// `ngx.req.socket`: reads the request body and nothing else.
    request: bool,
    /// `ngx.req.socket(true)`: the client's connection, which it neither
    /// connects nor keeps.
    raw: bool,
    /// The local address `bind` gave connections.
    local: Option<IpAddr>,
    tls: Option<crate::tls::TlsId>,
    /// The certificate `setclientcert` gave handshakes to present.
    client: Option<Arc<CertifiedKey>>,
}

impl TcpSocket {
    pub(crate) fn new(slot: Arc<Slot>, pool: Arc<Pool>) -> Self {
        let sockets = defaults(&slot);
        Self {
            slot,
            pool,
            stream: None,
            key: String::new(),
            host: String::new(),
            buffer: BytesMut::new(),
            connect_timeout: sockets.connect_timeout,
            send_timeout: sockets.send_timeout,
            read_timeout: sockets.read_timeout,
            read_size: sockets.buffer_size.max(1024),
            reused: 0,
            request: false,
            raw: false,
            local: None,
            tls: sockets.tls,
            client: None,
        }
    }

    /// The request body of the run under way as a read-only cosocket.
    fn request(slot: Arc<Slot>, pool: Arc<Pool>) -> Self {
        let mut socket = Self::new(Arc::clone(&slot), pool);
        socket.stream = Some(Stream::Request(slot));
        socket.request = true;
        socket
    }

    /// The client's connection as a full-duplex cosocket.
    fn raw(slot: Arc<Slot>, pool: Arc<Pool>) -> Self {
        let mut socket = Self::new(Arc::clone(&slot), pool);
        socket.stream = Some(Stream::Raw(slot));
        socket.raw = true;
        socket
    }

    /// Refuses what the request socket does not do.
    fn writable(&self, lua: &Lua) -> Option<mlua::Result<MultiValue>> {
        self.request
            .then(|| failed(lua, "not supported on the request socket"))
    }

    /// Refuses what the request sockets do not do with their connection.
    fn own_connection(&self, lua: &Lua) -> Option<mlua::Result<MultiValue>> {
        (self.request || self.raw).then(|| failed(lua, "not supported on the request socket"))
    }

    fn failed(&self, lua: &Lua, action: &str, error: &str) -> mlua::Result<MultiValue> {
        log_failure(&self.slot, "tcp", action, error);
        failed(lua, error)
    }

    fn allowed(&self) -> mlua::Result<()> {
        if self.request || self.raw {
            let cell = cell(&self.slot, Api::ReqSocket)?;
            return require_permission(&cell, Api::ReqSocket, |granted| granted.body, "body");
        }
        allowed(&self.slot, Api::Socket)
    }

    /// Reads once more into the buffer; false at the end of the stream.
    async fn fill(&mut self) -> Result<bool, String> {
        let Some(stream) = self.stream.as_mut() else {
            return Err("closed".into());
        };
        if self.buffer.len() >= MOST_RECEIVED {
            return Err(format!("more than {MOST_RECEIVED} bytes arrived at once"));
        }
        let read = within(
            self.read_timeout,
            stream.read_into(&mut self.buffer, self.read_size),
        )
        .await?;
        Ok(read > 0)
    }

    async fn connect(
        &mut self,
        lua: &Lua,
        host: String,
        port: u16,
        pool: Option<String>,
    ) -> mlua::Result<MultiValue> {
        self.allowed()?;
        self.stream = None;
        self.buffer.clear();
        self.key = pool.unwrap_or_else(|| format!("{host}:{port}"));
        self.host = host.clone();
        if let Some((stream, reused)) = self.pool.take(&self.key) {
            self.stream = Some(stream);
            self.reused = reused;
            return Ok(results([Value::Integer(1)]));
        }
        let local = self.local;
        let connecting = async move {
            let Some(local) = local else {
                return TcpStream::connect((host.as_str(), port)).await;
            };
            let target = tokio::net::lookup_host((host.as_str(), port))
                .await?
                .find(|target| target.is_ipv4() == local.is_ipv4())
                .ok_or_else(|| {
                    std::io::Error::other("no address of the host is of the bound address's family")
                })?;
            let socket = if local.is_ipv4() {
                LocalSocket::new_v4()?
            } else {
                LocalSocket::new_v6()?
            };
            socket.bind(SocketAddr::new(local, 0))?;
            socket.connect(target).await
        };
        match within(self.connect_timeout, connecting).await {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                self.stream = Some(Stream::Plain(stream));
                self.reused = 0;
                Ok(results([Value::Integer(1)]))
            }
            Err(error) => self.failed(lua, "connect", &error),
        }
    }

    async fn handshake(
        &mut self,
        lua: &Lua,
        server_name: Option<String>,
        verify: bool,
    ) -> mlua::Result<MultiValue> {
        self.allowed()?;
        let plain = match self.stream.take() {
            Some(Stream::Plain(stream)) => stream,
            Some(tls @ Stream::Tls(_)) => {
                self.stream = Some(tls);
                return Ok(results([Value::Boolean(true)]));
            }
            Some(request @ (Stream::Request(_) | Stream::Raw(_))) => {
                self.stream = Some(request);
                return failed(lua, "not supported on the request socket");
            }
            None => return failed(lua, "closed"),
        };
        let config = match self.tls {
            Some(id) => lua
                .app_data_ref::<crate::tls::TlsTable>()
                .and_then(|table| {
                    table
                        .0
                        .get(id.0 as usize)
                        .map(|configs| configs.get(verify))
                })
                .ok_or_else(|| "the TLS terms of this handler are not held".to_owned()),
            None => crate::tls::system(verify),
        };
        let config = match config {
            Ok(config) => config,
            Err(error) => return failed(lua, &error),
        };
        let config = match &self.client {
            Some(certified) => {
                let mut presenting = (*config).clone();
                presenting.client_auth_cert_resolver =
                    Arc::new(SingleCertAndKey::from(Arc::clone(certified)));
                // A session resumed from the shared cache would carry the
                // identity another handshake presented.
                presenting.resumption = Resumption::disabled();
                Arc::new(presenting)
            }
            None => config,
        };
        let name = server_name.unwrap_or_else(|| self.host.clone());
        let Ok(name) = ServerName::try_from(name) else {
            return failed(lua, "the server name is not a host name or an IP address");
        };
        let connecting = TlsConnector::from(config).connect(name, plain);
        match within(self.connect_timeout, connecting).await {
            Ok(stream) => {
                self.stream = Some(Stream::Tls(Box::new(stream)));
                Ok(results([Value::Boolean(true)]))
            }
            Err(error) => self.failed(lua, "handshake", &format!("handshake failed: {error}")),
        }
    }

    async fn send(&mut self, lua: &Lua, data: Vec<u8>) -> mlua::Result<MultiValue> {
        self.allowed()?;
        let Some(stream) = self.stream.as_mut() else {
            return failed(lua, "closed");
        };
        match within(self.send_timeout, stream.write_all(&data)).await {
            Ok(()) => Ok(results([Value::Integer(
                i64::try_from(data.len()).unwrap_or(i64::MAX),
            )])),
            Err(error) => {
                self.stream = None;
                self.failed(lua, "send", &error)
            }
        }
    }

    /// `*l`: a line without its line break; `*a`: everything until the
    /// peer closes; a number: exactly that many bytes.
    async fn receive(&mut self, lua: &Lua, pattern: Value) -> mlua::Result<MultiValue> {
        self.allowed()?;
        match pattern {
            Value::Integer(_) | Value::Number(_) => {
                let wanted = match pattern {
                    Value::Integer(count) => usize::try_from(count).unwrap_or(0),
                    Value::Number(count) => count.max(0.0) as usize,
                    _ => 0,
                };
                while self.buffer.len() < wanted {
                    match self.fill().await {
                        Ok(true) => {}
                        Ok(false) => {
                            let data = self.buffer.split();
                            return partial(lua, "closed", &data);
                        }
                        Err(error) => {
                            let data = self.buffer.split();
                            log_failure(&self.slot, "tcp", "receive", &error);
                            return partial(lua, &error, &data);
                        }
                    }
                }
                let data = self.buffer.split_to(wanted);
                Ok(results([Value::String(lua.create_string(&data)?)]))
            }
            Value::String(pattern) if matches!(pattern.as_bytes().as_ref(), b"*a" | b"a") => loop {
                match self.fill().await {
                    Ok(true) => {}
                    Ok(false) => {
                        let data = self.buffer.split();
                        return Ok(results([Value::String(lua.create_string(&data)?)]));
                    }
                    Err(error) => {
                        let data = self.buffer.split();
                        log_failure(&self.slot, "tcp", "receive", &error);
                        return partial(lua, &error, &data);
                    }
                }
            },
            Value::Nil => self.line(lua).await,
            Value::String(pattern) if matches!(pattern.as_bytes().as_ref(), b"*l" | b"l") => {
                self.line(lua).await
            }
            _ => Err(mlua::Error::runtime(
                "bad argument #1 to 'receive' (bad pattern argument)",
            )),
        }
    }

    async fn line(&mut self, lua: &Lua) -> mlua::Result<MultiValue> {
        loop {
            if let Some(end) = self.buffer.iter().position(|&byte| byte == b'\n') {
                let mut line = self.buffer.split_to(end + 1);
                line.truncate(end);
                if line.last() == Some(&b'\r') {
                    line.truncate(end - 1);
                }
                return Ok(results([Value::String(lua.create_string(&line)?)]));
            }
            match self.fill().await {
                Ok(true) => {}
                Ok(false) => {
                    let data = self.buffer.split();
                    return partial(lua, "closed", &data);
                }
                Err(error) => {
                    let data = self.buffer.split();
                    log_failure(&self.slot, "tcp", "receive", &error);
                    return partial(lua, &error, &data);
                }
            }
        }
    }

    async fn receive_any(&mut self, lua: &Lua, most: usize) -> mlua::Result<MultiValue> {
        self.allowed()?;
        if self.buffer.is_empty() {
            match self.fill().await {
                Ok(true) => {}
                Ok(false) => return failed(lua, "closed"),
                Err(error) => return failed(lua, &error),
            }
        }
        let take = most.min(self.buffer.len());
        let data = self.buffer.split_to(take);
        Ok(results([Value::String(lua.create_string(&data)?)]))
    }

    /// The data before the next `pattern`, or at most `size` bytes of it at
    /// a time with nil once the pattern is reached.
    async fn until(
        &mut self,
        lua: &Lua,
        pattern: &[u8],
        size: Option<usize>,
        inclusive: bool,
        reached: &mut bool,
    ) -> mlua::Result<MultiValue> {
        self.allowed()?;
        if *reached {
            *reached = false;
            return Ok(results([Value::Nil]));
        }
        loop {
            let found = self
                .buffer
                .windows(pattern.len().max(1))
                .position(|window| window == pattern);
            if let Some(at) = found {
                if let Some(size) = size.filter(|size| at > *size) {
                    let data = self.buffer.split_to(size);
                    return Ok(results([Value::String(lua.create_string(&data)?)]));
                }
                let mut data = self.buffer.split_to(at + pattern.len());
                if !inclusive {
                    data.truncate(at);
                }
                if size.is_some() {
                    *reached = true;
                }
                return Ok(results([Value::String(lua.create_string(&data)?)]));
            }
            if let Some(size) = size {
                let safe = self.buffer.len().saturating_sub(pattern.len());
                if safe >= size {
                    let data = self.buffer.split_to(size);
                    return Ok(results([Value::String(lua.create_string(&data)?)]));
                }
            }
            match self.fill().await {
                Ok(true) => {}
                Ok(false) => {
                    let data = self.buffer.split();
                    return partial(lua, "closed", &data);
                }
                Err(error) => {
                    let data = self.buffer.split();
                    log_failure(&self.slot, "tcp", "receive", &error);
                    return partial(lua, &error, &data);
                }
            }
        }
    }
}

pub(super) fn milliseconds(value: Option<f64>) -> Option<Duration> {
    value
        .filter(|ms| ms.is_finite() && *ms >= 0.0)
        .map(|ms| match ms {
            0.0 => Duration::from_secs(u64::MAX / 4),
            ms => Duration::from_secs_f64(ms / 1000.0),
        })
}

/// Checks the phase and the network permission of a cosocket's call.
pub(super) fn allowed(slot: &Slot, api: Api) -> mlua::Result<()> {
    let cell = cell(slot, api)?;
    require_permission(&cell, api, |granted| granted.network, "network")?;
    if slot.isolated.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(refused("connections are not opened in a test"));
    }
    Ok(())
}

/// What `send` takes: a string, a number, or an array of them.
pub(super) fn payload(value: Value) -> mlua::Result<Vec<u8>> {
    let mut data = Vec::new();
    fn append(data: &mut Vec<u8>, value: &Value) -> mlua::Result<()> {
        match value {
            Value::String(text) => data.extend_from_slice(&text.as_bytes()),
            Value::Integer(number) => data.extend_from_slice(number.to_string().as_bytes()),
            Value::Number(number) => data.extend_from_slice(super::number_text(*number).as_bytes()),
            Value::Boolean(flag) => data.extend_from_slice(flag.to_string().as_bytes()),
            Value::Table(items) => {
                for item in items.sequence_values::<Value>() {
                    append(data, &item?)?;
                }
            }
            _ => {
                return Err(mlua::Error::runtime(
                    "bad argument #1 to 'send' (bad data type)",
                ))
            }
        }
        Ok(())
    }
    append(&mut data, &value)?;
    Ok(data)
}

impl UserData for TcpSocket {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method_mut(
            "connect",
            |lua, mut this, (host, port, options): (String, Option<u16>, Option<Table>)| async move {
                let Some(port) = port else {
                    return failed(&lua, "unix domain sockets are not available");
                };
                if let Some(refused) = this.own_connection(&lua) {
                    return refused;
                }
                let pool = options
                    .map(|options| options.get::<Option<String>>("pool"))
                    .transpose()?
                    .flatten();
                this.connect(&lua, host, port, pool).await
            },
        );
        methods.add_async_method_mut(
            "sslhandshake",
            |lua,
             mut this,
             (_reused, server_name, verify): (Value, Option<String>, Option<bool>)| async move {
                if let Some(refused) = this.own_connection(&lua) {
                    return refused;
                }
                this.handshake(&lua, server_name, verify != Some(false))
                    .await
            },
        );
        methods.add_method_mut(
            "setclientcert",
            |lua, this, (chain, key): (Option<UserDataRef<Chain>>, Option<UserDataRef<Key>>)| {
                if let Some(refused) = this.own_connection(lua) {
                    return refused;
                }
                this.client =
                    match (chain, key) {
                        (None, None) => None,
                        (Some(chain), Some(key)) => match certified(&chain.0, &key.0) {
                            Ok(certified) => Some(Arc::new(certified)),
                            Err(error) => return failed(lua, &error),
                        },
                        _ => return failed(
                            lua,
                            "client certificate must be supplied with corresponding private key",
                        ),
                    };
                Ok(results([Value::Boolean(true)]))
            },
        );
        methods.add_async_method_mut("send", |lua, mut this, data: Value| async move {
            if let Some(refused) = this.writable(&lua) {
                return refused;
            }
            let data = payload(data)?;
            this.send(&lua, data).await
        });
        methods.add_async_method_mut("receive", |lua, mut this, pattern: Value| async move {
            this.receive(&lua, pattern).await
        });
        methods.add_async_method_mut("receiveany", |lua, mut this, most: usize| async move {
            if most == 0 {
                return Err(mlua::Error::runtime(
                    "bad argument #1 to 'receiveany' (bad max argument)",
                ));
            }
            this.receive_any(&lua, most).await
        });
        methods.add_function(
            "receiveuntil",
            |lua, (socket, pattern, options): (AnyUserData, LuaString, Option<Table>)| {
                let pattern = pattern.as_bytes().to_vec();
                if pattern.is_empty() {
                    return Err(mlua::Error::runtime(
                        "bad argument #1 to 'receiveuntil' (pattern is empty)",
                    ));
                }
                let inclusive = options
                    .map(|options| options.get::<Option<bool>>("inclusive"))
                    .transpose()?
                    .flatten()
                    .unwrap_or(false);
                let reached = Arc::new(Mutex::new(false));
                lua.create_async_function(move |lua, size: Option<usize>| {
                    let (socket, pattern, reached) =
                        (socket.clone(), pattern.clone(), Arc::clone(&reached));
                    async move {
                        let mut this: UserDataRefMut<TcpSocket> = socket.borrow_mut()?;
                        let mut done = *reached.lock();
                        let result = this.until(&lua, &pattern, size, inclusive, &mut done).await;
                        *reached.lock() = done;
                        result
                    }
                })
            },
        );
        methods.add_method_mut("settimeout", |_, this, ms: Option<f64>| {
            if let Some(limit) = milliseconds(ms) {
                this.connect_timeout = limit;
                this.send_timeout = limit;
                this.read_timeout = limit;
            }
            Ok(())
        });
        methods.add_method_mut(
            "settimeouts",
            |_, this, (connect, send, read): (Option<f64>, Option<f64>, Option<f64>)| {
                if let Some(limit) = milliseconds(connect) {
                    this.connect_timeout = limit;
                }
                if let Some(limit) = milliseconds(send) {
                    this.send_timeout = limit;
                }
                if let Some(limit) = milliseconds(read) {
                    this.read_timeout = limit;
                }
                Ok(())
            },
        );
        methods.add_method_mut(
            "setkeepalive",
            |lua, this, (idle, size): (Option<f64>, Option<usize>)| {
                if let Some(refused) = this.own_connection(lua) {
                    return refused;
                }
                let Some(stream) = this.stream.take() else {
                    return failed(lua, "closed");
                };
                if !this.buffer.is_empty() {
                    this.buffer.clear();
                    return failed(lua, "unread data in buffer");
                }
                let sockets = defaults(&this.slot);
                let idle = milliseconds(idle).unwrap_or(sockets.keepalive_timeout);
                let size = size.filter(|size| *size > 0).unwrap_or(sockets.pool_size);
                this.pool
                    .put(this.key.clone(), stream, idle, size, this.reused);
                Ok(results([Value::Integer(1)]))
            },
        );
        methods.add_method("getreusedtimes", |_, this, ()| Ok(this.reused));
        methods.add_method_mut("bind", |lua, this, address: String| {
            if let Some(refused) = this.own_connection(lua) {
                return refused;
            }
            match address.parse::<IpAddr>() {
                Ok(local) => {
                    this.local = Some(local);
                    Ok(results([Value::Integer(1)]))
                }
                Err(_) => failed(lua, "bad address"),
            }
        });
        methods.add_method("getfd", |lua, this, ()| {
            let fd = match &this.stream {
                Some(Stream::Plain(stream)) => descriptor(stream),
                Some(Stream::Tls(stream)) => descriptor(stream.get_ref().0),
                Some(Stream::Request(_) | Stream::Raw(_)) => {
                    return failed(lua, "not supported on the request socket")
                }
                None => return failed(lua, "closed"),
            };
            match fd {
                Some(fd) => Ok(results([Value::Integer(fd)])),
                None => failed(lua, "file descriptors are not available on this system"),
            }
        });
        methods.add_method_mut("close", |lua, this, ()| {
            if this.stream.take().is_none() {
                return failed(lua, "closed");
            }
            this.buffer.clear();
            Ok(results([Value::Integer(1)]))
        });
        methods.add_method("setoption", |_, _, _: MultiValue| Ok(1));
    }
}

/// The certificate chain and key `setclientcert` was given, as handshakes
/// present them.
fn certified(chain: &[Vec<u8>], key: &[u8]) -> Result<CertifiedKey, String> {
    let key = PrivateKeyDer::try_from(key)
        .map_err(|error| format!("the private key cannot be read: {error}"))?
        .clone_key();
    let chain = chain
        .iter()
        .map(|der| CertificateDer::from(der.clone()))
        .collect();
    CertifiedKey::from_der(chain, key, &rustls::crypto::ring::default_provider())
        .map_err(|error| format!("the client certificate and its key cannot be used: {error}"))
}

/// The number of the descriptor `stream` holds, where descriptors are
/// numbers.
fn descriptor(stream: &TcpStream) -> Option<i64> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        Some(i64::from(stream.as_raw_fd()))
    }
    #[cfg(not(unix))]
    {
        let _ = stream;
        None
    }
}

/// Installs `ngx.socket.tcp` and `ngx.socket.connect`.
pub(super) fn install(
    lua: &Lua,
    ngx: &Table,
    slot: &Arc<Slot>,
    pool: &Arc<Pool>,
) -> mlua::Result<()> {
    let socket: Table = ngx.raw_get("socket")?;
    let (create_slot, create_pool) = (Arc::clone(slot), Arc::clone(pool));
    socket.raw_set(
        "tcp",
        lua.create_function(move |lua, ()| {
            lua.create_userdata(TcpSocket::new(
                Arc::clone(&create_slot),
                Arc::clone(&create_pool),
            ))
        })?,
    )?;
    let (connect_slot, connect_pool) = (Arc::clone(slot), Arc::clone(pool));
    socket.raw_set(
        "connect",
        lua.create_async_function(move |lua, (host, port): (String, Option<u16>)| {
            let socket = TcpSocket::new(Arc::clone(&connect_slot), Arc::clone(&connect_pool));
            async move {
                let Some(port) = port else {
                    return failed(&lua, "unix domain sockets are not available");
                };
                let userdata = lua.create_userdata(socket)?;
                let connected = {
                    let mut socket: UserDataRefMut<TcpSocket> = userdata.borrow_mut()?;
                    socket.connect(&lua, host, port, None).await?
                };
                if connected.front().is_some_and(Value::is_nil) {
                    return Ok(connected);
                }
                Ok(results([Value::UserData(userdata)]))
            }
        })?,
    )?;
    let req: Table = ngx.raw_get("req")?;
    let (request_slot, request_pool) = (Arc::clone(slot), Arc::clone(pool));
    req.raw_set(
        "socket",
        lua.create_function(move |lua, raw: Option<bool>| {
            let cell = cell(&request_slot, Api::ReqSocket)?;
            require_permission(&cell, Api::ReqSocket, |granted| granted.body, "body")?;
            if raw == Some(true) {
                let streams = cell.run.lock().streams;
                let (streaming, pending) = {
                    let exchange = cell.exchange.lock();
                    (exchange.streaming(), !exchange.response.body.is_empty())
                };
                if !streams {
                    return failed(lua, "the raw request socket is not available here");
                }
                if pending {
                    return failed(lua, "pending data to write");
                }
                if !streaming {
                    return failed(
                        lua,
                        "the raw request socket follows a response header the gateway sent: call ngx.send_headers() and ngx.flush(true) first",
                    );
                }
                let socket = TcpSocket::raw(Arc::clone(&request_slot), Arc::clone(&request_pool));
                return Ok(results([Value::UserData(lua.create_userdata(socket)?)]));
            }
            if cell.exchange.lock().request.body.is_some() {
                return failed(lua, "request body already exists");
            }
            let socket = TcpSocket::request(Arc::clone(&request_slot), Arc::clone(&request_pool));
            Ok(results([Value::UserData(lua.create_userdata(socket)?)]))
        })?,
    )?;
    Ok(())
}
