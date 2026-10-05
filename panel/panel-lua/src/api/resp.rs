//! The response: `ngx.header`, `ngx.resp`, `ngx.status`, output with
//! `ngx.say` and `ngx.print`, `ngx.exit`, `ngx.redirect` and `ngx.arg`.

use super::{bytes, cell, exchange, require_permission, Api};
use crate::{
    exchange::{Exit, LogLevel, Phase},
    vm::{refused, Slot},
};
use http::{header, HeaderMap, HeaderName, HeaderValue};
use mlua::{Lua, LuaString, Table, Value, Variadic};
use std::{future, sync::Arc};

/// Bytes a handler may print; the response is held in memory until sent.
const MAX_OUTPUT: usize = 16 << 20;
/// Header fields `get_headers` returns by default.
pub(crate) const MAX_HEADERS: usize = 100;

pub(crate) fn header_name(key: &[u8], underscores: bool) -> mlua::Result<HeaderName> {
    let normalized: Vec<u8> = key
        .iter()
        .map(|&byte| {
            if underscores && byte == b'_' {
                b'-'
            } else {
                byte
            }
        })
        .collect();
    HeaderName::from_bytes(&normalized).map_err(|_| {
        mlua::Error::runtime(format!(
            "invalid header name \"{}\"",
            String::from_utf8_lossy(key)
        ))
    })
}

fn header_value(value: &Value) -> mlua::Result<HeaderValue> {
    let text = match value {
        Value::Boolean(value) => Some(value.to_string().into_bytes()),
        other => bytes(other),
    };
    let Some(text) = text else {
        return Err(mlua::Error::runtime(format!(
            "invalid header value of type {}",
            value.type_name()
        )));
    };
    HeaderValue::from_bytes(&text).map_err(|_| {
        mlua::Error::runtime(
            "invalid header value: field values cannot hold CR, LF or NUL (RFC 9110 §5.5)",
        )
    })
}

/// The lines a Lua value sets a field to: none for `nil`, one per element of
/// a table.
pub(crate) fn header_values(value: &Value) -> mlua::Result<Vec<HeaderValue>> {
    match value {
        Value::Nil => Ok(Vec::new()),
        Value::Table(table) => table
            .sequence_values::<Value>()
            .map(|value| header_value(&value?))
            .collect(),
        other => Ok(vec![header_value(other)?]),
    }
}

/// Replaces every line of `name` with `values`.
pub(crate) fn set_header(headers: &mut HeaderMap, name: HeaderName, values: Vec<HeaderValue>) {
    headers.remove(&name);
    for value in values {
        headers.append(name.clone(), value);
    }
}

/// A field's lines as Lua gives them: a string for one, a table for more.
pub(crate) fn header_lua(lua: &Lua, headers: &HeaderMap, name: &HeaderName) -> mlua::Result<Value> {
    let mut values = headers.get_all(name).iter();
    let Some(first) = values.next() else {
        return Ok(Value::Nil);
    };
    let rest: Vec<&HeaderValue> = values.collect();
    if rest.is_empty() {
        return Ok(Value::String(lua.create_string(first.as_bytes())?));
    }
    let table = lua.create_table_with_capacity(rest.len() + 1, 0)?;
    table.raw_push(lua.create_string(first.as_bytes())?)?;
    for value in rest {
        table.raw_push(lua.create_string(value.as_bytes())?)?;
    }
    Ok(Value::Table(table))
}

/// A metatable that looks keys up lower-cased and with `_` as `-`, as
/// `get_headers` results do.
pub(crate) fn headers_meta(lua: &Lua) -> mlua::Result<Table> {
    let meta = lua.create_table()?;
    meta.raw_set(
        "__index",
        lua.create_function(|_, (table, key): (Table, Value)| {
            let Value::String(key) = key else {
                return Ok(Value::Nil);
            };
            let normalized: Vec<u8> = key
                .as_bytes()
                .iter()
                .map(|&byte| match byte {
                    b'_' => b'-',
                    other => other.to_ascii_lowercase(),
                })
                .collect();
            if normalized[..] == key.as_bytes()[..] {
                return Ok(Value::Nil);
            }
            table.raw_get::<Value>(mlua::BString::from(normalized))
        })?,
    )?;
    Ok(meta)
}

/// Header fields as `get_headers` returns them, and whether there were more
/// than `max` (zero for all).
pub(crate) fn headers_table(
    lua: &Lua,
    headers: &HeaderMap,
    max: usize,
    raw: bool,
    meta: &Table,
) -> mlua::Result<(Table, bool)> {
    let table = lua.create_table()?;
    let mut count = 0;
    let mut truncated = false;
    for name in headers.keys() {
        let lines = headers.get_all(name).iter().count();
        if max != 0 && count + lines > max {
            truncated = true;
            break;
        }
        count += lines;
        table.raw_set(name.as_str(), header_lua(lua, headers, name)?)?;
    }
    if !raw {
        table.set_metatable(Some(meta.clone()))?;
    }
    Ok((table, truncated))
}

pub(super) fn header_proxy(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let proxy = lua.create_table()?;
    let meta = lua.create_table()?;
    let reader = Arc::clone(slot);
    meta.raw_set(
        "__index",
        lua.create_function(move |lua, (_, key): (Table, LuaString)| {
            let name = header_name(&key.as_bytes(), true)?;
            exchange(&reader, Api::Header, |exchange| {
                header_lua(lua, &exchange.response.headers, &name)
            })
        })?,
    )?;
    let writer = Arc::clone(slot);
    meta.raw_set(
        "__newindex",
        lua.create_function(move |_, (_, key, value): (Table, LuaString, Value)| {
            let name = header_name(&key.as_bytes(), true)?;
            let values = header_values(&value)?;
            exchange(&writer, Api::Header, |exchange| {
                if exchange.headers_sent && exchange.phase != Phase::HeaderFilter {
                    exchange.log(
                        LogLevel::Err,
                        format!(
                            "attempt to set ngx.header.{} after sending out response headers",
                            name.as_str()
                        ),
                    );
                    return Ok(());
                }
                set_header(&mut exchange.response.headers, name, values);
                exchange.changes.response_headers = true;
                Ok(())
            })
        })?,
    )?;
    proxy.set_metatable(Some(meta))?;
    Ok(proxy)
}

pub(super) fn resp_table(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let resp = lua.create_table()?;
    let meta = headers_meta(lua)?;
    let slot = Arc::clone(slot);
    resp.raw_set(
        "get_headers",
        lua.create_function(move |lua, (max, raw): (Option<usize>, Option<bool>)| {
            exchange(&slot, Api::RespGetHeaders, |exchange| {
                let (table, truncated) = headers_table(
                    lua,
                    &exchange.response.headers,
                    max.unwrap_or(MAX_HEADERS),
                    raw.unwrap_or(false),
                    &meta,
                )?;
                super::maybe_truncated(lua, table, truncated)
            })
        })?,
    )?;
    Ok(resp)
}

pub(super) fn set_status(slot: &Slot, value: &Value) -> mlua::Result<()> {
    let status = match value {
        Value::Integer(status) => *status,
        Value::Number(status) if status.fract() == 0.0 => *status as i64,
        Value::String(text) => text
            .to_str()
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0),
        _ => 0,
    };
    let Ok(status) = u16::try_from(status) else {
        return Err(mlua::Error::runtime("ngx.status must be a status code"));
    };
    if http::StatusCode::from_u16(status).is_err() {
        return Err(mlua::Error::runtime(format!(
            "ngx.status {status} is not a status code"
        )));
    }
    exchange(slot, Api::Status, |exchange| {
        if exchange.headers_sent && exchange.phase != Phase::HeaderFilter {
            exchange.log(
                LogLevel::Err,
                "attempt to set ngx.status after sending out response headers".into(),
            );
            return Ok(());
        }
        exchange.response.status = status;
        exchange.changes.status = true;
        Ok(())
    })
}

fn output(value: &Value, into: &mut Vec<u8>, depth: usize) -> mlua::Result<()> {
    match value {
        Value::Nil => into.extend_from_slice(b"nil"),
        Value::Boolean(value) => into.extend_from_slice(if *value { b"true" } else { b"false" }),
        Value::LightUserData(data) if data.0.is_null() => into.extend_from_slice(b"null"),
        Value::Table(table) if depth < 32 => {
            for element in table.sequence_values::<Value>() {
                output(&element?, into, depth + 1)?;
            }
        }
        other => match bytes(other) {
            Some(text) => into.extend_from_slice(&text),
            None => {
                return Err(mlua::Error::runtime(format!(
                    "bad argument to 'say' (string, number, boolean, nil, ngx.null, or array table expected, got {})",
                    other.type_name()
                )));
            }
        },
    }
    if into.len() > MAX_OUTPUT {
        return Err(mlua::Error::runtime("the response printed is too large"));
    }
    Ok(())
}

fn print(slot: &Slot, args: &[Value], newline: bool) -> mlua::Result<i64> {
    let mut text = Vec::new();
    for arg in args {
        output(arg, &mut text, 0)?;
    }
    if newline {
        text.push(b'\n');
    }
    exchange(slot, Api::Output, |exchange| {
        if exchange.exit.is_some() {
            return Err(mlua::Error::runtime("seen eof"));
        }
        if exchange.response.body.len() + text.len() > MAX_OUTPUT {
            return Err(mlua::Error::runtime("the response printed is too large"));
        }
        start_response(exchange);
        exchange.response.body.extend_from_slice(&text);
        Ok(1)
    })
}

fn start_response(exchange: &mut crate::exchange::Exchange) {
    if !exchange.headers_sent {
        exchange.headers_sent = true;
        if exchange.response.status == 0 {
            exchange.response.status = 200;
        }
        exchange.changes.status = true;
        exchange.changes.response_headers = true;
    }
}

/// How `ngx.exit(status)` ends the phase it is called in.
fn exit(slot: &Slot, status: i64) -> mlua::Result<()> {
    exchange(slot, Api::Exit, |exchange| {
        let phase = exchange.phase;
        let end = match status {
            0 | -5 => Exit::Phase,
            -1 | 444 => Exit::Abort,
            200..=999 if phase == Phase::HeaderFilter => {
                exchange.response.status = u16::try_from(status).unwrap_or(500);
                exchange.changes.status = true;
                Exit::Phase
            }
            200..=999 => {
                if !exchange.headers_sent {
                    exchange.response.status = u16::try_from(status).unwrap_or(500);
                    exchange.changes.status = true;
                }
                Exit::Respond
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "bad argument #1 to 'exit' (bad status {other})"
                )));
            }
        };
        exchange.exit = Some(end);
        Ok(())
    })
}

fn redirect(slot: &Slot, location: &[u8], status: Option<i64>) -> mlua::Result<()> {
    let status = status.unwrap_or(302);
    if ![301, 302, 303, 307, 308].contains(&status) {
        return Err(mlua::Error::runtime(format!(
            "only ngx.HTTP_MOVED_TEMPORARILY, ngx.HTTP_MOVED_PERMANENTLY, ngx.HTTP_PERMANENT_REDIRECT, ngx.HTTP_SEE_OTHER, and ngx.HTTP_TEMPORARY_REDIRECT are allowed, got {status}"
        )));
    }
    let location = HeaderValue::from_bytes(location).map_err(|_| {
        mlua::Error::runtime("invalid redirect location: it cannot hold CR, LF or NUL")
    })?;
    exchange(slot, Api::Redirect, |exchange| {
        if exchange.headers_sent {
            return Err(mlua::Error::runtime(
                "attempt to call ngx.redirect after sending out the headers",
            ));
        }
        exchange.response.status = u16::try_from(status).unwrap_or(302);
        exchange.response.headers.insert(header::LOCATION, location);
        exchange.changes.status = true;
        exchange.changes.response_headers = true;
        exchange.exit = Some(Exit::Respond);
        Ok(())
    })
}

pub(super) fn install(lua: &Lua, ngx: &Table, slot: &Arc<Slot>) -> mlua::Result<()> {
    let say = Arc::clone(slot);
    ngx.raw_set(
        "say",
        lua.create_function(move |_, args: Variadic<Value>| print(&say, &args, true))?,
    )?;
    let printer = Arc::clone(slot);
    ngx.raw_set(
        "print",
        lua.create_function(move |_, args: Variadic<Value>| print(&printer, &args, false))?,
    )?;
    let flush = Arc::clone(slot);
    ngx.raw_set(
        "flush",
        lua.create_function(move |_, _: Option<bool>| {
            exchange(&flush, Api::Flush, |exchange| {
                start_response(exchange);
                Ok(1)
            })
        })?,
    )?;
    let send_headers = Arc::clone(slot);
    ngx.raw_set(
        "send_headers",
        lua.create_function(move |_, ()| {
            exchange(&send_headers, Api::SendHeaders, |exchange| {
                start_response(exchange);
                Ok(1)
            })
        })?,
    )?;
    let eof = Arc::clone(slot);
    ngx.raw_set(
        "eof",
        lua.create_function(move |_, ()| {
            exchange(&eof, Api::Eof, |exchange| {
                start_response(exchange);
                exchange.exit.get_or_insert(Exit::Respond);
                Ok(1)
            })
        })?,
    )?;
    // `ngx.exit` and `ngx.redirect` never return: their phase handler ends.
    let exiter = Arc::clone(slot);
    ngx.raw_set(
        "exit",
        lua.create_async_function(move |_, status: i64| {
            let ended = exit(&exiter, status);
            async move {
                ended?;
                future::pending::<mlua::Result<()>>().await
            }
        })?,
    )?;
    let redirector = Arc::clone(slot);
    ngx.raw_set(
        "redirect",
        lua.create_async_function(move |_, (location, status): (LuaString, Option<i64>)| {
            let ended = redirect(&redirector, &location.as_bytes(), status);
            async move {
                ended?;
                future::pending::<mlua::Result<()>>().await
            }
        })?,
    )?;
    Ok(())
}

/// `ngx.arg[1]`, the body chunk, and `ngx.arg[2]`, whether it is the last.
pub(super) fn arg_proxy(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let proxy = lua.create_table()?;
    let meta = lua.create_table()?;
    let reader = Arc::clone(slot);
    meta.raw_set(
        "__index",
        lua.create_function(move |lua, (_, index): (Table, i64)| {
            let cell = cell(&reader, Api::Arg)?;
            require_permission(&cell, Api::Arg, |granted| granted.body, "body")?;
            let exchange = cell.exchange.lock();
            Ok(match index {
                1 => Value::String(lua.create_string(&exchange.chunk.data)?),
                2 => Value::Boolean(exchange.chunk.eof),
                _ => Value::Nil,
            })
        })?,
    )?;
    let writer = Arc::clone(slot);
    meta.raw_set(
        "__newindex",
        lua.create_function(move |_, (_, index, value): (Table, i64, Value)| {
            let cell = cell(&writer, Api::Arg)?;
            require_permission(&cell, Api::Arg, |granted| granted.body, "body")?;
            let mut exchange = cell.exchange.lock();
            match index {
                1 => {
                    let mut data = Vec::new();
                    if !value.is_nil() {
                        output(&value, &mut data, 0)?;
                    }
                    exchange.chunk.data = data.into();
                }
                2 => exchange.chunk.eof = value.as_boolean().unwrap_or(false),
                _ => return Err(refused("ngx.arg only has [1] and [2]")),
            }
            exchange.changes.chunk = true;
            Ok(())
        })?,
    )?;
    proxy.set_metatable(Some(meta))?;
    Ok(proxy)
}
