//! `ngx.socket.tcp`: cosockets, connections a script opens to other
//! services with lua-nginx-module's methods, granted by the network
//! permission. Waits count against the run's time limit; idle connections
//! kept with `setkeepalive` are reused by later requests of the same VM.
//! TLS verifies the certificate unless the script says `ssl_verify` false.

use super::{cell, require_permission, results, Api};
use crate::vm::{refused, Slot};
use bytes::BytesMut;
use mlua::{
    AnyUserData, Lua, LuaString, MultiValue, Table, UserData, UserDataMethods, UserDataRefMut,
    Value,
};
use parking_lot::Mutex;
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::CryptoProvider,
    pki_types::{CertificateDer, ServerName, UnixTime},
    ClientConfig, DigitallySignedStruct, SignatureScheme,
};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{client::TlsStream, TlsConnector};

/// lua-nginx-module's default for connecting, sending and reading.
pub(super) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_IDLE: Duration = Duration::from_secs(60);
const DEFAULT_POOL_SIZE: usize = 30;
/// The most a single receive holds, as the VM's memory limit does not see it.
const MOST_RECEIVED: usize = 16 << 20;
const READ_SIZE: usize = 16 << 10;

enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl Stream {
    async fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.write_all(data).await,
            Self::Tls(stream) => {
                stream.write_all(data).await?;
                stream.flush().await
            }
        }
    }

    async fn read_into(&mut self, buffer: &mut BytesMut) -> std::io::Result<usize> {
        buffer.reserve(READ_SIZE);
        match self {
            Self::Plain(stream) => stream.read_buf(buffer).await,
            Self::Tls(stream) => stream.read_buf(buffer).await,
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

/// Accepts any certificate chain but still checks the handshake's
/// signatures, for `ssl_verify = false`.
#[derive(Debug)]
struct Unverified(Arc<CryptoProvider>);

impl ServerCertVerifier for Unverified {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// TLS settings for connections that verify certificates against the
/// system's trusted roots, or that do not.
fn tls(verify: bool) -> Result<Arc<ClientConfig>, String> {
    static VERIFYING: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    static TRUSTING: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    let build = move || -> Result<Arc<ClientConfig>, String> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let versions = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .map_err(|error| error.to_string())?;
        let config = if verify {
            use rustls_platform_verifier::BuilderVerifierExt;
            versions
                .with_platform_verifier()
                .map_err(|error| format!("the system's trusted roots: {error}"))?
                .with_no_client_auth()
        } else {
            versions
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(Unverified(provider)))
                .with_no_client_auth()
        };
        Ok(Arc::new(config))
    };
    if verify {
        VERIFYING.get_or_init(build).clone()
    } else {
        TRUSTING.get_or_init(build).clone()
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
    reused: u32,
}

impl TcpSocket {
    pub(crate) fn new(slot: Arc<Slot>, pool: Arc<Pool>) -> Self {
        Self {
            slot,
            pool,
            stream: None,
            key: String::new(),
            host: String::new(),
            buffer: BytesMut::new(),
            connect_timeout: DEFAULT_TIMEOUT,
            send_timeout: DEFAULT_TIMEOUT,
            read_timeout: DEFAULT_TIMEOUT,
            reused: 0,
        }
    }

    fn allowed(&self) -> mlua::Result<()> {
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
        let read = within(self.read_timeout, stream.read_into(&mut self.buffer)).await?;
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
        match within(
            self.connect_timeout,
            TcpStream::connect((host.as_str(), port)),
        )
        .await
        {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                self.stream = Some(Stream::Plain(stream));
                self.reused = 0;
                Ok(results([Value::Integer(1)]))
            }
            Err(error) => failed(lua, &error),
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
            None => return failed(lua, "closed"),
        };
        let config = match tls(verify) {
            Ok(config) => config,
            Err(error) => return failed(lua, &error),
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
            Err(error) => failed(lua, &format!("handshake failed: {error}")),
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
                failed(lua, &error)
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
                this.handshake(&lua, server_name, verify != Some(false))
                    .await
            },
        );
        methods.add_async_method_mut("send", |lua, mut this, data: Value| async move {
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
                let Some(stream) = this.stream.take() else {
                    return failed(lua, "closed");
                };
                if !this.buffer.is_empty() {
                    this.buffer.clear();
                    return failed(lua, "unread data in buffer");
                }
                let idle = milliseconds(idle).unwrap_or(DEFAULT_IDLE);
                let size = size.filter(|size| *size > 0).unwrap_or(DEFAULT_POOL_SIZE);
                this.pool
                    .put(this.key.clone(), stream, idle, size, this.reused);
                Ok(results([Value::Integer(1)]))
            },
        );
        methods.add_method("getreusedtimes", |_, this, ()| Ok(this.reused));
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
    Ok(())
}
