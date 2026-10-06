//! `ngx.errlog` and `lua_capture_error_log`: what a VM's scripts log, kept
//! in a buffer of the configured size for scripts to read back, oldest
//! messages first and dropped first.

use super::{failed, results};
use crate::{
    exchange::{LogLevel, Phase},
    vm::Slot,
};
use mlua::{Lua, LuaString, MultiValue, Table, Value};
use std::{collections::VecDeque, sync::Arc, time::SystemTime};

/// Bytes an entry takes beyond its message, as nginx counts its header.
const ENTRY_OVERHEAD: usize = 32;

const NOT_CONFIGURED: &str = "the 'lua_capture_error_log' directive is not configured";

struct Entry {
    level: LogLevel,
    time: f64,
    message: String,
}

/// The messages a VM keeps for `ngx.errlog.get_logs`.
pub(crate) struct ErrorLog {
    capacity: usize,
    used: usize,
    entries: VecDeque<Entry>,
    /// The least severe level kept (`set_filter_level`).
    filter: Option<LogLevel>,
}

impl ErrorLog {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            used: 0,
            entries: VecDeque::new(),
            filter: None,
        }
    }

    fn push(&mut self, level: LogLevel, message: &str) {
        if self.capacity == 0 || self.filter.is_some_and(|filter| level > filter) {
            return;
        }
        let size = message.len() + ENTRY_OVERHEAD;
        if size > self.capacity {
            return;
        }
        while self.used + size > self.capacity {
            let Some(oldest) = self.entries.pop_front() else {
                break;
            };
            self.used -= oldest.message.len() + ENTRY_OVERHEAD;
        }
        let time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0.0, |since| since.as_millis() as f64 / 1000.0);
        self.entries.push_back(Entry {
            level,
            time,
            message: message.to_owned(),
        });
        self.used += size;
    }
}

/// Keeps `message` in the VM's buffer, when it has one.
pub(crate) fn capture(lua: &Lua, level: LogLevel, message: &str) {
    if let Some(mut log) = lua.app_data_mut::<ErrorLog>() {
        log.push(level, message);
    }
}

fn configured(lua: &Lua) -> bool {
    lua.app_data_ref::<ErrorLog>()
        .is_some_and(|log| log.capacity > 0)
}

/// The `ngx.errlog` module.
pub(super) fn module(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    module.raw_set(
        "get_logs",
        lua.create_function(|lua, (max, res): (Option<usize>, Option<Table>)| {
            if !configured(lua) {
                return failed(lua, 1, NOT_CONFIGURED);
            }
            let res = match res {
                Some(res) => res,
                None => lua.create_table()?,
            };
            let mut taken = Vec::new();
            if let Some(mut log) = lua.app_data_mut::<ErrorLog>() {
                for _ in 0..max.filter(|max| *max > 0).unwrap_or(10) {
                    let Some(entry) = log.entries.pop_front() else {
                        break;
                    };
                    log.used -= entry.message.len() + ENTRY_OVERHEAD;
                    taken.push(entry);
                }
            }
            let mut at = 0;
            for entry in taken {
                res.raw_set(at + 1, entry.level as i64)?;
                res.raw_set(at + 2, entry.time)?;
                res.raw_set(at + 3, lua.create_string(&entry.message)?)?;
                at += 3;
            }
            res.raw_set(at + 1, Value::Nil)?;
            Ok(results([Value::Table(res)]))
        })?,
    )?;
    let filter = Arc::clone(slot);
    module.raw_set(
        "set_filter_level",
        lua.create_function(move |lua, level: i64| {
            let Some(level) = LogLevel::from_number(level) else {
                return Err(mlua::Error::runtime("bad log level"));
            };
            if filter.cell().is_some_and(|cell| cell.exchange.lock().phase != Phase::Init) {
                return Err(mlua::Error::runtime(
                    "API disabled in the current context: set_filter_level only works in init_by_lua*",
                ));
            }
            if !configured(lua) {
                return failed(lua, 1, NOT_CONFIGURED);
            }
            if let Some(mut log) = lua.app_data_mut::<ErrorLog>() {
                log.filter = Some(level);
            }
            Ok(results([Value::Boolean(true)]))
        })?,
    )?;
    let level = Arc::clone(slot);
    module.raw_set(
        "get_sys_filter_level",
        lua.create_function(move |_, ()| {
            Ok(level
                .cell()
                .and_then(|cell| cell.run.lock().log_level)
                .unwrap_or(LogLevel::Notice) as i64)
        })?,
    )?;
    let raw = Arc::clone(slot);
    module.raw_set(
        "raw_log",
        lua.create_function(move |lua, (level, message): (i64, LuaString)| {
            let Some(level) = LogLevel::from_number(level) else {
                return Err(mlua::Error::runtime("bad log level"));
            };
            let message = String::from_utf8_lossy(&message.as_bytes()).into_owned();
            let cell = raw.cell();
            let least = cell
                .as_ref()
                .and_then(|cell| cell.run.lock().log_level)
                .unwrap_or(LogLevel::Notice);
            if level > least {
                return Ok(MultiValue::new());
            }
            capture(lua, level, &message);
            if let Some(cell) = cell {
                cell.exchange.lock().log(level, message);
            }
            Ok(MultiValue::new())
        })?,
    )?;
    Ok(module)
}
