//! `ngx.thread`: light threads that run with the request's code, on its
//! budget and permissions. A spawned thread runs until it first waits
//! before `spawn` returns; the run is over once its entry thread and every
//! light thread have ended, or as soon as one of them exits or the entry
//! thread fails.

use super::{cell, failed, results, Api};
use crate::{
    runtime::thread_ended,
    vm::{LightThread, Slot, ThreadState},
};
use mlua::{Function, IntoLua, Lua, MultiValue, Thread, Value, Variadic};
use std::{
    future::{poll_fn, Future},
    sync::Arc,
    task::Poll,
};

pub(super) fn install(lua: &Lua, ngx: &mlua::Table, slot: &Arc<Slot>) -> mlua::Result<()> {
    let thread: mlua::Table = ngx.raw_get("thread")?;
    thread.raw_set("spawn", spawn(lua, slot)?)?;
    thread.raw_set("wait", wait(lua, slot)?)?;
    thread.raw_set("kill", kill(lua, slot)?)?;
    ngx.raw_set("on_abort", on_abort(lua, slot)?)?;
    Ok(())
}

/// `ngx.on_abort`: the function to run as a light thread of the run when
/// the client closes the connection, once per handler.
fn on_abort(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Function> {
    let slot = Arc::clone(slot);
    lua.create_function(move |lua, callback: Function| {
        let cell = cell(&slot, Api::OnAbort)?;
        let mut run = cell.run.lock();
        if !run.check_abort {
            return failed(lua, 1, "lua_check_client_abort is off");
        }
        if run.on_abort.is_some() {
            return failed(lua, 1, "duplicate call");
        }
        run.on_abort = Some(lua.create_thread(callback)?);
        Ok(results([Value::Integer(1)]))
    })
}

fn key(thread: &Thread) -> usize {
    thread.to_pointer() as usize
}

fn spawn(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Function> {
    let slot = Arc::clone(slot);
    lua.create_async_function(move |lua, (function, args): (Function, MultiValue)| {
        let started = cell(&slot, Api::Thread).and_then(|cell| {
            let parent = key(&lua.current_thread());
            let thread = lua.create_thread(function)?;
            let future = Box::pin(thread.clone().into_async::<MultiValue>(args)?);
            Ok((cell, parent, thread, future))
        });
        let slot = Arc::clone(&slot);
        async move {
            let (cell, parent, thread, mut future) = started?;
            let child = key(&thread);
            slot.entries.lock().insert(child, Arc::clone(&cell));
            cell.threads.lock().spawned.insert(
                child,
                LightThread {
                    parent,
                    state: ThreadState::Running,
                },
            );
            let first = poll_fn(|context| Poll::Ready(future.as_mut().poll(context))).await;
            slot.enter(parent, &cell);
            match first {
                Poll::Ready(result) => thread_ended(&slot, &cell, child, result),
                Poll::Pending => cell.threads.lock().waiting.push((child, future)),
            }
            Ok(thread)
        }
    })
}

/// How a wait ended.
enum Waited {
    Ended(Result<MultiValue, String>),
    Dead,
}

fn wait(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Function> {
    let slot = Arc::clone(slot);
    lua.create_async_function(move |lua, threads: Variadic<Thread>| {
        let checked = cell(&slot, Api::Thread).and_then(|cell| {
            if threads.is_empty() {
                return Err(mlua::Error::runtime(
                    "at least one coroutine should be specified",
                ));
            }
            let me = key(&lua.current_thread());
            let keys: Vec<usize> = threads.iter().map(key).collect();
            let spawned = &cell.threads.lock().spawned;
            for wanted in &keys {
                match spawned.get(wanted) {
                    None => return Err(mlua::Error::runtime("not user thread")),
                    Some(thread) if thread.parent != me => {
                        return Err(mlua::Error::runtime(
                            "only the parent coroutine can wait on the thread",
                        ))
                    }
                    Some(_) => {}
                }
            }
            Ok((Arc::clone(&cell), keys))
        });
        async move {
            let (cell, keys) = checked?;
            // The run's driver polls again whenever a light thread ends.
            let waited = poll_fn(|_| {
                let mut threads = cell.threads.lock();
                for wanted in &keys {
                    let Some(thread) = threads.spawned.get_mut(wanted) else {
                        continue;
                    };
                    match &thread.state {
                        ThreadState::Running => {}
                        ThreadState::Dead => return Poll::Ready(Waited::Dead),
                        ThreadState::Ended(_) => {
                            let ThreadState::Ended(result) =
                                std::mem::replace(&mut thread.state, ThreadState::Dead)
                            else {
                                unreachable!("the state was just matched");
                            };
                            return Poll::Ready(Waited::Ended(result));
                        }
                    }
                }
                Poll::Pending
            })
            .await;
            match waited {
                Waited::Ended(Ok(values)) => {
                    Ok(results(std::iter::once(Value::Boolean(true)).chain(values)))
                }
                Waited::Ended(Err(message)) => {
                    Ok(results([Value::Boolean(false), message.into_lua(&lua)?]))
                }
                Waited::Dead => failed(&lua, 1, "already waited or killed"),
            }
        }
    })
}

fn kill(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Function> {
    let slot = Arc::clone(slot);
    lua.create_function(move |lua, thread: Thread| {
        let cell = cell(&slot, Api::Thread)?;
        let me = key(&lua.current_thread());
        let target = key(&thread);
        let removed = {
            let mut threads = cell.threads.lock();
            let Some(light) = threads.spawned.get_mut(&target) else {
                return Err(mlua::Error::runtime("not user thread"));
            };
            if light.parent != me {
                return Err(mlua::Error::runtime(
                    "only the parent coroutine can kill the thread",
                ));
            }
            match light.state {
                ThreadState::Ended(_) => return failed(lua, 1, "already terminated"),
                ThreadState::Dead => return failed(lua, 1, "already waited or killed"),
                ThreadState::Running => light.state = ThreadState::Dead,
            }
            let at = threads.waiting.iter().position(|(key, _)| *key == target);
            at.map(|at| threads.waiting.swap_remove(at))
        };
        slot.entries.lock().remove(&target);
        // Dropping the thread resets its coroutine, outside the lock.
        drop(removed);
        Ok(results([Value::Boolean(true)]))
    })
}
