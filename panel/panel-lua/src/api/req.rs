//! `ngx.req`: the request line, its arguments, header and body.

use super::{
    bytes, cell,
    codec::{args_table, encode_args, parse_args},
    exchange, failed, maybe_truncated, require_permission,
    resp::{header_name, header_values, headers_meta, headers_table, set_header, MAX_HEADERS},
    Api,
};
use crate::vm::{refused, HostCall, HostReply, Slot};
use mlua::{Lua, LuaString, Table, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Bytes `ngx.req.read_body` reads at most; the gateway's own limits on
/// request bodies apply as well.
pub(crate) const MAX_BODY: usize = 16 << 20;

fn method_name(method: i64) -> Option<&'static str> {
    super::ngx::METHODS
        .iter()
        .find(|(_, flag)| *flag == method)
        .map(|(name, _)| *name)
}

pub(super) fn table(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let req = lua.create_table()?;
    let meta = headers_meta(lua)?;

    let s = Arc::clone(slot);
    req.raw_set(
        "get_method",
        lua.create_function(move |_, ()| {
            exchange(&s, Api::ReqGetMethod, |exchange| {
                Ok(exchange.request.method.clone())
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "set_method",
        lua.create_function(move |_, method: i64| {
            let Some(name) = method_name(method) else {
                return Err(mlua::Error::runtime(format!(
                    "bad argument #1 to 'set_method' (unsupported HTTP method: {method})"
                )));
            };
            exchange(&s, Api::ReqSetMethod, |exchange| {
                exchange.request.method = name.to_owned();
                exchange.changes.method = true;
                Ok(())
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "get_uri_args",
        lua.create_function(move |lua, max: Option<usize>| {
            let query = exchange(&s, Api::ReqGetUriArgs, |exchange| {
                Ok(exchange.request.args.clone().unwrap_or_default())
            })?;
            let (arguments, truncated) = parse_args(query.as_bytes(), max.unwrap_or(100));
            maybe_truncated(lua, args_table(lua, arguments)?, truncated)
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "set_uri_args",
        lua.create_function(move |_, args: Value| {
            let query = match &args {
                Value::Table(table) => encode_args(table)?,
                other => bytes(other).ok_or_else(|| {
                    mlua::Error::runtime(
                        "bad argument #1 to 'set_uri_args' (string or table expected)",
                    )
                })?,
            };
            let query = String::from_utf8(query)
                .map_err(|_| mlua::Error::runtime("the query must be ASCII once escaped"))?;
            exchange(&s, Api::ReqSetUriArgs, |exchange| {
                exchange.request.args = (!query.is_empty()).then_some(query);
                exchange.changes.args = true;
                Ok(())
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "get_post_args",
        lua.create_function(move |lua, max: Option<usize>| {
            let cell = cell(&s, Api::ReqGetPostArgs)?;
            require_permission(&cell, Api::ReqGetPostArgs, |granted| granted.body, "body")?;
            let body = cell.exchange.lock().request.body.clone();
            let Some(body) = body else {
                return failed(lua, 1, "no request body found");
            };
            let (arguments, truncated) = parse_args(&body, max.unwrap_or(100));
            maybe_truncated(lua, args_table(lua, arguments)?, truncated)
        })?,
    )?;
    let s = Arc::clone(slot);
    let get_meta = meta.clone();
    req.raw_set(
        "get_headers",
        lua.create_function(move |lua, (max, raw): (Option<usize>, Option<bool>)| {
            exchange(&s, Api::ReqGetHeaders, |exchange| {
                let (table, truncated) = headers_table(
                    lua,
                    &exchange.request.headers,
                    max.unwrap_or(MAX_HEADERS),
                    raw.unwrap_or(false),
                    &get_meta,
                )?;
                maybe_truncated(lua, table, truncated)
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "set_header",
        lua.create_function(move |_, (name, value): (LuaString, Value)| {
            let name = header_name(&name.as_bytes(), false)?;
            let values = header_values(&value)?;
            exchange(&s, Api::ReqSetHeader, |exchange| {
                set_header(&mut exchange.request.headers, name, values);
                exchange.changes.headers = true;
                Ok(())
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "clear_header",
        lua.create_function(move |_, name: LuaString| {
            let name = header_name(&name.as_bytes(), false)?;
            exchange(&s, Api::ReqClearHeader, |exchange| {
                exchange.request.headers.remove(&name);
                exchange.changes.headers = true;
                Ok(())
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "read_body",
        lua.create_async_function(move |_, ()| {
            let call = cell(&s, Api::ReqReadBody).and_then(|cell| {
                require_permission(&cell, Api::ReqReadBody, |granted| granted.body, "body")?;
                if cell.exchange.lock().request.body.is_some() {
                    return Ok(None);
                }
                let (reply, answer) = oneshot::channel();
                cell.run.lock().call = Some((HostCall::ReadBody { limit: MAX_BODY }, reply));
                Ok(Some((cell, answer)))
            });
            async move {
                let Some((cell, answer)) = call? else {
                    return Ok(());
                };
                let Ok(HostReply::Body(body)) = answer.await else {
                    return Err(mlua::Error::runtime("the request body could not be read"));
                };
                let body = body.map_err(mlua::Error::runtime)?;
                cell.exchange.lock().request.body = Some(body);
                Ok(())
            }
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "discard_body",
        lua.create_function(move |_, ()| {
            cell(&s, Api::ReqDiscardBody)?;
            Ok(())
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "get_body_data",
        lua.create_function(move |lua, max: Option<usize>| {
            let cell = cell(&s, Api::ReqGetBodyData)?;
            require_permission(&cell, Api::ReqGetBodyData, |granted| granted.body, "body")?;
            // nil for a body that is not read or is empty, as in
            // lua-nginx-module.
            let body = cell.exchange.lock().request.body.clone();
            body.filter(|body| !body.is_empty())
                .map(|body| {
                    let end = max
                        .filter(|max| *max > 0)
                        .map_or(body.len(), |max| max.min(body.len()));
                    lua.create_string(&body[..end])
                })
                .transpose()
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "set_body_data",
        lua.create_function(move |_, data: LuaString| {
            let cell = cell(&s, Api::ReqSetBodyData)?;
            require_permission(&cell, Api::ReqSetBodyData, |granted| granted.body, "body")?;
            let mut exchange = cell.exchange.lock();
            if exchange.request.body.is_none() {
                return Err(mlua::Error::runtime(
                    "request body not read yet: call ngx.req.read_body first",
                ));
            }
            exchange.request.body = Some(::bytes::Bytes::copy_from_slice(&data.as_bytes()));
            exchange.changes.body = true;
            Ok(())
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "init_body",
        lua.create_function(move |_, size: Option<usize>| {
            let cell = cell(&s, Api::ReqInitBody)?;
            require_permission(&cell, Api::ReqInitBody, |granted| granted.body, "body")?;
            let mut exchange = cell.exchange.lock();
            if exchange.request.body.is_none() {
                return Err(mlua::Error::runtime(
                    "request body not read yet: call ngx.req.read_body first",
                ));
            }
            exchange.new_body = Some(Vec::with_capacity(size.unwrap_or(0).min(MAX_BODY)));
            Ok(())
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "append_body",
        lua.create_function(move |_, data: LuaString| {
            let cell = cell(&s, Api::ReqAppendBody)?;
            require_permission(&cell, Api::ReqAppendBody, |granted| granted.body, "body")?;
            let mut exchange = cell.exchange.lock();
            let Some(body) = exchange.new_body.as_mut() else {
                return Err(mlua::Error::runtime("request body not initialized"));
            };
            let data = data.as_bytes();
            if body.len() + data.len() > MAX_BODY {
                return Err(mlua::Error::runtime(
                    "the new request body is larger than 16 MiB",
                ));
            }
            body.extend_from_slice(&data);
            Ok(())
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "finish_body",
        lua.create_function(move |_, ()| {
            let cell = cell(&s, Api::ReqFinishBody)?;
            require_permission(&cell, Api::ReqFinishBody, |granted| granted.body, "body")?;
            let mut exchange = cell.exchange.lock();
            let Some(body) = exchange.new_body.take() else {
                return Err(mlua::Error::runtime("request body not initialized"));
            };
            exchange.request.body = Some(::bytes::Bytes::from(body));
            exchange.changes.body = true;
            Ok(())
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "get_body_file",
        lua.create_function(move |_, ()| {
            let cell = cell(&s, Api::ReqGetBodyFile)?;
            require_permission(&cell, Api::ReqGetBodyFile, |granted| granted.body, "body")?;
            // Bodies are kept in memory, never in a file.
            Ok(Value::Nil)
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "http_version",
        lua.create_function(move |_, ()| {
            exchange(&s, Api::ReqHttpVersion, |exchange| {
                Ok(match exchange.request.version {
                    http::Version::HTTP_09 => 0.9,
                    http::Version::HTTP_10 => 1.0,
                    http::Version::HTTP_2 => 2.0,
                    http::Version::HTTP_3 => 3.0,
                    _ => 1.1,
                })
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "raw_header",
        lua.create_function(move |lua, no_request_line: Option<bool>| {
            exchange(&s, Api::ReqRawHeader, |exchange| {
                let request = &exchange.request;
                let mut raw = Vec::new();
                if !no_request_line.unwrap_or(false) {
                    let version = match request.version {
                        http::Version::HTTP_10 => "HTTP/1.0",
                        http::Version::HTTP_2 => "HTTP/2.0",
                        http::Version::HTTP_3 => "HTTP/3.0",
                        _ => "HTTP/1.1",
                    };
                    raw.extend_from_slice(
                        format!("{} {} {version}\r\n", request.method, request.request_uri)
                            .as_bytes(),
                    );
                }
                for (name, value) in &request.headers {
                    raw.extend_from_slice(name.as_str().as_bytes());
                    raw.extend_from_slice(b": ");
                    raw.extend_from_slice(value.as_bytes());
                    raw.extend_from_slice(b"\r\n");
                }
                raw.extend_from_slice(b"\r\n");
                lua.create_string(raw)
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "start_time",
        lua.create_function(move |_, ()| {
            exchange(&s, Api::ReqStartTime, |exchange| {
                Ok(exchange
                    .connection
                    .started
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |since| since.as_millis() as f64 / 1000.0))
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "is_internal",
        lua.create_function(move |_, ()| {
            exchange(&s, Api::ReqIsInternal, |exchange| {
                Ok(exchange.internal || exchange.subrequest)
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    req.raw_set(
        "set_uri",
        lua.create_function(
            move |_, (uri, jump, _binary): (LuaString, Option<bool>, Option<bool>)| {
                let uri = uri.to_str().map_err(|_| {
                    mlua::Error::runtime("bad argument #1 to 'set_uri' (the URI must be UTF-8)")
                })?;
                if uri.is_empty() {
                    return Err(mlua::Error::runtime(
                        "bad argument #1 to 'set_uri' (uri should not be empty)",
                    ));
                }
                if uri.bytes().any(|byte| byte.is_ascii_control()) {
                    return Err(refused("the URI cannot hold control characters"));
                }
                exchange(&s, Api::ReqSetUri, |exchange| {
                    if jump.unwrap_or(false)
                        && !matches!(
                            exchange.phase,
                            crate::exchange::Phase::Rewrite | crate::exchange::Phase::ServerRewrite
                        )
                    {
                        return Err(refused(
                            "ngx.req.set_uri with jump only works in rewrite_by_lua*",
                        ));
                    }
                    exchange.request.uri = uri.to_owned();
                    exchange.changes.uri = true;
                    exchange.changes.jump |= jump.unwrap_or(false);
                    Ok(())
                })
            },
        )?,
    )?;
    Ok(req)
}
