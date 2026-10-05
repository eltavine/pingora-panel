//! The VMs of a configuration and the runs of its handlers.

use crate::{
    exchange::{
        Exchange, Exit, Failure, FailureKind, Limits, LogEntry, LogLevel, Outcome, Permissions,
        Phase,
    },
    program::{HandlerId, Program},
    shared::SharedStore,
    vm::{Cell, Exceeded, HostCall, HostReply, Refused, Vm},
};
use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::{Mutex, MutexGuard};
use std::{
    future::{poll_fn, Future},
    pin::pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::Poll,
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
pub struct Handler {
    pub id: HandlerId,
    pub phase: Phase,
    pub limits: Limits,
    pub permissions: Permissions,
    /// Messages less severe than this are dropped.
    pub log_level: LogLevel,
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
        let mut vms = Vec::with_capacity(settings.vms.max(1));
        let mut logs = Vec::new();
        for index in 0..settings.vms.max(1) {
            let (vm, mut logged) =
                Vm::new(program, &dicts, settings, index).map_err(|error| failure(&error))?;
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
                }),
            },
            logs,
        ))
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
    Done(mlua::Result<()>),
    Exited,
    Sliced,
    Serve(HostCall, tokio::sync::oneshot::Sender<HostReply>),
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
        let (index, cell) = self.bind();
        let runtime = Arc::clone(&self.runtime);
        let vm = &runtime.vms[index];
        let before = {
            let mut exchange = self.exchange.lock();
            exchange.begin(handler.phase);
            exchange.clone()
        };
        {
            let mut run = cell.run.lock();
            *run = crate::vm::Run {
                permissions: handler.permissions,
                log_level: Some(handler.log_level),
                work_left: i64::try_from(handler.limits.work).unwrap_or(i64::MAX),
                deadline: vm.slot.after(handler.limits.time),
                ..Default::default()
            };
        }
        let thread = match Self::entry(vm, handler.id, &cell) {
            Ok(thread) => thread,
            Err(error) => return self.fail(before, failure(&error)),
        };
        let key = thread.to_pointer() as usize;
        vm.slot.entries.lock().insert(key, Arc::clone(&cell));
        let result =
            tokio::time::timeout(handler.limits.time, Self::drive(&thread, &cell, host)).await;
        vm.slot.entries.lock().remove(&key);
        {
            let mut current = vm.slot.current.lock();
            if current
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &cell))
            {
                *current = None;
            }
        }
        let exceeded = cell.run.lock().exceeded;
        let outcome = match (exceeded, result) {
            (Some(exceeded), _) => Err(exceeded_failure(exceeded)),
            (None, Err(_)) => Err(exceeded_failure(Exceeded::Time)),
            (None, Ok(Err(error))) => Err(failure(&error)),
            (None, Ok(Ok(()))) => Ok(()),
        };
        match outcome {
            Ok(()) => {
                let exchange = self.exchange.lock();
                match exchange.exit {
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

    async fn drive(
        thread: &mlua::Thread,
        cell: &Arc<Cell>,
        host: &mut (dyn Host + Send),
    ) -> mlua::Result<()> {
        let mut running = pin!(thread.clone().into_async::<()>(())?);
        loop {
            let step = poll_fn(|context| match running.as_mut().poll(context) {
                Poll::Ready(result) => Poll::Ready(Step::Done(result)),
                Poll::Pending => {
                    if cell.exchange.lock().exit.is_some() {
                        return Poll::Ready(Step::Exited);
                    }
                    let mut run = cell.run.lock();
                    if let Some((call, reply)) = run.call.take() {
                        return Poll::Ready(Step::Serve(call, reply));
                    }
                    if std::mem::take(&mut run.sliced) {
                        return Poll::Ready(Step::Sliced);
                    }
                    Poll::Pending
                }
            })
            .await;
            match step {
                Step::Done(result) => return result,
                Step::Exited => return Ok(()),
                Step::Sliced => tokio::task::yield_now().await,
                Step::Serve(call, reply) => {
                    let answer = match call {
                        HostCall::ReadBody { limit } => {
                            HostReply::Body(host.read_body(limit).await)
                        }
                    };
                    let _ = reply.send(answer);
                }
            }
        }
    }
}

fn exceeded_failure(exceeded: Exceeded) -> Failure {
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
