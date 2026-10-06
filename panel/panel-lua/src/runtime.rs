//! The VMs of a configuration and the runs of its handlers.

use crate::{
    exchange::{
        Exchange, Exit, Failure, FailureKind, Limits, LogEntry, LogLevel, Outcome, Permissions,
        Phase, Sockets,
    },
    program::{HandlerId, Program},
    shared::SharedStore,
    timer::{TimerReports, TimerRun, Timers},
    vm::{Cell, Exceeded, HostCall, HostReply, LightThread, Refused, Slot, ThreadState, Vm},
};
use async_trait::async_trait;
use bytes::Bytes;
use mlua::{MultiValue, Value};
use parking_lot::{Mutex, MutexGuard};
use std::{
    future::{poll_fn, Future},
    pin::pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

/// How a runtime's VMs are set up.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Settings {
    /// VMs to start: one per data-plane worker thread.
    pub vms: usize,
    /// Bytes each VM may allocate.
    pub memory: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            vms: std::thread::available_parallelism().map_or(1, usize::from),
            memory: 64 << 20,
        }
    }
}

/// The work the gateway does for scripts that wait on the request.
#[async_trait]
pub trait Host: Send {
    /// Reads the request body, refusing one of more than `limit` bytes.
    async fn read_body(&mut self, limit: usize) -> Result<Bytes, String>;

    /// Reads the next piece of the request body, `None` at its end, for
    /// `ngx.req.socket`.
    async fn read_body_chunk(&mut self) -> Result<Option<Bytes>, String> {
        Err("the request body cannot be streamed here".into())
    }

    /// Ends once the client has closed the connection; never where there
    /// is no client to watch.
    async fn closed(&mut self) {
        std::future::pending::<()>().await;
    }

    /// Makes the subrequests of `ngx.location.capture`, all at once, and
    /// gives their responses in order.
    async fn capture(
        &mut self,
        _requests: Vec<crate::capture::Capture>,
    ) -> Result<Vec<crate::capture::Captured>, String> {
        Err("subrequests cannot be made here".into())
    }
}

/// A host for runs that have no request to read from.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHost;

#[async_trait]
impl Host for NoHost {
    async fn read_body(&mut self, _limit: usize) -> Result<Bytes, String> {
        Err("the request body cannot be read here".into())
    }
}

/// A handler and the terms it runs on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Handler {
    pub id: HandlerId,
    pub phase: Phase,
    pub limits: Limits,
    pub permissions: Permissions,
    /// Messages less severe than this are dropped.
    pub log_level: LogLevel,
    pub sockets: Sockets,
    /// `ngx.header` turns underscores into hyphens, as
    /// `lua_transform_underscores_in_response_headers on` does.
    pub transform_underscores: bool,
    /// An answer without a `Content-Type` gets the default one, as
    /// `lua_use_default_type on` does.
    pub default_type: bool,
    /// The request body is read before a rewrite, access or content
    /// handler runs, as `lua_need_request_body on` does.
    pub read_body_first: bool,
    /// The run watches for the client closing the connection, as
    /// `lua_check_client_abort on` does.
    pub check_client_abort: bool,
}

impl Handler {
    /// `id` in `phase` on the default terms: no permissions beyond the
    /// default ones and messages from `notice` on.
    pub fn new(id: HandlerId, phase: Phase) -> Self {
        Self {
            id,
            phase,
            limits: Limits::default(),
            permissions: Permissions::default(),
            log_level: LogLevel::Notice,
            sockets: Sockets::default(),
            transform_underscores: true,
            default_type: true,
            read_body_first: false,
            check_client_abort: false,
        }
    }
}

/// The VMs of a configuration's scripts.
#[derive(Clone, Debug)]
pub struct Runtime {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    vms: Vec<Vm>,
    next: AtomicUsize,
    timers: Vec<Arc<Timers>>,
    reports: Arc<TimerReports>,
    /// The VMs of `ngx.run_worker_thread`, which hold the runtime's VMs
    /// only weakly.
    _workers: Arc<crate::worker::WorkerThreads>,
    /// Dropped with the runtime, which runs the pending timers at once.
    _closing: tokio::sync::watch::Sender<()>,
}

impl Drop for Inner {
    /// Runs each VM's `exit_worker_by_lua` before the pending timers are told
    /// to run at once; a test's runtime runs neither.
    fn drop(&mut self) {
        if self.timers.iter().any(|timers| timers.is_discarded()) {
            return;
        }
        for (index, vm) in self.vms.iter().enumerate() {
            if let Some(run) = vm.exit(index) {
                self.reports.send(run);
            }
        }
    }
}

impl Runtime {
    /// Starts `settings.vms` VMs for `program`, each running its
    /// `init_by_lua` and `init_worker_by_lua`. Returns what they logged.
    pub fn start(
        program: &Program,
        settings: &Settings,
        store: &SharedStore,
    ) -> Result<(Self, Vec<LogEntry>), Failure> {
        let dicts = store.resolve(&program.dicts);
        let (closing, closed) = tokio::sync::watch::channel(());
        let reports = Arc::new(TimerReports::default());
        let handle = tokio::runtime::Handle::try_current().ok();
        // Worker threads create no timers that would run.
        let idle_timers = Arc::new(Timers::new(
            0,
            (0, 0),
            closed.clone(),
            Arc::clone(&reports),
            None,
        ));
        idle_timers.discard();
        let workers = Arc::new(crate::worker::WorkerThreads::new(
            program,
            dicts.clone(),
            settings,
            idle_timers,
        ));
        let mut vms = Vec::with_capacity(settings.vms.max(1));
        let mut timers = Vec::with_capacity(settings.vms.max(1));
        let mut logs = Vec::new();
        for index in 0..settings.vms.max(1) {
            let vm_timers = Arc::new(Timers::new(
                index,
                (
                    match program.pending_timers {
                        0 => crate::timer::MOST_PENDING,
                        most => most,
                    },
                    match program.running_timers {
                        0 => crate::timer::MOST_RUNNING,
                        most => most,
                    },
                ),
                closed.clone(),
                Arc::clone(&reports),
                handle.clone(),
            ));
            timers.push(Arc::clone(&vm_timers));
            let started = Vm::new(
                program,
                &dicts,
                settings,
                index,
                vm_timers,
                Arc::downgrade(&workers),
                false,
            );
            let (vm, mut logged) = match started {
                Ok(started) => started,
                Err(error) => {
                    // The timers of a refused program never run.
                    timers.iter().for_each(|timers| timers.discard());
                    return Err(failure(&error));
                }
            };
            if index == 0 {
                logs.append(&mut logged);
            }
            vms.push(vm);
        }
        Ok((
            Self {
                inner: Arc::new(Inner {
                    vms,
                    next: AtomicUsize::new(0),
                    timers,
                    reports,
                    _workers: workers,
                    _closing: closing,
                }),
            },
            logs,
        ))
    }

    /// Hands what timers' callbacks did to `report`: first what they did
    /// so far, then each run as it ends.
    pub fn on_timer(&self, report: impl Fn(TimerRun) + Send + Sync + 'static) {
        self.inner.reports.attach(Arc::new(report));
    }

    /// Keeps the scripts from reaching outside, as for a test: timers never
    /// run, and the ones created from now on are logged instead, and
    /// cosockets are refused.
    pub fn isolate(&self) {
        for vm in &self.inner.vms {
            vm.slot.isolated.store(true, Ordering::Relaxed);
        }
        self.inner.timers.iter().for_each(|timers| timers.discard());
    }

    /// Scripts for a subrequest that `share` ties to the VM and `ngx.ctx` of
    /// the script that made it.
    pub fn scripts_sharing(&self, exchange: Exchange, share: &crate::capture::Share) -> Scripts {
        let exchange = Arc::new(Mutex::new(exchange));
        let cell = Arc::new(Cell::new(Arc::clone(&exchange)));
        *cell.ctx.lock() = Some(share.ctx.clone());
        Scripts {
            runtime: Arc::clone(&self.inner),
            exchange,
            bound: Some((share.vm.min(self.inner.vms.len() - 1), cell)),
        }
    }

    /// Scripts for one request, starting from `exchange`.
    pub fn scripts(&self, exchange: Exchange) -> Scripts {
        Scripts {
            runtime: Arc::clone(&self.inner),
            exchange: Arc::new(Mutex::new(exchange)),
            bound: None,
        }
    }

    /// Bytes each VM uses.
    pub fn memory(&self) -> Vec<usize> {
        self.inner
            .vms
            .iter()
            .map(|vm| vm.lua.used_memory())
            .collect()
    }
}

/// One request's runs. Its handlers all run on the VM the first one took,
/// so `ngx.ctx` and the request's globals stay with it.
#[derive(Debug)]
pub struct Scripts {
    runtime: Arc<Inner>,
    exchange: Arc<Mutex<Exchange>>,
    bound: Option<(usize, Arc<Cell>)>,
}

enum Step {
    Done(mlua::Result<Value>),
    Exited,
    Sliced,
    Serve(HostCall, tokio::sync::oneshot::Sender<HostReply>),
    /// The client closed the connection.
    Closed,
}

impl Scripts {
    /// What the handlers see and changed.
    pub fn exchange(&self) -> MutexGuard<'_, Exchange> {
        self.exchange.lock()
    }

    fn bind(&mut self) -> (usize, Arc<Cell>) {
        if let Some((vm, cell)) = &self.bound {
            return (*vm, Arc::clone(cell));
        }
        let vm = self.runtime.next.fetch_add(1, Ordering::Relaxed) % self.runtime.vms.len();
        let cell = Arc::new(Cell::new(Arc::clone(&self.exchange)));
        self.bound = Some((vm, Arc::clone(&cell)));
        (vm, cell)
    }

    /// Runs `handler`. A run that fails leaves the exchange as it was before
    /// it, but for what the script logged.
    pub async fn run(&mut self, handler: Handler, host: &mut (dyn Host + Send)) -> Outcome {
        if handler.read_body_first
            && matches!(
                handler.phase,
                Phase::ServerRewrite
                    | Phase::Rewrite
                    | Phase::Access
                    | Phase::Precontent
                    | Phase::Content
            )
            && self.exchange.lock().request.body.is_none()
        {
            match host.read_body(crate::api::MAX_BODY).await {
                Ok(body) => self.exchange.lock().request.body = Some(body),
                Err(error) => {
                    return Outcome::Failed(Failure {
                        kind: FailureKind::Error,
                        message: error,
                    })
                }
            }
        }
        let (index, cell) = self.bind();
        let runtime = Arc::clone(&self.runtime);
        let vm = &runtime.vms[index];
        let before = {
            let mut exchange = self.exchange.lock();
            exchange.begin(handler.phase);
            exchange.default_type = handler.default_type;
            exchange.clone()
        };
        {
            let mut run = cell.run.lock();
            *run = crate::vm::Run {
                limits: handler.limits,
                sockets: handler.sockets,
                keep_underscores: !handler.transform_underscores,
                permissions: handler.permissions,
                log_level: Some(handler.log_level),
                work_left: i64::try_from(handler.limits.work).unwrap_or(i64::MAX),
                deadline: vm.slot.after(handler.limits.time),
                check_abort: handler.check_client_abort && handler.phase.answers(),
                ..Default::default()
            };
        }
        let thread = match Self::entry(vm, handler.id, &cell) {
            Ok(thread) => thread,
            Err(error) => return self.fail(before, failure(&error)),
        };
        let key = thread.to_pointer() as usize;
        vm.slot.entries.lock().insert(key, Arc::clone(&cell));
        let result = tokio::time::timeout(
            handler.limits.time,
            drive(&vm.slot, &thread, &cell, host, MultiValue::new()),
        )
        .await;
        match settle(&vm.slot, key, &cell, result) {
            Ok(value) => {
                let mut exchange = self.exchange.lock();
                if handler.phase == Phase::Set {
                    match crate::api::set_value(&value) {
                        Ok(text) => exchange.value = Some(text),
                        Err(message) => {
                            drop(exchange);
                            return self.fail(
                                before,
                                Failure {
                                    kind: FailureKind::Error,
                                    message,
                                },
                            );
                        }
                    }
                }
                match exchange.exit {
                    Some(Exit::Exec) => Outcome::Continue,
                    Some(Exit::Abort) => Outcome::Abort,
                    Some(Exit::Respond) => Outcome::Respond,
                    Some(Exit::Phase) if handler.phase == Phase::Content => Outcome::Respond,
                    Some(Exit::Phase) => Outcome::Continue,
                    None if handler.phase == Phase::Content => Outcome::Respond,
                    None if handler.phase.answers() && exchange.headers_sent => Outcome::Respond,
                    None => Outcome::Continue,
                }
            }
            Err(failure) => self.fail(before, failure),
        }
    }

    /// Runs the `set_by_lua` handler of `variable` with `arguments` as
    /// `ngx.arg`, and sets the variable to what it returns.
    pub async fn set(
        &mut self,
        handler: Handler,
        host: &mut (dyn Host + Send),
        variable: &str,
        arguments: Vec<String>,
    ) -> Outcome {
        self.exchange.lock().arguments = arguments;
        let outcome = self.run(handler, host).await;
        let mut exchange = self.exchange.lock();
        exchange.arguments.clear();
        if let Some(value) = exchange.value.take() {
            if outcome == Outcome::Continue {
                exchange.variables.insert(variable.to_owned(), value);
            }
        }
        outcome
    }

    fn fail(&self, before: Exchange, failure: Failure) -> Outcome {
        let mut exchange = self.exchange.lock();
        let logs = std::mem::take(&mut exchange.logs);
        let dropped = exchange.dropped_logs;
        *exchange = before;
        exchange.logs = logs;
        exchange.dropped_logs = dropped;
        Outcome::Failed(failure)
    }

    fn entry(vm: &Vm, handler: HandlerId, cell: &Arc<Cell>) -> mlua::Result<mlua::Thread> {
        let env = {
            let mut env = cell.env.lock();
            match &*env {
                Some(env) => env.clone(),
                None => env.insert(vm.request_env()?).clone(),
            }
        };
        vm.entry(handler, &env)
    }
}

/// Drives a run's entry thread and the light threads it spawns until they
/// have all ended, a thread exits or the entry thread fails, serving the
/// host calls they make.
pub(crate) async fn drive(
    slot: &Slot,
    thread: &mlua::Thread,
    cell: &Arc<Cell>,
    host: &mut (dyn Host + Send),
    args: MultiValue,
) -> mlua::Result<Value> {
    let mut running = pin!(thread.clone().into_async::<Value>(args)?);
    let mut entry: Option<mlua::Result<Value>> = None;
    let mut watching = cell.run.lock().check_abort;
    loop {
        let mut closed = watching.then(|| host.closed());
        let step = poll_fn(|context| {
            if entry.is_none() {
                if let Poll::Ready(result) = running.as_mut().poll(context) {
                    entry = Some(result);
                }
            }
            if matches!(entry, Some(Err(_))) {
                return Poll::Ready(Step::Done(entry.take().unwrap_or(Ok(Value::Nil))));
            }
            let ended = poll_threads(slot, cell, context);
            if cell.exchange.lock().exit.is_some() {
                return Poll::Ready(Step::Exited);
            }
            if entry.is_some() && cell.threads.lock().waiting.is_empty() {
                return Poll::Ready(Step::Done(entry.take().unwrap_or(Ok(Value::Nil))));
            }
            if ended {
                // Threads waiting on the ones that ended go on.
                context.waker().wake_by_ref();
            }
            let mut run = cell.run.lock();
            if let Some((call, reply)) = run.call.take() {
                return Poll::Ready(Step::Serve(call, reply));
            }
            if std::mem::take(&mut run.sliced) {
                return Poll::Ready(Step::Sliced);
            }
            drop(run);
            if let Some(closed) = closed.as_mut() {
                if closed.as_mut().poll(context).is_ready() {
                    return Poll::Ready(Step::Closed);
                }
            }
            Poll::Pending
        })
        .await;
        drop(closed);
        match step {
            Step::Done(result) => return result,
            Step::Exited => return Ok(Value::Nil),
            Step::Sliced => tokio::task::yield_now().await,
            Step::Serve(call, reply) => {
                let answer = match call {
                    HostCall::ReadBody { limit } => HostReply::Body(host.read_body(limit).await),
                    HostCall::ReadBodyChunk => HostReply::Chunk(host.read_body_chunk().await),
                    HostCall::Capture(requests) => {
                        HostReply::Captured(host.capture(requests).await)
                    }
                };
                let _ = reply.send(answer);
            }
            Step::Closed => {
                watching = false;
                cell.exchange.lock().client_closed = true;
                let Some(callback) = cell.run.lock().on_abort.take() else {
                    // Without a callback the run stops, its threads with it.
                    cell.exchange.lock().exit = Some(Exit::Abort);
                    return Ok(Value::Nil);
                };
                let child = callback.to_pointer() as usize;
                let future = Box::pin(callback.into_async::<MultiValue>(())?);
                slot.entries.lock().insert(child, Arc::clone(cell));
                let mut threads = cell.threads.lock();
                threads.spawned.insert(
                    child,
                    LightThread {
                        parent: thread.to_pointer() as usize,
                        state: ThreadState::Running,
                    },
                );
                threads.waiting.push((child, future));
            }
        }
    }
}

/// Ends a run whose entry thread was `key`: drops its light threads and
/// tells how it went.
pub(crate) fn settle<T>(
    slot: &Slot,
    key: usize,
    cell: &Arc<Cell>,
    result: Result<mlua::Result<T>, tokio::time::error::Elapsed>,
) -> Result<T, Failure> {
    slot.entries.lock().remove(&key);
    end_threads(slot, cell);
    {
        let mut current = slot.current.lock();
        if current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, cell))
        {
            *current = None;
        }
    }
    let exceeded = cell.run.lock().exceeded;
    match (exceeded, result) {
        (Some(exceeded), _) => Err(exceeded_failure(exceeded)),
        (None, Err(_)) => Err(exceeded_failure(Exceeded::Time)),
        (None, Ok(Err(error))) => Err(failure(&error)),
        (None, Ok(Ok(value))) => Ok(value),
    }
}

/// Polls the light threads that wait. Returns whether one ended.
fn poll_threads(slot: &Slot, cell: &Arc<Cell>, context: &mut Context<'_>) -> bool {
    let mut waiting = std::mem::take(&mut cell.threads.lock().waiting);
    if waiting.is_empty() {
        return false;
    }
    let mut ended = false;
    let mut index = 0;
    while index < waiting.len() {
        let (key, future) = &mut waiting[index];
        let key = *key;
        if !is_running(cell, key) {
            drop(waiting.swap_remove(index));
            slot.entries.lock().remove(&key);
            continue;
        }
        match future.as_mut().poll(context) {
            Poll::Ready(result) => {
                drop(waiting.swap_remove(index));
                thread_ended(slot, cell, key, result);
                ended = true;
            }
            Poll::Pending => index += 1,
        }
    }
    // Threads killed while these were polled, and the ones spawned.
    waiting.retain(|(key, _)| is_running(cell, *key));
    let mut threads = cell.threads.lock();
    waiting.append(&mut threads.waiting);
    threads.waiting = waiting;
    ended
}

fn is_running(cell: &Cell, key: usize) -> bool {
    cell.threads
        .lock()
        .spawned
        .get(&key)
        .is_some_and(|thread| matches!(thread.state, ThreadState::Running))
}

/// Records how a light thread ended; one that failed is logged, as
/// lua-nginx-module does, and the run goes on.
pub(crate) fn thread_ended(slot: &Slot, cell: &Cell, key: usize, result: mlua::Result<MultiValue>) {
    slot.entries.lock().remove(&key);
    let result = result.map_err(|error| {
        let message = failure(&error).message;
        cell.exchange
            .lock()
            .log(LogLevel::Err, format!("lua user thread aborted: {message}"));
        message
    });
    if let Some(thread) = cell.threads.lock().spawned.get_mut(&key) {
        if matches!(thread.state, ThreadState::Running) {
            thread.state = ThreadState::Ended(result);
        }
    }
}

/// Drops the light threads a run left behind once it is over.
fn end_threads(slot: &Slot, cell: &Cell) {
    let threads = std::mem::take(&mut *cell.threads.lock());
    let mut entries = slot.entries.lock();
    for key in threads.spawned.keys() {
        entries.remove(key);
    }
    drop(entries);
    drop(threads);
}

pub(crate) fn exceeded_failure(exceeded: Exceeded) -> Failure {
    Failure {
        kind: match exceeded {
            Exceeded::Time => FailureKind::Timeout,
            Exceeded::Work => FailureKind::Work,
        },
        message: exceeded.to_string(),
    }
}

/// The failure an error from a run stands for.
pub(crate) fn failure(error: &mlua::Error) -> Failure {
    let mut cause = error;
    loop {
        match cause {
            mlua::Error::CallbackError { cause: inner, .. } => cause = inner,
            mlua::Error::WithContext { cause: inner, .. } => cause = inner,
            _ => break,
        }
    }
    if let Some(exceeded) = cause.downcast_ref::<Exceeded>() {
        return exceeded_failure(*exceeded);
    }
    let kind = if cause.downcast_ref::<Refused>().is_some() {
        FailureKind::Refused
    } else if matches!(cause, mlua::Error::MemoryError(_)) {
        FailureKind::Memory
    } else {
        FailureKind::Error
    };
    let message = match cause {
        mlua::Error::RuntimeError(message) | mlua::Error::MemoryError(message) => message.clone(),
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        other => other.to_string(),
    };
    let message = message.lines().next().unwrap_or_default().trim().to_owned();
    Failure { kind, message }
}
