//! `ngx.timer`: callbacks that run after a delay on the VM whose code
//! created them, detached from any request, under the limits and
//! permissions of the run that created them. Once the runtime is dropped,
//! as when a configuration replaces it, the timers still pending run at once
//! with `premature` true, as lua-nginx-module runs them when a worker exits.

use crate::{
    api::{cell, failed, results, Api},
    exchange::{
        Exchange, Failure, FailureKind, Limits, LogEntry, LogLevel, Permissions, Phase, Sockets,
    },
    runtime::{drive, failure, settle, NoHost},
    vm::{Cell, Run, Slot},
};
use mlua::{Function, Lua, MultiValue, Table, Value};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, sync::watch};

/// lua-nginx-module's defaults for `lua_max_pending_timers` and
/// `lua_max_running_timers`.
pub(crate) const MOST_PENDING: usize = 1024;
pub(crate) const MOST_RUNNING: usize = 256;
/// Reports kept until the embedding takes them.
const KEPT_REPORTS: usize = 64;

/// What a timer's callback did.
#[derive(Clone, Debug)]
pub struct TimerRun {
    /// The VM it ran on.
    pub vm: usize,
    /// It ran early because its runtime was dropped.
    pub premature: bool,
    pub duration: Duration,
    /// Why it failed, if it did.
    pub failure: Option<Failure>,
    /// What it logged.
    pub logs: Vec<LogEntry>,
}

type Report = Arc<dyn Fn(TimerRun) + Send + Sync>;

enum Sink {
    Kept(VecDeque<TimerRun>),
    Attached(Report),
}

/// Where the runs of a runtime's timers go.
pub(crate) struct TimerReports {
    sink: Mutex<Sink>,
}

impl Default for TimerReports {
    fn default() -> Self {
        Self {
            sink: Mutex::new(Sink::Kept(VecDeque::new())),
        }
    }
}

impl fmt::Debug for TimerReports {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let attached = matches!(&*self.sink.lock(), Sink::Attached(_));
        formatter
            .debug_struct("TimerReports")
            .field("attached", &attached)
            .finish()
    }
}

impl TimerReports {
    /// Sends every later run to `report`, after the ones kept so far.
    pub fn attach(&self, report: Report) {
        let kept = std::mem::replace(&mut *self.sink.lock(), Sink::Attached(Arc::clone(&report)));
        if let Sink::Kept(kept) = kept {
            kept.into_iter().for_each(|run| report(run));
        }
    }

    fn send(&self, run: TimerRun) {
        let report = match &mut *self.sink.lock() {
            Sink::Attached(report) => Arc::clone(report),
            Sink::Kept(kept) => {
                if kept.len() == KEPT_REPORTS {
                    kept.pop_front();
                }
                kept.push_back(run);
                return;
            }
        };
        report(run);
    }
}

/// The timers of one VM.
#[derive(Debug)]
pub(crate) struct Timers {
    vm: usize,
    most_pending: usize,
    most_running: usize,
    pending: AtomicUsize,
    running: AtomicUsize,
    discarded: AtomicBool,
    /// Closed when the runtime is dropped.
    closing: watch::Receiver<()>,
    reports: Arc<TimerReports>,
    /// The event loop the runtime started on, for timers created where
    /// there is none.
    handle: Option<Handle>,
}

impl Timers {
    pub fn new(
        vm: usize,
        (most_pending, most_running): (usize, usize),
        closing: watch::Receiver<()>,
        reports: Arc<TimerReports>,
        handle: Option<Handle>,
    ) -> Self {
        Self {
            vm,
            most_pending,
            most_running,
            pending: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            discarded: AtomicBool::new(false),
            closing,
            reports,
            handle,
        }
    }

    /// Makes the pending timers end without running.
    pub fn discard(&self) {
        self.discarded.store(true, Relaxed);
    }

    fn closed(&self) -> bool {
        self.closing.has_changed().is_err()
    }
}

/// The terms of the run that created a timer, which its callback runs on.
#[derive(Clone, Copy)]
struct Terms {
    limits: Limits,
    sockets: Sockets,
    keep_underscores: bool,
    permissions: Permissions,
    log_level: Option<LogLevel>,
}

pub(crate) fn install(
    lua: &Lua,
    ngx: &Table,
    slot: &Arc<Slot>,
    timers: &Arc<Timers>,
) -> mlua::Result<()> {
    let table: Table = ngx.raw_get("timer")?;
    table.raw_set("at", create(lua, slot, timers, false)?)?;
    table.raw_set("every", create(lua, slot, timers, true)?)?;
    let counted = Arc::clone(timers);
    table.raw_set(
        "pending_count",
        lua.create_function(move |_, ()| Ok(counted.pending.load(Relaxed)))?,
    )?;
    let counted = Arc::clone(timers);
    table.raw_set(
        "running_count",
        lua.create_function(move |_, ()| Ok(counted.running.load(Relaxed)))?,
    )?;
    Ok(())
}

fn create(
    lua: &Lua,
    slot: &Arc<Slot>,
    timers: &Arc<Timers>,
    every: bool,
) -> mlua::Result<Function> {
    let name = if every { "every" } else { "at" };
    let slot = Arc::clone(slot);
    let timers = Arc::clone(timers);
    lua.create_function(
        move |lua, (delay, callback, args): (f64, Function, MultiValue)| {
            let cell = cell(&slot, Api::Timer)?;
            if !delay.is_finite() || delay < 0.0 {
                return Err(mlua::Error::runtime(format!(
                    "bad argument #1 to '{name}' (delay cannot be negative)"
                )));
            }
            if every && delay == 0.0 {
                return Err(mlua::Error::runtime(format!(
                    "bad argument #1 to '{name}' (delay cannot be zero)"
                )));
            }
            if timers.closed() {
                return failed(lua, 1, "process exiting");
            }
            if slot.isolated.load(Relaxed) {
                cell.exchange.lock().log(
                    LogLevel::Notice,
                    "a timer created in a test does not run".into(),
                );
                return Ok(results([Value::Boolean(true)]));
            }
            let Some(handle) = Handle::try_current().ok().or_else(|| timers.handle.clone()) else {
                return failed(lua, 1, "no event loop to run timers on");
            };
            if timers.pending.fetch_add(1, Relaxed) >= timers.most_pending {
                timers.pending.fetch_sub(1, Relaxed);
                return failed(lua, 1, "too many pending timers");
            }
            let terms = {
                let run = cell.run.lock();
                Terms {
                    limits: run.limits,
                    sockets: run.sockets,
                    keep_underscores: run.keep_underscores,
                    permissions: run.permissions,
                    log_level: run.log_level,
                }
            };
            handle.spawn(fire(Timer {
                lua: lua.clone(),
                slot: Arc::clone(&slot),
                timers: Arc::clone(&timers),
                callback,
                args,
                delay: Duration::from_secs_f64(delay),
                every,
                terms,
            }));
            Ok(results([Value::Boolean(true)]))
        },
    )
}

struct Timer {
    lua: Lua,
    slot: Arc<Slot>,
    timers: Arc<Timers>,
    callback: Function,
    args: MultiValue,
    delay: Duration,
    every: bool,
    terms: Terms,
}

async fn fire(timer: Timer) {
    let timers = Arc::clone(&timer.timers);
    let mut closing = timers.closing.clone();
    loop {
        let premature = tokio::select! {
            () = tokio::time::sleep(timer.delay) => false,
            _ = closing.changed() => true,
        };
        timers.pending.fetch_sub(1, Relaxed);
        if timers.discarded.load(Relaxed) {
            return;
        }
        let ran = if timers.running.fetch_add(1, Relaxed) >= timers.most_running {
            TimerRun {
                vm: timers.vm,
                premature,
                duration: Duration::ZERO,
                failure: Some(Failure {
                    kind: FailureKind::Refused,
                    message: "too many running timers".into(),
                }),
                logs: Vec::new(),
            }
        } else {
            timer.run(premature).await
        };
        timers.running.fetch_sub(1, Relaxed);
        timers.reports.send(ran);
        if !timer.every || premature {
            return;
        }
        timers.pending.fetch_add(1, Relaxed);
    }
}

impl Timer {
    async fn run(&self, premature: bool) -> TimerRun {
        let started = Instant::now();
        let mut exchange = Exchange::new(Default::default(), Default::default());
        exchange.begin(Phase::Timer);
        let cell = Arc::new(Cell::new(Arc::new(Mutex::new(exchange))));
        let limits = self.terms.limits;
        *cell.run.lock() = Run {
            limits,
            sockets: self.terms.sockets,
            keep_underscores: self.terms.keep_underscores,
            permissions: self.terms.permissions,
            log_level: self.terms.log_level,
            work_left: i64::try_from(limits.work).unwrap_or(i64::MAX),
            deadline: self.slot.after(limits.time),
            ..Default::default()
        };
        let failure = match self.lua.create_thread(self.callback.clone()) {
            Err(error) => Some(failure(&error)),
            Ok(thread) => {
                let key = thread.to_pointer() as usize;
                self.slot.entries.lock().insert(key, Arc::clone(&cell));
                let mut args = MultiValue::with_capacity(self.args.len() + 1);
                args.push_back(Value::Boolean(premature));
                args.extend(self.args.iter().cloned());
                let result = tokio::time::timeout(
                    limits.time,
                    drive(&self.slot, &thread, &cell, &mut NoHost, args),
                )
                .await;
                settle(&self.slot, key, &cell, result).err()
            }
        };
        let logs = std::mem::take(&mut cell.exchange.lock().logs);
        TimerRun {
            vm: self.timers.vm,
            premature,
            duration: started.elapsed(),
            failure,
            logs,
        }
    }
}
