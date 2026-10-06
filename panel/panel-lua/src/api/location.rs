//! `ngx.location.capture` and `ngx.location.capture_multi`: subrequests to
//! other routes of the request's site, which the gateway handles as it
//! handles requests, without the access phase, and whose responses come
//! back to the script.

use super::{bytes, cell, codec::encode_args, ngx::METHODS, results, Api};
use crate::{
    capture::{Capture, Captured, Share, MOST_CAPTURES},
    vm::{Cell, HostCall, HostReply, Slot},
};
use bytes::Bytes;
use http::Method;
use mlua::{Function, Lua, LuaString, MultiValue, Table, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

pub(super) fn install(lua: &Lua, ngx: &Table, slot: &Arc<Slot>) -> mlua::Result<()> {
    let location: Table = ngx.raw_get("location")?;
    location.raw_set("capture", capture(lua, slot, false)?)?;
    location.raw_set("capture_multi", capture(lua, slot, true)?)?;
    Ok(())
}

fn option<T: mlua::FromLua>(options: Option<&Table>, name: &str) -> mlua::Result<Option<T>> {
    match options {
        Some(options) => options.raw_get(name),
        None => Ok(None),
    }
}

/// The subrequest `uri` and `options` describe, as lua-nginx-module reads
/// them.
fn request(cell: &Cell, slot: &Slot, uri: &[u8], options: Option<&Table>) -> mlua::Result<Capture> {
    let uri = std::str::from_utf8(uri)
        .map_err(|_| mlua::Error::runtime("the subrequest uri is not UTF-8"))?;
    if !uri.starts_with('/') {
        return Err(mlua::Error::runtime(format!(
            "the subrequest uri {uri:?} is not an absolute path"
        )));
    }
    let (path, query) = match uri.split_once('?') {
        Some((path, query)) => (path, Some(query.to_owned())),
        None => (uri, None),
    };
    let method = match option::<i64>(options, "method")? {
        None => Method::GET,
        Some(code) => {
            let name = METHODS
                .iter()
                .find(|(_, known)| *known == code)
                .map(|(name, _)| *name)
                .ok_or_else(|| mlua::Error::runtime(format!("unsupported HTTP method: {code}")))?;
            Method::from_bytes(name.as_bytes())
                .map_err(|_| mlua::Error::runtime(format!("unsupported HTTP method: {code}")))?
        }
    };
    let args = match option::<Value>(options, "args")? {
        None | Some(Value::Nil) => None,
        Some(Value::Table(table)) => {
            Some(String::from_utf8_lossy(&encode_args(&table)?).into_owned())
        }
        Some(other) => Some(
            bytes(&other)
                .map(|text| String::from_utf8_lossy(&text).into_owned())
                .ok_or_else(|| {
                    mlua::Error::runtime(format!(
                        "Bad args option value: {} (string or table expected)",
                        other.type_name()
                    ))
                })?,
        ),
    };
    let args = match (query, args) {
        (Some(query), Some(args)) if !args.is_empty() => Some(format!("{query}&{args}")),
        (Some(query), _) => Some(query),
        (None, args) => args,
    };
    let exchange = cell.exchange.lock();
    let body = match option::<LuaString>(options, "body")? {
        Some(body) => Some(Bytes::copy_from_slice(&body.as_bytes())),
        None => {
            let forward = option::<bool>(options, "always_forward_body")?.unwrap_or(false)
                || method == Method::POST
                || method == Method::PUT;
            if forward {
                exchange.request.body.clone()
            } else {
                None
            }
        }
    };
    let share_variables = option::<bool>(options, "share_all_vars")?.unwrap_or(false);
    let mut variables =
        if share_variables || option::<bool>(options, "copy_all_vars")?.unwrap_or(false) {
            exchange.variables.clone()
        } else {
            Default::default()
        };
    drop(exchange);
    if let Some(vars) = option::<Table>(options, "vars")? {
        for pair in vars.pairs::<LuaString, Value>() {
            let (name, value) = pair?;
            let value = bytes(&value).ok_or_else(|| {
                mlua::Error::runtime(format!(
                    "Bad vars option value: {} (string or number expected)",
                    value.type_name()
                ))
            })?;
            variables.insert(
                name.to_str()?.to_ascii_lowercase(),
                String::from_utf8_lossy(&value).into_owned(),
            );
        }
    }
    let share = option::<Table>(options, "ctx")?.map(|ctx| Share {
        vm: slot.index,
        ctx,
    });
    Ok(Capture {
        method,
        path: path.to_owned(),
        args,
        body,
        variables,
        share_variables,
        share,
    })
}

/// A header name as nginx sends it: `Content-Type`, `X-Request-Id`.
fn titled(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// `res`: the response as a table, its header looked up whatever the case
/// of the name.
fn response(lua: &Lua, captured: Captured) -> mlua::Result<Table> {
    let header = lua.create_table()?;
    for name in captured.headers.keys() {
        let values: Vec<LuaString> = captured
            .headers
            .get_all(name)
            .iter()
            .map(|value| lua.create_string(value.as_bytes()))
            .collect::<mlua::Result<_>>()?;
        let key = titled(name.as_str());
        match values.len() {
            1 => header.raw_set(key, values.into_iter().next())?,
            _ => header.raw_set(key, lua.create_sequence_from(values)?)?,
        }
    }
    let lookup = lua.create_table()?;
    lookup.raw_set(
        "__index",
        lua.create_function(|_, (table, name): (Table, LuaString)| {
            let name = name.to_str()?.replace('_', "-");
            table.raw_get::<Value>(titled(&name.to_ascii_lowercase()))
        })?,
    )?;
    header.set_metatable(Some(lookup))?;
    let res = lua.create_table()?;
    res.raw_set("status", captured.status)?;
    res.raw_set("header", header)?;
    res.raw_set("body", lua.create_string(&captured.body)?)?;
    res.raw_set("truncated", captured.truncated)?;
    Ok(res)
}

fn capture(lua: &Lua, slot: &Arc<Slot>, multi: bool) -> mlua::Result<Function> {
    let slot = Arc::clone(slot);
    lua.create_async_function(move |lua, args: MultiValue| {
        let api = if multi {
            Api::LocationCaptureMulti
        } else {
            Api::LocationCapture
        };
        let call = cell(&slot, api).and_then(|cell| {
            let mut args = args.into_iter();
            let requests = if multi {
                let Some(Value::Table(list)) = args.next() else {
                    return Err(mlua::Error::runtime(
                        "only one argument is expected, a table of subrequests",
                    ));
                };
                let mut requests = Vec::new();
                for entry in list.sequence_values::<Table>() {
                    let entry = entry?;
                    let uri: LuaString = entry.raw_get(1)?;
                    let options: Option<Table> = entry.raw_get(2)?;
                    requests.push(request(&cell, &slot, &uri.as_bytes(), options.as_ref())?);
                }
                if requests.is_empty() {
                    return Err(mlua::Error::runtime(
                        "at least one subrequest should be specified",
                    ));
                }
                requests
            } else {
                let Some(Value::String(uri)) = args.next() else {
                    return Err(mlua::Error::runtime("the subrequest uri must be a string"));
                };
                let options = match args.next() {
                    None | Some(Value::Nil) => None,
                    Some(Value::Table(options)) => Some(options),
                    Some(other) => {
                        return Err(mlua::Error::runtime(format!(
                            "Bad options argument: {} (table expected)",
                            other.type_name()
                        )))
                    }
                };
                vec![request(&cell, &slot, &uri.as_bytes(), options.as_ref())?]
            };
            if requests.len() > MOST_CAPTURES {
                return Err(mlua::Error::runtime(format!(
                    "more than {MOST_CAPTURES} subrequests at once"
                )));
            }
            let shared: Vec<bool> = requests
                .iter()
                .map(|request| request.share_variables)
                .collect();
            let (reply, answer) = oneshot::channel();
            cell.run.lock().call = Some((HostCall::Capture(requests), reply));
            Ok((cell, shared, answer))
        });
        async move {
            let (cell, shared, answer) = call?;
            let Ok(HostReply::Captured(captured)) = answer.await else {
                return Err(mlua::Error::runtime("the subrequests could not be made"));
            };
            let captured = captured.map_err(mlua::Error::runtime)?;
            let mut values = Vec::with_capacity(captured.len());
            for (mut response_of, share) in captured.into_iter().zip(shared) {
                if share {
                    if let Some(variables) = response_of.variables.take() {
                        cell.exchange.lock().variables.extend(variables);
                    }
                }
                values.push(Value::Table(response(&lua, response_of)?));
            }
            Ok(results(values))
        }
    })
}
