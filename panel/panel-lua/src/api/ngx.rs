//! The `ngx` table.

use super::{
    bytes, cell, codec, exchange, log_level, re, req, resp, shared, time, var, Api, Context,
};
use crate::{
    exchange::{LogLevel, Phase},
    vm::{refused, Slot},
};
use mlua::{Lua, LuaString, Table, Value, Variadic};
use std::{sync::Arc, time::Duration};

const CORE: [(&str, i64); 5] = [
    ("OK", 0),
    ("ERROR", -1),
    ("AGAIN", -2),
    ("DONE", -4),
    ("DECLINED", -5),
];

/// `NGX_HTTP_*` method flags, as nginx defines them.
pub(crate) const METHODS: [(&str, i64); 16] = [
    ("GET", 0x0002),
    ("HEAD", 0x0004),
    ("POST", 0x0008),
    ("PUT", 0x0010),
    ("DELETE", 0x0020),
    ("MKCOL", 0x0040),
    ("COPY", 0x0080),
    ("MOVE", 0x0100),
    ("OPTIONS", 0x0200),
    ("PROPFIND", 0x0400),
    ("PROPPATCH", 0x0800),
    ("LOCK", 0x1000),
    ("UNLOCK", 0x2000),
    ("PATCH", 0x4000),
    ("TRACE", 0x8000),
    ("CONNECT", 0x10000),
];

const STATUSES: [(&str, i64); 37] = [
    ("CONTINUE", 100),
    ("SWITCHING_PROTOCOLS", 101),
    ("OK", 200),
    ("CREATED", 201),
    ("ACCEPTED", 202),
    ("NO_CONTENT", 204),
    ("PARTIAL_CONTENT", 206),
    ("SPECIAL_RESPONSE", 300),
    ("MOVED_PERMANENTLY", 301),
    ("MOVED_TEMPORARILY", 302),
    ("SEE_OTHER", 303),
    ("NOT_MODIFIED", 304),
    ("TEMPORARY_REDIRECT", 307),
    ("PERMANENT_REDIRECT", 308),
    ("BAD_REQUEST", 400),
    ("UNAUTHORIZED", 401),
    ("PAYMENT_REQUIRED", 402),
    ("FORBIDDEN", 403),
    ("NOT_FOUND", 404),
    ("NOT_ALLOWED", 405),
    ("NOT_ACCEPTABLE", 406),
    ("REQUEST_TIMEOUT", 408),
    ("CONFLICT", 409),
    ("GONE", 410),
    ("UPGRADE_REQUIRED", 426),
    ("TOO_MANY_REQUESTS", 429),
    ("CLOSE", 444),
    ("ILLEGAL", 451),
    ("INTERNAL_SERVER_ERROR", 500),
    ("NOT_IMPLEMENTED", 501),
    ("METHOD_NOT_IMPLEMENTED", 501),
    ("BAD_GATEWAY", 502),
    ("SERVICE_UNAVAILABLE", 503),
    ("GATEWAY_TIMEOUT", 504),
    ("VERSION_NOT_SUPPORTED", 505),
    ("INSUFFICIENT_STORAGE", 507),
    ("TOO_EARLY", 425),
];

/// What lua-nginx-module has and this gateway does not: each raises an error
/// naming itself rather than doing something else.
pub(crate) const UNAVAILABLE: [&str; 20] = [
    "ngx.exec",
    "ngx.on_abort",
    "ngx.run_worker_thread",
    "ngx.location.capture",
    "ngx.location.capture_multi",
    "ngx.socket.tcp",
    "ngx.socket.udp",
    "ngx.socket.stream",
    "ngx.socket.connect",
    "ngx.thread.spawn",
    "ngx.thread.wait",
    "ngx.thread.kill",
    "ngx.timer.at",
    "ngx.timer.every",
    "ngx.req.socket",
    "ngx.req.init_body",
    "ngx.req.append_body",
    "ngx.req.finish_body",
    "ngx.req.get_body_file",
    "ngx.req.set_body_file",
];

/// Identifies the lua-nginx-module release this API follows.
const NGX_LUA_VERSION: i64 = 10028;
const NGINX_VERSION: i64 = 1_027_001;

pub(super) fn table(lua: &Lua, context: &Context) -> mlua::Result<Table> {
    let ngx = lua.create_table()?;
    for (name, value) in CORE {
        ngx.raw_set(name, value)?;
    }
    for (name, value) in METHODS {
        ngx.raw_set(format!("HTTP_{name}"), value)?;
    }
    for (name, value) in STATUSES {
        ngx.raw_set(format!("HTTP_{name}"), value)?;
    }
    for level in LogLevel::ALL {
        let name = match level {
            LogLevel::Err => "ERR".to_owned(),
            other => other.name().to_ascii_uppercase(),
        };
        ngx.raw_set(name, level as i64)?;
    }
    ngx.raw_set("null", Value::NULL)?;
    let slot = &context.slot;
    ngx.raw_set("req", req::table(lua, slot)?)?;
    ngx.raw_set("resp", resp::resp_table(lua, slot)?)?;
    ngx.raw_set("header", resp::header_proxy(lua, slot)?)?;
    ngx.raw_set("arg", resp::arg_proxy(lua, slot)?)?;
    ngx.raw_set("var", var::proxy(lua, slot)?)?;
    ngx.raw_set("re", re::table(lua)?)?;
    ngx.raw_set("shared", shared::table(lua, context)?)?;
    resp::install(lua, &ngx, slot)?;
    codec::install(lua, &ngx)?;
    time::install(lua, &ngx)?;
    for (name, function) in [
        ("log", log(lua, slot, false)?),
        ("print_global", log(lua, slot, true)?),
        ("sleep", sleep(lua, slot)?),
        ("get_phase", get_phase(lua, slot)?),
    ] {
        ngx.raw_set(name, function)?;
    }
    ngx.raw_set("config", config(lua)?)?;
    ngx.raw_set("worker", worker(lua, context)?)?;
    for name in ["location", "socket", "thread", "timer"] {
        ngx.raw_set(name, lua.create_table()?)?;
    }
    let timer: Table = ngx.raw_get("timer")?;
    timer.raw_set("running_count", lua.create_function(|_, ()| Ok(0))?)?;
    timer.raw_set("pending_count", lua.create_function(|_, ()| Ok(0))?)?;
    for path in UNAVAILABLE {
        set_path(lua, &ngx, path, unavailable(lua, path)?)?;
    }
    ngx.set_metatable(Some(dynamic_fields(lua, slot)?))?;
    Ok(ngx)
}

fn set_path(lua: &Lua, ngx: &Table, path: &str, value: mlua::Function) -> mlua::Result<()> {
    let mut table = ngx.clone();
    let mut parts = path.trim_start_matches("ngx.").split('.').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            if table.raw_get::<Value>(part)?.is_nil() {
                table.raw_set(part, value.clone())?;
            }
            return Ok(());
        }
        table = match table.raw_get::<Value>(part)? {
            Value::Table(inner) => inner,
            _ => {
                let inner = lua.create_table()?;
                table.raw_set(part, inner.clone())?;
                inner
            }
        };
    }
    Ok(())
}

fn unavailable(lua: &Lua, path: &'static str) -> mlua::Result<mlua::Function> {
    lua.create_function(move |_, _: Variadic<Value>| -> mlua::Result<()> {
        Err(refused(format!("{path} is not available in Pingora Panel")))
    })
}

/// `ngx.ctx`, `ngx.status`, `ngx.headers_sent` and `ngx.is_subrequest`,
/// which depend on the request whose code runs.
fn dynamic_fields(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let meta = lua.create_table()?;
    let reader = Arc::clone(slot);
    meta.raw_set(
        "__index",
        lua.create_function(move |lua, (_, key): (Table, Value)| {
            let Value::String(key) = key else {
                return Ok(Value::Nil);
            };
            match &*key.as_bytes() {
                b"ctx" => {
                    let cell = cell(&reader, Api::Ctx)?;
                    let mut ctx = cell.ctx.lock();
                    let table = match &*ctx {
                        Some(table) => table.clone(),
                        None => ctx.insert(lua.create_table()?).clone(),
                    };
                    Ok(Value::Table(table))
                }
                b"status" => exchange(&reader, Api::Status, |exchange| {
                    Ok(Value::Integer(exchange.response.status.into()))
                }),
                b"headers_sent" => exchange(&reader, Api::HeadersSent, |exchange| {
                    Ok(Value::Boolean(exchange.headers_sent))
                }),
                b"is_subrequest" => {
                    cell(&reader, Api::IsSubrequest)?;
                    Ok(Value::Boolean(false))
                }
                _ => Ok(Value::Nil),
            }
        })?,
    )?;
    let writer = Arc::clone(slot);
    meta.raw_set(
        "__newindex",
        lua.create_function(move |_, (_, key, value): (Table, LuaString, Value)| {
            match &*key.as_bytes() {
                b"ctx" => {
                    let cell = cell(&writer, Api::Ctx)?;
                    let Value::Table(table) = value else {
                        return Err(mlua::Error::runtime("ngx.ctx must be a table"));
                    };
                    *cell.ctx.lock() = Some(table);
                    Ok(())
                }
                b"status" => resp::set_status(&writer, &value),
                other => Err(mlua::Error::runtime(format!(
                    "attempt to write to undeclared field ngx.{}",
                    String::from_utf8_lossy(other)
                ))),
            }
        })?,
    )?;
    Ok(meta)
}

/// `ngx.log(level, ...)`, and `print(...)` at `ngx.NOTICE`.
fn log(lua: &Lua, slot: &Arc<Slot>, print: bool) -> mlua::Result<mlua::Function> {
    let slot = Arc::clone(slot);
    lua.create_function(move |lua, mut args: Variadic<Value>| {
        let level = if print {
            LogLevel::Notice
        } else {
            let level = match args.first() {
                Some(Value::Integer(level)) => LogLevel::from_number(*level),
                Some(Value::Number(level)) => LogLevel::from_number(*level as i64),
                _ => None,
            };
            let Some(level) = level else {
                return Err(mlua::Error::runtime(
                    "bad argument #1 to 'log' (bad log level)",
                ));
            };
            args.remove(0);
            level
        };
        let Some(cell) = slot.cell() else {
            return Ok(());
        };
        if level > log_level(&cell) {
            return Ok(());
        }
        let mut message = String::new();
        if let Some(location) = lua.inspect_stack(1, |debug| {
            let source = debug.source();
            let name = source.short_src.as_deref().unwrap_or("?").to_owned();
            debug.current_line().map(|line| format!("{name}:{line}: "))
        }) {
            message.push_str(&location.unwrap_or_default());
        }
        for (index, arg) in args.iter().enumerate() {
            match arg {
                Value::Nil => message.push_str("nil"),
                Value::Boolean(value) => message.push_str(if *value { "true" } else { "false" }),
                Value::LightUserData(data) if data.0.is_null() => message.push_str("null"),
                other => match bytes(other) {
                    Some(text) => message.push_str(&String::from_utf8_lossy(&text)),
                    None => {
                        return Err(mlua::Error::runtime(format!(
                            "bad argument #{} to 'log' (string, number, boolean, or nil expected, got {})",
                            index + if print { 1 } else { 2 },
                            other.type_name()
                        )));
                    }
                },
            }
        }
        cell.exchange.lock().log(level, message);
        Ok(())
    })
}

fn sleep(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<mlua::Function> {
    let slot = Arc::clone(slot);
    lua.create_async_function(move |_, seconds: f64| {
        let allowed = cell(&slot, Api::Sleep).map(|_| ());
        async move {
            allowed?;
            if !seconds.is_finite() || seconds < 0.0 {
                return Err(mlua::Error::runtime(
                    "bad argument #1 to 'sleep' (invalid sleep time)",
                ));
            }
            tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
            Ok(())
        }
    })
}

fn get_phase(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<mlua::Function> {
    let slot = Arc::clone(slot);
    lua.create_function(move |_, ()| {
        Ok(slot
            .cell()
            .map_or(Phase::Init, |cell| cell.exchange.lock().phase)
            .name())
    })
}

fn config(lua: &Lua) -> mlua::Result<Table> {
    let config = lua.create_table()?;
    config.raw_set("subsystem", "http")?;
    config.raw_set("debug", false)?;
    config.raw_set("nginx_version", NGINX_VERSION)?;
    config.raw_set("ngx_lua_version", NGX_LUA_VERSION)?;
    config.raw_set("prefix", lua.create_function(|_, ()| Ok(""))?)?;
    config.raw_set("nginx_configure", lua.create_function(|_, ()| Ok(""))?)?;
    Ok(config)
}

fn worker(lua: &Lua, context: &Context) -> mlua::Result<Table> {
    let worker = lua.create_table()?;
    let (id, count) = (context.worker as i64, context.workers as i64);
    let pid = i64::from(std::process::id());
    worker.raw_set("id", lua.create_function(move |_, ()| Ok(id))?)?;
    worker.raw_set("count", lua.create_function(move |_, ()| Ok(count))?)?;
    worker.raw_set("pid", lua.create_function(move |_, ()| Ok(pid))?)?;
    worker.raw_set("pids", lua.create_function(move |_, ()| Ok(vec![pid]))?)?;
    worker.raw_set("exiting", lua.create_function(|_, ()| Ok(false))?)?;
    Ok(worker)
}
