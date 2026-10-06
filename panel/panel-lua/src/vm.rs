//! A Luau VM with the sandbox, limits and API of ADR 0039.

use crate::{
    api,
    exchange::{Exchange, Limits, LogLevel, Permissions, Phase, Sockets},
    program::{HandlerId, Program},
    runtime::Settings,
    shared::Dict,
    timer::Timers,
};
use bytes::Bytes;
use mlua::{
    chunk::ChunkMode,
    thread::{AsyncThread, ThreadEvent, ThreadTriggers},
    Function, Lua, LuaOptions, MultiValue, StdLib, Table, Thread, VmState,
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    fmt,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering::Relaxed},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// How long a run keeps its thread before it is suspended for other tasks.
pub(crate) const SLICE: Duration = Duration::from_millis(1);
/// Interrupt checks between two readings of the clock.
const CLOCK_EVERY: i64 = 1024;

/// A limit a run went over, raised from the VM's interrupt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Exceeded {
    Time,
    Work,
}

impl fmt::Display for Exceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Exceeded::Time => "the handler ran longer than its time limit",
            Exceeded::Work => "the handler used up its work limit",
        })
    }
}

impl std::error::Error for Exceeded {}

/// A call its phase or the scripts' permissions do not allow.
#[derive(Clone, Debug)]
pub(crate) struct Refused(pub String);

impl fmt::Display for Refused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

pub(crate) fn refused(message: impl Into<String>) -> mlua::Error {
    mlua::Error::external(Refused(message.into()))
}

/// Work the gateway does for a script that waits: the run's driver serves
/// it between resumptions of the script.
#[derive(Debug)]
pub(crate) enum HostCall {
    ReadBody {
        limit: usize,
    },
    /// The next piece of the request body, for `ngx.req.socket`.
    ReadBodyChunk,
    /// The subrequests of `ngx.location.capture`.
    Capture(Vec<crate::capture::Capture>),
    /// Output sent to the client before the handler ends.
    Send(crate::stream::Output),
}

#[derive(Debug)]
pub(crate) enum HostReply {
    Body(Result<Bytes, String>),
    Chunk(Result<Option<Bytes>, String>),
    Captured(Result<Vec<crate::capture::Captured>, String>),
    Sent(Result<(), String>),
}

/// The run under way for a request.
#[derive(Debug, Default)]
pub(crate) struct Run {
    /// The limits the run was given, which the timers it creates run on.
    pub limits: Limits,
    pub sockets: Sockets,
    /// `ngx.header` keeps underscores in names.
    pub keep_underscores: bool,
    pub permissions: Permissions,
    pub log_level: Option<LogLevel>,
    pub work_left: i64,
    /// Nanoseconds after the VM's epoch.
    pub deadline: u64,
    pub exceeded: Option<Exceeded>,
    /// The interrupt suspended the run to let other tasks go first.
    pub sliced: bool,
    pub call: Option<(HostCall, oneshot::Sender<HostReply>)>,
    /// The run watches for the client closing the connection
    /// (`lua_check_client_abort on`).
    pub check_abort: bool,
    /// What `ngx.on_abort` registered, to run as a light thread when the
    /// client closes the connection.
    pub on_abort: Option<mlua::Thread>,
    /// The response goes to the client as the handler makes it.
    pub streams: bool,
}

/// A request on a VM: what its scripts read and change, its globals and
/// `ngx.ctx`, and the run under way.
#[derive(Debug)]
pub(crate) struct Cell {
    pub exchange: Arc<Mutex<Exchange>>,
    pub env: Mutex<Option<Table>>,
    pub ctx: Mutex<Option<Table>>,
    pub run: Mutex<Run>,
    pub threads: Mutex<Threads>,
}

impl Cell {
    pub fn new(exchange: Arc<Mutex<Exchange>>) -> Self {
        Self {
            exchange,
            env: Mutex::new(None),
            ctx: Mutex::new(None),
            run: Mutex::new(Run::default()),
            threads: Mutex::new(Threads::default()),
        }
    }
}

/// The light threads of the run under way, by the address of their
/// coroutine.
#[derive(Default)]
pub(crate) struct Threads {
    /// Threads that wait, for the run's driver to poll.
    pub waiting: Vec<(usize, Pin<Box<AsyncThread<MultiValue>>>)>,
    pub spawned: HashMap<usize, LightThread>,
}

impl fmt::Debug for Threads {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Threads")
            .field("waiting", &self.waiting.len())
            .field("spawned", &self.spawned.len())
            .finish()
    }
}

#[derive(Debug)]
pub(crate) struct LightThread {
    /// The coroutine that spawned it, the only one that may wait on it or
    /// kill it.
    pub parent: usize,
    pub state: ThreadState,
}

#[derive(Debug)]
pub(crate) enum ThreadState {
    Running,
    /// Ended, with what it returned or why it failed, not yet waited on.
    Ended(Result<MultiValue, String>),
    /// Waited on or killed.
    Dead,
}

/// What the VM knows of the run whose code is executing. The interrupt
/// reads it at every check, so it holds plain counters; the request it
/// belongs to is swapped in and out as threads resume and yield.
#[derive(Debug)]
pub(crate) struct Slot {
    epoch: Instant,
    entry: AtomicUsize,
    work_left: AtomicI64,
    deadline: AtomicU64,
    slice_end: AtomicU64,
    pub current: Mutex<Option<Arc<Cell>>>,
    pub entries: Mutex<HashMap<usize, Arc<Cell>>>,
    /// Runs reach nothing outside, as in a test.
    pub isolated: AtomicBool,
    /// The VM's place in its runtime.
    pub index: usize,
}

impl Slot {
    fn new(index: usize) -> Self {
        Self {
            index,
            epoch: Instant::now(),
            entry: AtomicUsize::new(0),
            work_left: AtomicI64::new(i64::MAX),
            deadline: AtomicU64::new(u64::MAX),
            slice_end: AtomicU64::new(u64::MAX),
            current: Mutex::new(None),
            entries: Mutex::new(HashMap::new()),
            isolated: AtomicBool::new(false),
        }
    }

    pub fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    pub fn after(&self, duration: Duration) -> u64 {
        self.now()
            .saturating_add(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
    }

    /// The request whose code is executing.
    pub fn cell(&self) -> Option<Arc<Cell>> {
        self.current.lock().clone()
    }

    /// Makes `cell` the request whose code executes, with its budget. What
    /// the code that executed before used is kept first, since a light
    /// thread resumed from within its parent does not yield it.
    pub fn enter(&self, key: usize, cell: &Arc<Cell>) {
        if let Some(previous) = self.current.lock().as_ref() {
            previous.run.lock().work_left = self.work_left.load(Relaxed);
        }
        {
            let run = cell.run.lock();
            self.entry.store(key, Relaxed);
            let work_left = if run.exceeded.is_some() {
                -1
            } else {
                run.work_left
            };
            self.work_left.store(work_left, Relaxed);
            self.deadline.store(run.deadline, Relaxed);
        }
        self.slice_end.store(self.after(SLICE), Relaxed);
        *self.current.lock() = Some(Arc::clone(cell));
    }

    /// Keeps what is left of the budget of the run that yielded.
    fn leave(&self, key: usize) {
        if self.entry.load(Relaxed) != key {
            return;
        }
        if let Some(cell) = self.current.lock().as_ref() {
            cell.run.lock().work_left = self.work_left.load(Relaxed);
        }
    }

    fn resumed(&self, key: usize) {
        let cell = self.entries.lock().get(&key).cloned();
        if let Some(cell) = cell {
            self.enter(key, &cell);
        }
    }

    fn interrupt(&self, lua: &Lua) -> mlua::Result<VmState> {
        let left = self.work_left.load(Relaxed) - 1;
        self.work_left.store(left, Relaxed);
        if left < 0 {
            return Err(self.trip(Exceeded::Work));
        }
        if left % CLOCK_EVERY == 0 {
            let now = self.now();
            if now >= self.deadline.load(Relaxed) {
                return Err(self.trip(Exceeded::Time));
            }
            if now >= self.slice_end.load(Relaxed)
                && lua.current_thread().to_pointer() as usize == self.entry.load(Relaxed)
            {
                self.slice_end.store(self.after(SLICE), Relaxed);
                if let Some(cell) = self.current.lock().as_ref() {
                    cell.run.lock().sliced = true;
                }
                return Ok(VmState::Yield);
            }
        }
        Ok(VmState::Continue)
    }

    /// Records the limit the run went over; every later check fails too, so
    /// a script cannot catch the error and go on.
    fn trip(&self, exceeded: Exceeded) -> mlua::Error {
        self.work_left.store(-1, Relaxed);
        if let Some(cell) = self.current.lock().as_ref() {
            cell.run.lock().exceeded.get_or_insert(exceeded);
        }
        mlua::Error::external(exceeded)
    }
}

/// A VM, its loaded handlers and the metatable request globals fall back
/// through.
pub(crate) struct Vm {
    pub lua: Lua,
    pub slot: Arc<Slot>,
    handlers: Vec<Function>,
    env_meta: Table,
    /// `exit_worker_by_lua`, and the terms it runs on.
    exit_worker: Option<(Function, Limits, Permissions)>,
}

impl fmt::Debug for Vm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Vm")
            .field("handlers", &self.handlers.len())
            .finish_non_exhaustive()
    }
}

impl Vm {
    pub fn new(
        program: &Program,
        dicts: &HashMap<String, Arc<Dict>>,
        settings: &Settings,
        index: usize,
        timers: Arc<Timers>,
        worker_threads: std::sync::Weak<crate::worker::WorkerThreads>,
        worker: bool,
    ) -> mlua::Result<(Self, Vec<crate::exchange::LogEntry>)> {
        let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::new())?;
        lua.set_app_data(api::Regexes::new(
            program.regex_cache.unwrap_or(api::REGEX_CACHE_ENTRIES),
            program.regex_match_limit,
        ));
        lua.set_app_data(crate::tls::TlsTable(program.tls.clone().into()));
        lua.set_app_data(api::ErrorLog::new(program.error_log));
        let slot = Arc::new(Slot::new(index));
        {
            let slot = Arc::clone(&slot);
            lua.set_interrupt(move |lua| slot.interrupt(lua));
        }
        {
            let slot = Arc::clone(&slot);
            lua.set_thread_event_callback(ThreadTriggers::ON_RESUME.on_yield(), move |_, event| {
                match event {
                    ThreadEvent::Resume(thread) => slot.resumed(thread.to_pointer() as usize),
                    ThreadEvent::Yield(thread) => slot.leave(thread.to_pointer() as usize),
                    _ => {}
                }
                Ok(())
            });
        }
        let base = lua.globals();
        api::install(
            &lua,
            &base,
            &api::Context {
                slot: Arc::clone(&slot),
                modules: Arc::new(program.modules.clone()),
                dicts: dicts.clone(),
                worker: index,
                workers: settings.vms,
                timers,
                worker_threads,
            },
        )?;
        let load = |index: HandlerId| -> mlua::Result<Function> {
            let compiled = &program.handlers[index.0 as usize];
            lua.load(&compiled.bytecode[..])
                .set_name(format!("={}", compiled.name))
                .set_mode(ChunkMode::Binary)
                .into_function()
        };
        // `init_by_lua` and `init_worker_by_lua` run in the global
        // environment before it is frozen: what they define is there for
        // every request to read, and like the standard library it cannot be
        // changed. State requests change lives in modules and shared
        // dictionaries.
        // A worker thread's VM loads the modules and runs nothing else.
        let logs = if worker {
            Vec::new()
        } else {
            Self::initialize(&slot, program, &load)?
        };
        lua.sandbox(true)?;
        let handlers = (0..program.handlers.len())
            .map(|index| load(HandlerId(index as u32)))
            .collect::<mlua::Result<Vec<_>>>()?;
        let env_meta = lua.create_table()?;
        env_meta.set("__index", base)?;
        env_meta.set_readonly(true);
        let exit_worker = program
            .exit_worker
            .map(load)
            .transpose()?
            .map(|function| (function, program.init_limits, program.init_permissions));
        lua.set_memory_limit(settings.memory)?;
        Ok((
            Self {
                lua,
                slot,
                handlers,
                env_meta,
                exit_worker,
            },
            logs,
        ))
    }

    fn initialize(
        slot: &Arc<Slot>,
        program: &Program,
        load: &dyn Fn(HandlerId) -> mlua::Result<Function>,
    ) -> mlua::Result<Vec<crate::exchange::LogEntry>> {
        let mut logs = Vec::new();
        for (phase, handler) in [
            (Phase::Init, program.init),
            (Phase::InitWorker, program.init_worker),
        ] {
            let Some(handler) = handler else {
                continue;
            };
            let mut exchange = Exchange::new(Default::default(), Default::default());
            exchange.begin(phase);
            let cell = Arc::new(Cell::new(Arc::new(Mutex::new(exchange))));
            {
                let mut run = cell.run.lock();
                run.limits = program.init_limits;
                run.permissions = program.init_permissions;
                run.work_left = i64::try_from(program.init_limits.work).unwrap_or(i64::MAX);
                run.deadline = slot.after(program.init_limits.time);
            }
            let function = load(handler)?;
            // Keyed 0, which no thread is, a run on the main thread is never
            // sliced: it cannot yield.
            slot.enter(0, &cell);
            let result = function.call::<()>(());
            *slot.current.lock() = None;
            logs.append(&mut cell.exchange.lock().logs);
            if let Some(exceeded) = cell.run.lock().exceeded {
                return Err(mlua::Error::external(exceeded));
            }
            result?;
        }
        Ok(logs)
    }

    /// Runs `exit_worker_by_lua` as the VM stops, on the terms of
    /// `init_by_lua`; nothing it starts may wait.
    pub fn exit(&self, index: usize) -> Option<crate::timer::TimerRun> {
        let (function, limits, permissions) = self.exit_worker.as_ref()?;
        let started = Instant::now();
        let mut exchange = Exchange::new(Default::default(), Default::default());
        exchange.begin(Phase::ExitWorker);
        let cell = Arc::new(Cell::new(Arc::new(Mutex::new(exchange))));
        {
            let mut run = cell.run.lock();
            run.limits = *limits;
            run.permissions = *permissions;
            run.work_left = i64::try_from(limits.work).unwrap_or(i64::MAX);
            run.deadline = self.slot.after(limits.time);
        }
        self.slot.enter(0, &cell);
        let result = function.call::<()>(());
        *self.slot.current.lock() = None;
        let exceeded = cell.run.lock().exceeded;
        let failure = match (exceeded, result) {
            (Some(exceeded), _) => Some(crate::runtime::exceeded_failure(exceeded)),
            (None, Err(error)) => Some(crate::runtime::failure(&error)),
            (None, Ok(())) => None,
        };
        let logs = std::mem::take(&mut cell.exchange.lock().logs);
        Some(crate::timer::TimerRun {
            vm: index,
            phase: Phase::ExitWorker,
            premature: false,
            duration: started.elapsed(),
            failure,
            logs,
        })
    }

    /// A global environment for a request: writes stay in it, reads fall
    /// through to the frozen globals.
    pub fn request_env(&self) -> mlua::Result<Table> {
        let env = self.lua.create_table()?;
        env.set_metatable(Some(self.env_meta.clone()))?;
        env.set_safeenv(true);
        Ok(env)
    }

    /// A thread running `handler` with the request's globals.
    pub fn entry(&self, handler: HandlerId, env: &Table) -> mlua::Result<Thread> {
        let function = self
            .handlers
            .get(handler.0 as usize)
            .ok_or_else(|| mlua::Error::runtime("no such handler"))?
            .deep_clone()?;
        function.set_environment(env.clone())?;
        self.lua.create_thread(function)
    }
}
