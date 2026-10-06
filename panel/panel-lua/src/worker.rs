//! `ngx.run_worker_thread`: a module's function run on a thread of its own,
//! in a VM of its own, so CPU-bound work does not hold the thread requests
//! run on. Arguments and results are copied between the VMs; the function
//! runs on the time and work limits of the run that called it.

use crate::{
    api::{cell, results, Api},
    exchange::{Exchange, Limits, Phase},
    program::Program,
    runtime::{exceeded_failure, failure, Settings},
    shared::Dict,
    timer::Timers,
    vm::{Cell, Vm},
};
use mlua::{Function, Lua, MultiValue, Table, Value};
use parking_lot::Mutex;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc, Weak,
    },
};

/// lua-nginx-module's default for `lua_worker_thread_vm_pool_size`.
pub(crate) const MOST_VMS: usize = 10;
/// How deeply tables passed to a worker thread may nest.
const MOST_DEPTH: usize = 100;

/// A value that crosses between VMs.
#[derive(Debug)]
enum Copied {
    Nil,
    Boolean(bool),
    Integer(i64),
    Number(f64),
    String(Vec<u8>),
    Table(Vec<(Copied, Copied)>),
}

impl Copied {
    fn of(value: &Value, path: &mut HashSet<usize>) -> Result<Self, String> {
        Ok(match value {
            Value::Nil => Self::Nil,
            Value::Boolean(flag) => Self::Boolean(*flag),
            Value::Integer(number) => Self::Integer(*number),
            Value::Number(number) => Self::Number(*number),
            Value::String(text) => Self::String(text.as_bytes().to_vec()),
            Value::Table(table) => {
                let key = table.to_pointer() as usize;
                if path.len() >= MOST_DEPTH || !path.insert(key) {
                    return Err("a table that holds itself cannot cross to a worker thread".into());
                }
                let mut pairs = Vec::new();
                for pair in table.pairs::<Value, Value>() {
                    let (key, value) = pair.map_err(|error| error.to_string())?;
                    pairs.push((Self::of(&key, path)?, Self::of(&value, path)?));
                }
                path.remove(&key);
                Self::Table(pairs)
            }
            other => {
                return Err(format!(
                    "a {} cannot cross to a worker thread",
                    other.type_name()
                ))
            }
        })
    }

    fn into_value(self, lua: &Lua) -> mlua::Result<Value> {
        Ok(match self {
            Self::Nil => Value::Nil,
            Self::Boolean(flag) => Value::Boolean(flag),
            Self::Integer(number) => Value::Integer(number),
            Self::Number(number) => Value::Number(number),
            Self::String(text) => Value::String(lua.create_string(text)?),
            Self::Table(pairs) => {
                let table = lua.create_table()?;
                for (key, value) in pairs {
                    table.raw_set(key.into_value(lua)?, value.into_value(lua)?)?;
                }
                Value::Table(table)
            }
        })
    }

    fn all(values: &MultiValue) -> Result<Vec<Self>, String> {
        values
            .iter()
            .map(|value| Self::of(value, &mut HashSet::new()))
            .collect()
    }
}

/// The VMs worker threads run on, made as they are needed up to a limit.
pub(crate) struct WorkerThreads {
    program: Program,
    dicts: HashMap<String, Arc<Dict>>,
    settings: Settings,
    timers: Arc<Timers>,
    idle: Mutex<Vec<Vm>>,
    made: AtomicUsize,
    most: usize,
}

impl std::fmt::Debug for WorkerThreads {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerThreads")
            .field("made", &self.made.load(Relaxed))
            .field("most", &self.most)
            .finish_non_exhaustive()
    }
}

impl WorkerThreads {
    pub fn new(
        program: &Program,
        dicts: HashMap<String, Arc<Dict>>,
        settings: &Settings,
        timers: Arc<Timers>,
    ) -> Self {
        Self {
            program: program.clone(),
            dicts,
            settings: settings.clone(),
            timers,
            idle: Mutex::new(Vec::new()),
            made: AtomicUsize::new(0),
            most: match program.worker_vms {
                0 => MOST_VMS,
                most => most,
            },
        }
    }

    fn take(self: &Arc<Self>) -> Result<Vm, String> {
        if let Some(vm) = self.idle.lock().pop() {
            return Ok(vm);
        }
        if self.made.fetch_add(1, Relaxed) >= self.most {
            self.made.fetch_sub(1, Relaxed);
            return Err("no available Lua vm".into());
        }
        let made = Vm::new(
            &self.program,
            &self.dicts,
            &self.settings,
            0,
            Arc::clone(&self.timers),
            Arc::downgrade(self),
            true,
        );
        made.map(|(vm, _)| vm).map_err(|error| {
            self.made.fetch_sub(1, Relaxed);
            failure(&error).message
        })
    }

    /// Runs `function` of `module` with `args` on a worker VM, which goes
    /// back to the pool once it is done, whoever still waits for it.
    async fn run(
        self: &Arc<Self>,
        module: String,
        function: String,
        args: Vec<Copied>,
        limits: Limits,
    ) -> Result<Vec<Copied>, String> {
        let vm = self.take()?;
        let pool = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let result = vm.call(&module, &function, args, limits);
            pool.idle.lock().push(vm);
            result
        })
        .await
        .unwrap_or_else(|_| Err("the worker thread stopped".into()))
    }
}

impl Vm {
    fn call(
        &self,
        module: &str,
        function: &str,
        args: Vec<Copied>,
        limits: Limits,
    ) -> Result<Vec<Copied>, String> {
        let mut exchange = Exchange::new(Default::default(), Default::default());
        exchange.begin(Phase::WorkerThread);
        let cell = Arc::new(Cell::new(Arc::new(Mutex::new(exchange))));
        {
            let mut run = cell.run.lock();
            run.limits = limits;
            run.work_left = i64::try_from(limits.work).unwrap_or(i64::MAX);
            run.deadline = self.slot.after(limits.time);
        }
        // No thread is keyed 0, so the run is never sliced: it holds a thread
        // of its own.
        self.slot.enter(0, &cell);
        let called = (|| -> mlua::Result<Result<Vec<Copied>, String>> {
            let require: Function = self.lua.globals().get("require")?;
            let table: Table = require.call(module)?;
            let Value::Function(function) = table.get::<Value>(function)? else {
                return Ok(Err(format!("{module}.{function} is not a function")));
            };
            let args = args
                .into_iter()
                .map(|arg| arg.into_value(&self.lua))
                .collect::<mlua::Result<MultiValue>>()?;
            let values: MultiValue = function.call(args)?;
            Ok(Copied::all(&values))
        })();
        *self.slot.current.lock() = None;
        if let Some(exceeded) = cell.run.lock().exceeded {
            return Err(exceeded_failure(exceeded).message);
        }
        called.map_err(|error| failure(&error).message)?
    }
}

pub(crate) fn install(
    lua: &Lua,
    ngx: &Table,
    slot: &Arc<crate::vm::Slot>,
    workers: &Weak<WorkerThreads>,
) -> mlua::Result<()> {
    let slot = Arc::clone(slot);
    let workers = Weak::clone(workers);
    ngx.raw_set(
        "run_worker_thread",
        lua.create_async_function(
            move |lua, (pool, module, function, args): (String, String, String, MultiValue)| {
                let prepared = cell(&slot, Api::RunWorkerThread).and_then(|cell| {
                    if pool.is_empty() {
                        return Err(mlua::Error::runtime(
                            "bad argument #1 to 'run_worker_thread' (a thread pool name)",
                        ));
                    }
                    let args = Copied::all(&args).map_err(mlua::Error::runtime)?;
                    let limits = cell.run.lock().limits;
                    Ok((args, limits))
                });
                let workers = workers.upgrade();
                async move {
                    let (args, limits) = prepared?;
                    let Some(workers) = workers else {
                        return Err(mlua::Error::runtime("the runtime has stopped"));
                    };
                    match workers.run(module, function, args, limits).await {
                        Ok(values) => Ok(results(
                            std::iter::once(Ok(Value::Boolean(true)))
                                .chain(values.into_iter().map(|value| value.into_value(&lua)))
                                .collect::<mlua::Result<Vec<_>>>()?,
                        )),
                        Err(error) => Ok(results([
                            Value::Boolean(false),
                            Value::String(lua.create_string(error)?),
                        ])),
                    }
                }
            },
        )?,
    )
}
