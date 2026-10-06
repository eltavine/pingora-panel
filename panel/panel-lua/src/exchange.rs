//! What a handler sees of the request it runs for and what it changes: plain
//! data the gateway fills in before a run and reads back after it.

use bytes::Bytes;
use http::HeaderMap;
use std::{collections::HashMap, net::SocketAddr, time::Duration, time::SystemTime};

/// The phases handlers run in, named as `ngx.get_phase()` names them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Phase {
    Init,
    InitWorker,
    ServerRewrite,
    Rewrite,
    Access,
    /// After access and before content: `precontent_by_lua`.
    Precontent,
    Content,
    Balancer,
    HeaderFilter,
    BodyFilter,
    Log,
    Timer,
    /// When a VM stops: `exit_worker_by_lua`.
    ExitWorker,
}

impl Phase {
    pub const ALL: [Phase; 13] = [
        Phase::Init,
        Phase::InitWorker,
        Phase::ServerRewrite,
        Phase::Rewrite,
        Phase::Access,
        Phase::Precontent,
        Phase::Content,
        Phase::Balancer,
        Phase::HeaderFilter,
        Phase::BodyFilter,
        Phase::Log,
        Phase::Timer,
        Phase::ExitWorker,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Phase::Init => "init",
            Phase::InitWorker => "init_worker",
            Phase::ServerRewrite => "server_rewrite",
            Phase::Rewrite => "rewrite",
            Phase::Access => "access",
            Phase::Precontent => "precontent",
            Phase::Content => "content",
            Phase::Balancer => "balancer",
            Phase::HeaderFilter => "header_filter",
            Phase::BodyFilter => "body_filter",
            Phase::Log => "log",
            Phase::Timer => "timer",
            Phase::ExitWorker => "exit_worker",
        }
    }

    pub(crate) const fn bit(self) -> u16 {
        1 << self as u16
    }

    /// Whether a handler of this phase may answer the request itself.
    pub(crate) const fn answers(self) -> bool {
        matches!(
            self,
            Phase::ServerRewrite
                | Phase::Rewrite
                | Phase::Access
                | Phase::Precontent
                | Phase::Content
        )
    }
}

/// The request line and header as the handler sees and changes them.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// `$uri`: the decoded and normalized path.
    pub uri: String,
    /// `$request_uri`: the path and query as the client sent them.
    pub request_uri: String,
    /// `$args`: the query, without `?`.
    pub args: Option<String>,
    pub version: http::Version,
    pub headers: HeaderMap,
    /// The body, once it has been read.
    pub body: Option<Bytes>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            method: "GET".into(),
            uri: "/".into(),
            request_uri: "/".into(),
            args: None,
            version: http::Version::HTTP_11,
            headers: HeaderMap::new(),
            body: None,
        }
    }
}

/// The response: the one the handler answers with, or the upstream's in the
/// header and body filters.
#[derive(Clone, Debug, Default)]
pub struct Response {
    /// Zero until a status is set.
    pub status: u16,
    pub headers: HeaderMap,
    /// What the handler printed, sent as the body when it answers.
    pub body: Vec<u8>,
}

/// The connection and the request's identity.
#[derive(Clone, Debug)]
pub struct Connection {
    /// The client, after trusted proxies.
    pub client: Option<SocketAddr>,
    pub server: Option<SocketAddr>,
    pub tls: bool,
    pub server_name: String,
    pub request_id: String,
    pub started: SystemTime,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            client: None,
            server: None,
            tls: false,
            server_name: String::new(),
            request_id: String::new(),
            started: SystemTime::now(),
        }
    }
}

/// A piece of the response body in `body_filter_by_lua`: `ngx.arg[1]` and
/// `ngx.arg[2]`.
#[derive(Clone, Debug, Default)]
pub struct Chunk {
    pub data: Bytes,
    pub eof: bool,
}

/// An upstream endpoint chosen by `balancer_by_lua`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Peer {
    pub host: String,
    pub port: u16,
}

/// Timeouts `ngx.balancer.set_timeouts` sets for the current try.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PeerTimeouts {
    pub connect: Option<Duration>,
    pub send: Option<Duration>,
    pub read: Option<Duration>,
}

/// What `balancer_by_lua` chooses from and what it chose.
#[derive(Clone, Debug, Default)]
pub struct Balancer {
    pub upstream: String,
    /// Tries made before this one.
    pub tries: u32,
    /// How the previous try ended, `failed` or `next`, and its status.
    pub last_failure: Option<(String, Option<u16>)>,
    pub peer: Option<Peer>,
    pub more_tries: Option<u32>,
    pub timeouts: PeerTimeouts,
}

/// Which parts of the exchange a handler changed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Changes {
    pub method: bool,
    pub uri: bool,
    /// `ngx.req.set_uri(uri, true)`: the route is chosen again.
    pub jump: bool,
    pub args: bool,
    pub headers: bool,
    pub body: bool,
    pub status: bool,
    pub response_headers: bool,
    pub chunk: bool,
    pub peer: bool,
}

impl Changes {
    pub fn any(self) -> bool {
        self != Self::default()
    }
}

/// `ngx.log` levels, most severe first, with lua-nginx-module's numbers.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LogLevel {
    Stderr = 0,
    Emerg = 1,
    Alert = 2,
    Crit = 3,
    Err = 4,
    Warn = 5,
    Notice = 6,
    Info = 7,
    Debug = 8,
}

impl LogLevel {
    pub const ALL: [LogLevel; 9] = [
        LogLevel::Stderr,
        LogLevel::Emerg,
        LogLevel::Alert,
        LogLevel::Crit,
        LogLevel::Err,
        LogLevel::Warn,
        LogLevel::Notice,
        LogLevel::Info,
        LogLevel::Debug,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            LogLevel::Stderr => "stderr",
            LogLevel::Emerg => "emerg",
            LogLevel::Alert => "alert",
            LogLevel::Crit => "crit",
            LogLevel::Err => "error",
            LogLevel::Warn => "warn",
            LogLevel::Notice => "notice",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }

    pub fn from_number(number: i64) -> Option<Self> {
        Self::ALL.get(usize::try_from(number).ok()?).copied()
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.name() == name)
    }
}

/// A message a script logged, with where it logged it from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
}

/// Messages a run keeps; later ones are dropped and counted.
pub(crate) const MAX_LOG_ENTRIES: usize = 256;
/// Bytes of one message; longer ones are cut.
pub(crate) const MAX_LOG_MESSAGE: usize = 4096;

/// How a handler ended its phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Exit {
    /// `ngx.exit(ngx.OK)`: the phase ends and the request goes on.
    Phase,
    /// The handler answers with the exchange's response.
    Respond,
    /// `ngx.exit(ngx.ERROR)` or 444: the connection closes without an answer.
    Abort,
    /// `ngx.exec`: the request is handled again with its new URI.
    Exec,
}

/// Everything a handler run reads and changes.
#[derive(Clone, Debug)]
pub struct Exchange {
    pub phase: Phase,
    pub request: Request,
    pub response: Response,
    pub connection: Connection,
    /// Variables the gateway knows, such as `status` or `upstream_addr`, and
    /// the ones scripts set.
    pub variables: HashMap<String, String>,
    pub chunk: Chunk,
    pub balancer: Balancer,
    /// What scripts logged, for the gateway to write out.
    pub logs: Vec<LogEntry>,
    /// Messages dropped once `logs` was full.
    pub dropped_logs: usize,
    pub(crate) changes: Changes,
    pub(crate) exit: Option<Exit>,
    /// The response header was sent by `ngx.say`, `ngx.flush` or
    /// `ngx.send_headers`.
    pub(crate) headers_sent: bool,
    /// The body `ngx.req.init_body` started and `ngx.req.finish_body`
    /// will make the request's.
    pub(crate) new_body: Option<Vec<u8>>,
    /// `ngx.exec` redirected the request internally.
    pub(crate) internal: bool,
    /// What the last run's handler says of `lua_use_default_type`.
    pub(crate) default_type: bool,
}

impl Exchange {
    pub fn new(request: Request, connection: Connection) -> Self {
        Self {
            phase: Phase::Rewrite,
            request,
            response: Response::default(),
            connection,
            variables: HashMap::new(),
            chunk: Chunk::default(),
            balancer: Balancer::default(),
            logs: Vec::new(),
            dropped_logs: 0,
            changes: Changes::default(),
            exit: None,
            headers_sent: false,
            new_body: None,
            internal: false,
            default_type: true,
        }
    }

    /// What the last run changed.
    pub fn changes(&self) -> Changes {
        self.changes
    }

    /// Whether an answer without a `Content-Type` gets the default one, as
    /// `lua_use_default_type on` has it.
    pub fn default_type(&self) -> bool {
        self.default_type
    }

    /// The last run called `ngx.exec`: the request is to be handled again,
    /// from `server_rewrite`, with the URI and arguments it now has.
    pub fn redirected(&self) -> bool {
        matches!(self.exit, Some(Exit::Exec))
    }

    /// Whether a handler has started sending the response.
    pub fn headers_sent(&self) -> bool {
        self.headers_sent
    }

    pub(crate) fn log(&mut self, level: LogLevel, mut message: String) {
        if self.logs.len() >= MAX_LOG_ENTRIES {
            self.dropped_logs += 1;
            return;
        }
        if message.len() > MAX_LOG_MESSAGE {
            let mut end = MAX_LOG_MESSAGE;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        self.logs.push(LogEntry { level, message });
    }

    /// Forgets what was changed, once the gateway has applied it.
    pub fn clear_changes(&mut self) {
        self.changes = Changes::default();
    }

    pub(crate) fn begin(&mut self, phase: Phase) {
        self.phase = phase;
        self.changes = Changes::default();
        self.exit = None;
        if phase == Phase::BodyFilter || phase == Phase::HeaderFilter || phase == Phase::Log {
            self.headers_sent = true;
        }
    }
}

/// How a run ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The request goes on to its next phase.
    Continue,
    /// Answer with the exchange's response and process nothing more.
    Respond,
    /// Close the connection without answering.
    Abort,
    /// The handler failed and changed nothing; its fallback decides.
    Failed(Failure),
}

/// Why a run failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum FailureKind {
    /// The script raised an error.
    Error,
    /// The run took longer than its time limit.
    Timeout,
    /// The run used up its work limit.
    Work,
    /// The VM's memory limit was reached.
    Memory,
    /// The script called a function it has not been granted or that the
    /// phase does not allow.
    Refused,
}

impl FailureKind {
    pub const fn name(self) -> &'static str {
        match self {
            FailureKind::Error => "error",
            FailureKind::Timeout => "timeout",
            FailureKind::Work => "work",
            FailureKind::Memory => "memory",
            FailureKind::Refused => "refused",
        }
    }
}

/// The defaults of the cosockets a run opens, as the `lua_socket_*`
/// directives set them; a script's own `settimeout` and `setkeepalive`
/// arguments come first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Sockets {
    pub connect_timeout: Duration,
    pub send_timeout: Duration,
    pub read_timeout: Duration,
    /// Bytes a read takes from a connection at a time.
    pub buffer_size: usize,
    /// Idle connections `setkeepalive` keeps for a pool.
    pub pool_size: usize,
    /// How long `setkeepalive` keeps an idle connection.
    pub keepalive_timeout: Duration,
    /// Failures are written to the error log.
    pub log_errors: bool,
}

impl Default for Sockets {
    /// lua-nginx-module's defaults, but for reads of 16 KiB at a time.
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(60),
            send_timeout: Duration::from_secs(60),
            read_timeout: Duration::from_secs(60),
            buffer_size: 16 << 10,
            pool_size: 30,
            keepalive_timeout: Duration::from_secs(60),
            log_errors: true,
        }
    }
}

/// Limits on one run of a handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Wall-clock time, waits included.
    pub time: Duration,
    /// Interrupt checks: function calls and loop iterations.
    pub work: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            time: Duration::from_millis(100),
            work: 10_000_000,
        }
    }
}

/// What scripts may do beyond reading and changing the request and response.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Permissions {
    /// Read and replace request and response bodies.
    pub body: bool,
    /// Choose upstream endpoints.
    pub upstream: bool,
    /// Open sockets.
    pub network: bool,
}
