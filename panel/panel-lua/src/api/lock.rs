//! `resty.lock`: lua-resty-lock's locks, keys of an `ngx.shared`
//! dictionary that wait with its backoff. Each lock keeps a token of its
//! own in its key, so it releases and extends only the lock it holds and
//! never one another took after its own expired, and a lock whose object
//! is collected is released, as lua-resty-lock's are.

use super::{cell, failed, results, shared::Handle, Api};
use crate::{
    shared::{Dict, Refusal, Scalar, SetMode},
    vm::Slot,
};
use mlua::{Lua, LuaString, MultiValue, Table, UserData, UserDataMethods, UserDataRef, Value};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

/// Numbers that make each lock's token its own within the process, which
/// the dictionaries belong to.
static TOKENS: AtomicU64 = AtomicU64::new(0);

struct Lock {
    slot: Arc<Slot>,
    dict: Arc<Dict>,
    token: Scalar,
    held: Option<Vec<u8>>,
    exptime: f64,
    timeout: f64,
    step: f64,
    ratio: f64,
    max_step: f64,
}

impl Drop for Lock {
    fn drop(&mut self) {
        if let Some(key) = self.held.take() {
            self.dict.delete_if(&key, &self.token);
        }
    }
}

/// A time to live of `seconds`; none, for ever, at zero.
fn lifetime(seconds: f64) -> Option<Duration> {
    (seconds > 0.0 && seconds.is_finite()).then(|| Duration::from_secs_f64(seconds))
}

impl Lock {
    /// Takes `key` if no one holds it. Errors other than its being held end
    /// the wait.
    fn take(&mut self, key: &[u8]) -> Result<bool, Refusal> {
        match self.dict.set(
            key,
            Some(self.token.clone()),
            lifetime(self.exptime),
            0,
            SetMode::Add,
        ) {
            Ok(_) => {
                self.held = Some(key.to_vec());
                Ok(true)
            }
            Err(Refusal::Exists) => Ok(false),
            Err(refusal) => Err(refusal),
        }
    }
}

impl UserData for Lock {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method_mut("lock", |lua, mut this, key: LuaString| async move {
            if this.held.is_some() {
                return failed(&lua, 1, "locked");
            }
            let key = key.as_bytes().to_vec();
            if key.is_empty() {
                return Err(mlua::Error::runtime(
                    "bad argument #1 to 'lock' (empty key)",
                ));
            }
            let mut elapsed = 0.0;
            let mut step = this.step;
            loop {
                match this.take(&key) {
                    Ok(true) => return Ok(results([Value::Number(elapsed)])),
                    Ok(false) => {}
                    Err(refusal) => return failed(&lua, 1, refusal.message()),
                }
                if elapsed >= this.timeout {
                    return failed(&lua, 1, "timeout");
                }
                step = step.min(this.timeout - elapsed);
                cell(&this.slot, Api::Sleep)?;
                tokio::time::sleep(Duration::from_secs_f64(step)).await;
                elapsed += step;
                step = (step * this.ratio).min(this.max_step);
            }
        });
        methods.add_method_mut("unlock", |lua, this, ()| {
            let Some(key) = this.held.take() else {
                return failed(lua, 1, "unlocked");
            };
            if this.dict.delete_if(&key, &this.token) {
                Ok(results([Value::Integer(1)]))
            } else {
                failed(lua, 1, "unlocked")
            }
        });
        methods.add_method_mut("expire", |lua, this, timeout: Option<f64>| {
            let Some(key) = &this.held else {
                return failed(lua, 1, "unlocked");
            };
            let ttl = lifetime(timeout.unwrap_or(this.exptime));
            if this.dict.expire_if(key, &this.token, ttl) {
                Ok(results([Value::Integer(1)]))
            } else {
                this.held = None;
                failed(lua, 1, "unlocked")
            }
        });
    }
}

/// The `resty.lock` module.
pub(super) fn module(lua: &Lua, ngx: &Table, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let shared: Table = ngx.raw_get("shared")?;
    let slot = Arc::clone(slot);
    let module = lua.create_table()?;
    module.raw_set("_VERSION", "0.09")?;
    module.raw_set(
        "new",
        lua.create_function(
            move |lua,
                  (_, name, options): (Value, LuaString, Option<Table>)|
                  -> mlua::Result<MultiValue> {
                let Some(handle) = shared.raw_get::<Option<UserDataRef<Handle>>>(name)? else {
                    return failed(lua, 1, "dictionary not found");
                };
                let option = |field: &str, default: f64| -> mlua::Result<f64> {
                    Ok(match &options {
                        Some(options) => options.get::<Option<f64>>(field)?.unwrap_or(default),
                        None => default,
                    })
                };
                let lock = Lock {
                    slot: Arc::clone(&slot),
                    dict: Arc::clone(&handle.0),
                    token: Scalar::String(
                        format!("lock {}", TOKENS.fetch_add(1, Ordering::Relaxed)).into_bytes(),
                    ),
                    held: None,
                    exptime: option("exptime", 30.0)?,
                    timeout: option("timeout", 5.0)?.max(0.0),
                    step: option("step", 0.001)?.max(0.001),
                    ratio: option("ratio", 2.0)?.max(1.0),
                    max_step: option("max_step", 0.5)?.max(0.001),
                };
                Ok(results([Value::UserData(lua.create_userdata(lock)?)]))
            },
        )?,
    )?;
    Ok(module)
}
