//! `require("ngx.semaphore")`: lua-resty-core's semaphores, which the light
//! threads, timers and requests of one VM wait on and post to. `post` hands
//! resources to waiters in the order they came, and `count` is negative by
//! the number of waiters.

use super::{cell, failed, results, Api};
use crate::vm::Slot;
use mlua::{Lua, MultiValue, Table, UserData, UserDataMethods, UserDataRef, Value};
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc, time::Duration};
use tokio::sync::oneshot;

struct Waiter {
    id: u64,
    grant: oneshot::Sender<()>,
}

#[derive(Default)]
struct State {
    resources: i64,
    waiters: VecDeque<Waiter>,
    next: u64,
}

impl State {
    /// Takes `id` out of the queue; false once it was granted.
    fn leave(&mut self, id: u64) -> bool {
        let before = self.waiters.len();
        self.waiters.retain(|waiter| waiter.id != id);
        self.waiters.len() != before
    }
}

struct Semaphore {
    slot: Arc<Slot>,
    state: Arc<Mutex<State>>,
}

/// Leaves the queue when a wait ends without its resource, as when the
/// thread waiting is killed.
struct Queued {
    state: Arc<Mutex<State>>,
    id: u64,
}

impl Drop for Queued {
    fn drop(&mut self) {
        self.state.lock().leave(self.id);
    }
}

fn no_negative(number: f64) -> mlua::Result<()> {
    if number.is_nan() || number < 0.0 {
        Err(mlua::Error::runtime("no negative number"))
    } else {
        Ok(())
    }
}

impl UserData for Semaphore {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method(
            "wait",
            |lua, this: UserDataRef<Self>, seconds: f64| async move {
                cell(&this.slot, Api::SemaphoreWait)?;
                no_negative(seconds)?;
                let (id, mut granted) = {
                    let mut state = this.state.lock();
                    if state.resources > 0 {
                        state.resources -= 1;
                        return Ok(results([Value::Boolean(true)]));
                    }
                    if seconds == 0.0 {
                        return failed(&lua, 1, "timeout");
                    }
                    let id = state.next;
                    state.next += 1;
                    let (grant, granted) = oneshot::channel();
                    state.waiters.push_back(Waiter { id, grant });
                    (id, granted)
                };
                let queued = Queued {
                    state: Arc::clone(&this.state),
                    id,
                };
                let limit = Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX);
                let waited = tokio::time::timeout(limit, &mut granted).await;
                let ok = match waited {
                    Ok(result) => result.is_ok(),
                    // Granted as the wait timed out: the resource is taken.
                    Err(_) => !this.state.lock().leave(id) && granted.try_recv().is_ok(),
                };
                drop(queued);
                if ok {
                    Ok(results([Value::Boolean(true)]))
                } else {
                    failed(&lua, 1, "timeout")
                }
            },
        );
        methods.add_method("post", |_, this, n: Option<f64>| {
            let n = n.unwrap_or(1.0);
            if n.is_nan() || n < 1.0 {
                return Err(mlua::Error::runtime("positive number required"));
            }
            let mut state = this.state.lock();
            for _ in 0..(n as u64).min(i64::MAX as u64) {
                loop {
                    let Some(waiter) = state.waiters.pop_front() else {
                        state.resources += 1;
                        break;
                    };
                    if waiter.grant.send(()).is_ok() {
                        break;
                    }
                }
            }
            Ok(true)
        });
        methods.add_method("count", |_, this, ()| {
            let state = this.state.lock();
            Ok(state.resources - i64::try_from(state.waiters.len()).unwrap_or(i64::MAX))
        });
    }
}

pub(super) fn module(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    module.raw_set("version", "0.1.29")?;
    let slot = Arc::clone(slot);
    module.raw_set(
        "new",
        lua.create_function(move |lua, n: Option<f64>| {
            let n = n.unwrap_or(0.0);
            no_negative(n)?;
            let semaphore = Semaphore {
                slot: Arc::clone(&slot),
                state: Arc::new(Mutex::new(State {
                    resources: n as i64,
                    ..State::default()
                })),
            };
            Ok(MultiValue::from_iter([Value::UserData(
                lua.create_userdata(semaphore)?,
            )]))
        })?,
    )?;
    Ok(module)
}
