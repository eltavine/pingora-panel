//! The globals scripts see: `ngx`, `require`, `print` and the built-in
//! modules, following lua-nginx-module's documentation.

mod bit;
mod codec;
pub(crate) mod contexts;
mod json;
mod modules;
mod ngx;
mod re;
mod req;
mod resp;
mod semaphore;
mod shared;
mod socket;
mod thread;
mod time;
mod udp;
mod var;

pub(crate) use contexts::Api;
pub(crate) use modules::BUILT_IN as BUILT_IN_MODULES;
pub(crate) use ngx::UNAVAILABLE;

use crate::{
    exchange::{Exchange, LogLevel, Permissions},
    program::Compiled,
    shared::Dict,
    vm::{refused, Cell, Slot},
};
use mlua::{Lua, Table, Value};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

/// What the globals of a VM are built from.
pub(crate) struct Context {
    pub slot: Arc<Slot>,
    pub modules: Arc<BTreeMap<String, Compiled>>,
    pub dicts: HashMap<String, Arc<Dict>>,
    pub worker: usize,
    pub workers: usize,
    pub timers: Arc<crate::timer::Timers>,
}

pub(crate) fn install(lua: &Lua, globals: &Table, context: &Context) -> mlua::Result<()> {
    for removed in ["getfenv", "setfenv"] {
        globals.raw_set(removed, Value::Nil)?;
    }
    protected_calls(lua, globals)?;
    let ngx = ngx::table(lua, context)?;
    globals.raw_set("ngx", ngx.clone())?;
    globals.raw_set("print", ngx.raw_get::<Value>("print_global")?)?;
    ngx.raw_set("print_global", Value::Nil)?;
    globals.raw_set("require", modules::require(lua, context, &ngx)?)?;
    freeze(&ngx);
    Ok(())
}

/// `pcall` and `xpcall` that hand scripts the message of an error raised by
/// a host function as a string, as a C function's error is in
/// lua-nginx-module, rather than the error object the VM carries it in.
fn protected_calls(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    let text = lua.create_function(|lua, error: Value| match error {
        Value::Error(error) => Ok(Value::String(
            lua.create_string(crate::runtime::failure(&error).message)?,
        )),
        other => Ok(other),
    })?;
    let wrap: mlua::Function = lua
        .load(
            r#"
            local raw_pcall, raw_xpcall, text = ...
            local function settle(ok, ...)
                if ok then
                    return true, ...
                end
                return false, text((...))
            end
            local function pcall(f, ...)
                return settle(raw_pcall(f, ...))
            end
            local function xpcall(f, handler, ...)
                return raw_xpcall(f, function(error)
                    return handler(text(error))
                end, ...)
            end
            return pcall, xpcall
            "#,
        )
        .set_name("=pcall")
        .into_function()?;
    let (pcall, xpcall): (mlua::Function, mlua::Function) = wrap.call((
        globals.raw_get::<mlua::Function>("pcall")?,
        globals.raw_get::<mlua::Function>("xpcall")?,
        text,
    ))?;
    globals.raw_set("pcall", pcall)?;
    globals.raw_set("xpcall", xpcall)?;
    Ok(())
}

/// Makes `table` and the tables it holds read-only, as the sandbox does for
/// the standard library.
fn freeze(table: &Table) {
    if table.is_readonly() {
        return;
    }
    table.set_readonly(true);
    if let Some(meta) = table.metatable() {
        freeze(&meta);
    }
    for (_, value) in table.pairs::<Value, Value>().flatten() {
        if let Value::Table(inner) = value {
            freeze(&inner);
        }
    }
}

/// The request whose code runs, if `api` may be called in its phase.
pub(crate) fn cell(slot: &Slot, api: Api) -> mlua::Result<Arc<Cell>> {
    let Some(cell) = slot.cell() else {
        return Err(refused(format!("{} has no request to act on", api.name())));
    };
    let phase = cell.exchange.lock().phase;
    if !api.allows(phase) {
        return Err(refused(format!(
            "API disabled in the context of {}",
            contexts::context(phase)
        )));
    }
    Ok(cell)
}

/// Runs `action` on the exchange of the request whose code runs. `action`
/// must not call back into Lua.
pub(crate) fn exchange<R>(
    slot: &Slot,
    api: Api,
    action: impl FnOnce(&mut Exchange) -> mlua::Result<R>,
) -> mlua::Result<R> {
    let cell = cell(slot, api)?;
    let mut exchange = cell.exchange.lock();
    action(&mut exchange)
}

/// Refuses unless the scripts were granted `wanted`.
pub(crate) fn require_permission(
    cell: &Cell,
    api: Api,
    wanted: fn(&Permissions) -> bool,
    name: &str,
) -> mlua::Result<()> {
    if wanted(&cell.run.lock().permissions) {
        Ok(())
    } else {
        Err(refused(format!(
            "{} needs the {name} permission (lua_allow {name})",
            api.name()
        )))
    }
}

/// The level messages must reach to be kept.
pub(crate) fn log_level(cell: &Cell) -> LogLevel {
    cell.run.lock().log_level.unwrap_or(LogLevel::Notice)
}

/// Results as lua-nginx-module returns them: exactly the values given.
pub(crate) fn results(values: impl IntoIterator<Item = Value>) -> mlua::MultiValue {
    values.into_iter().collect()
}

/// `nil` and an error message, as functions report failures.
pub(crate) fn failed(lua: &Lua, nils: usize, error: &str) -> mlua::Result<mlua::MultiValue> {
    let mut values = vec![Value::Nil; nils];
    values.push(Value::String(lua.create_string(error)?));
    Ok(results(values))
}

/// A table, followed by `"truncated"` when arguments or fields were left out.
pub(crate) fn maybe_truncated(
    lua: &Lua,
    table: Table,
    truncated: bool,
) -> mlua::Result<mlua::MultiValue> {
    let mut values = vec![Value::Table(table)];
    if truncated {
        values.push(Value::String(lua.create_string("truncated")?));
    }
    Ok(results(values))
}

/// Bytes of a string or number argument, as Lua's `tostring` gives them.
pub(crate) fn bytes(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::String(text) => Some(text.as_bytes().to_vec()),
        Value::Integer(number) => Some(number.to_string().into_bytes()),
        Value::Number(number) => Some(number_text(*number).into_bytes()),
        _ => None,
    }
}

/// A number as Lua 5.1 prints it (`%.14g`).
pub(crate) fn number_text(number: f64) -> String {
    if number.is_nan() {
        return if number.is_sign_negative() {
            "-nan"
        } else {
            "nan"
        }
        .into();
    }
    if number.is_infinite() {
        return if number < 0.0 { "-inf" } else { "inf" }.into();
    }
    if number == number.trunc() && number.abs() < 1e15 {
        return format!("{}", number as i64);
    }
    let formatted = format!("{number:.13e}");
    let (mantissa, exponent) = formatted.split_once('e').unwrap_or((&formatted, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if (-5..14).contains(&exponent) {
        let digits = usize::try_from(13 - exponent).unwrap_or(0);
        let fixed = format!("{number:.digits$}");
        trim_zeros(&fixed).to_owned()
    } else {
        let mantissa = trim_zeros(mantissa);
        format!(
            "{mantissa}e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        )
    }
}

fn trim_zeros(text: &str) -> &str {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::number_text;

    #[test]
    fn numbers_print_as_lua_prints_them() {
        assert_eq!(number_text(3.0), "3");
        assert_eq!(number_text(0.1), "0.1");
        assert_eq!(number_text(1.0 / 3.0), "0.33333333333333");
        assert_eq!(number_text(1e20), "1e+20");
        assert_eq!(number_text(-2.5e-7), "-2.5e-07");
    }
}
