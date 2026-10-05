//! `ngx.var`: nginx variables computed from the request, the ones the
//! gateway supplies, and those scripts set.

use super::{bytes, exchange, Api};
use crate::{exchange::Exchange, vm::Slot};
use chrono::{DateTime, Local};
use http::header;
use mlua::{Lua, LuaString, Table, Value};
use std::{net::IpAddr, sync::Arc, time::SystemTime};

/// Variables computed from the request; they cannot be set, but for `args`.
const BUILT_IN: [&str; 26] = [
    "uri",
    "document_uri",
    "request_uri",
    "args",
    "query_string",
    "is_args",
    "host",
    "remote_addr",
    "remote_port",
    "binary_remote_addr",
    "server_addr",
    "server_port",
    "server_name",
    "scheme",
    "https",
    "request_method",
    "request_id",
    "request",
    "server_protocol",
    "content_type",
    "content_length",
    "msec",
    "time_iso8601",
    "time_local",
    "request_time",
    "cookie",
];

fn protocol(version: http::Version) -> &'static str {
    match version {
        http::Version::HTTP_09 => "HTTP/0.9",
        http::Version::HTTP_10 => "HTTP/1.0",
        http::Version::HTTP_2 => "HTTP/2.0",
        http::Version::HTTP_3 => "HTTP/3.0",
        _ => "HTTP/1.1",
    }
}

/// The lines of a field joined as nginx joins them: `; ` for cookies,
/// `, ` for the rest (RFC 9110 §5.3).
fn field(exchange: &Exchange, name: &str) -> Option<Vec<u8>> {
    let name = header::HeaderName::from_bytes(name.replace('_', "-").as_bytes()).ok()?;
    let separator: &[u8] = if name == header::COOKIE { b"; " } else { b", " };
    let mut joined: Option<Vec<u8>> = None;
    for value in exchange.request.headers.get_all(&name) {
        match &mut joined {
            Some(joined) => {
                joined.extend_from_slice(separator);
                joined.extend_from_slice(value.as_bytes());
            }
            None => joined = Some(value.as_bytes().to_vec()),
        }
    }
    joined
}

fn cookie(exchange: &Exchange, name: &str) -> Option<Vec<u8>> {
    for line in exchange.request.headers.get_all(header::COOKIE) {
        for pair in line.as_bytes().split(|&byte| byte == b';') {
            let pair = pair.trim_ascii();
            let Some(at) = pair.iter().position(|&byte| byte == b'=') else {
                continue;
            };
            if pair[..at]
                .trim_ascii()
                .eq_ignore_ascii_case(name.as_bytes())
            {
                return Some(pair[at + 1..].trim_ascii().to_vec());
            }
        }
    }
    None
}

/// `$arg_name`: the first value of the argument, not unescaped; names match
/// ignoring case, as nginx matches them.
fn argument(exchange: &Exchange, name: &str) -> Option<Vec<u8>> {
    let query = exchange.request.args.as_deref()?;
    for part in query.split('&') {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        if key.eq_ignore_ascii_case(name) {
            return Some(value.as_bytes().to_vec());
        }
    }
    None
}

fn seconds(time: SystemTime) -> f64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_millis() as f64 / 1000.0)
}

pub(crate) fn variable(exchange: &Exchange, name: &str) -> Option<Vec<u8>> {
    let request = &exchange.request;
    let connection = &exchange.connection;
    let text = |value: String| Some(value.into_bytes());
    match name {
        "uri" | "document_uri" => text(request.uri.clone()),
        "request_uri" => text(request.request_uri.clone()),
        "args" | "query_string" => request.args.clone().map(String::into_bytes),
        "is_args" => text(
            if request.args.as_deref().is_some_and(|args| !args.is_empty()) {
                "?"
            } else {
                ""
            }
            .into(),
        ),
        "host" => {
            let host = request
                .headers
                .get(header::HOST)
                .and_then(|host| host.to_str().ok())
                .map(|host| {
                    let host = host.trim();
                    let without_port = if host.starts_with('[') {
                        host.split_once(']')
                            .map_or(host, |(address, _)| address)
                            .trim_start_matches('[')
                    } else {
                        host.rsplit_once(':').map_or(host, |(name, _)| name)
                    };
                    without_port.to_ascii_lowercase()
                })
                .filter(|host| !host.is_empty());
            text(host.unwrap_or_else(|| connection.server_name.clone()))
        }
        "remote_addr" => connection
            .client
            .map(|client| client.ip().to_string().into_bytes()),
        "remote_port" => connection
            .client
            .map(|client| client.port().to_string().into_bytes()),
        "binary_remote_addr" => connection.client.map(|client| match client.ip() {
            IpAddr::V4(address) => address.octets().to_vec(),
            IpAddr::V6(address) => address.octets().to_vec(),
        }),
        "server_addr" => connection
            .server
            .map(|server| server.ip().to_string().into_bytes()),
        "server_port" => connection
            .server
            .map(|server| server.port().to_string().into_bytes()),
        "server_name" => text(connection.server_name.clone()),
        "scheme" => text(if connection.tls { "https" } else { "http" }.into()),
        "https" => text(if connection.tls { "on" } else { "" }.into()),
        "request_method" => text(request.method.clone()),
        "request_id" => text(connection.request_id.clone()),
        "request" => text(format!(
            "{} {} {}",
            request.method,
            request.request_uri,
            protocol(request.version)
        )),
        "server_protocol" => text(protocol(request.version).into()),
        "content_type" => field(exchange, "content-type"),
        "content_length" => field(exchange, "content-length"),
        "cookie" => field(exchange, "cookie"),
        "msec" => text(format!("{:.3}", seconds(SystemTime::now()))),
        "time_iso8601" => text(Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()),
        "time_local" => text(Local::now().format("%d/%b/%Y:%H:%M:%S %z").to_string()),
        "request_time" => {
            let started: DateTime<Local> = connection.started.into();
            let elapsed = Local::now().signed_duration_since(started);
            text(format!(
                "{:.3}",
                elapsed.num_milliseconds().max(0) as f64 / 1000.0
            ))
        }
        other => {
            if let Some(name) = other.strip_prefix("http_") {
                field(exchange, name)
            } else if let Some(name) = other.strip_prefix("cookie_") {
                cookie(exchange, name)
            } else if let Some(name) = other.strip_prefix("arg_") {
                argument(exchange, name)
            } else {
                exchange
                    .variables
                    .get(other)
                    .map(|value| value.clone().into_bytes())
            }
        }
    }
}

pub(super) fn proxy(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let proxy = lua.create_table()?;
    let meta = lua.create_table()?;
    let reader = Arc::clone(slot);
    meta.raw_set(
        "__index",
        lua.create_function(move |lua, (_, name): (Table, Value)| {
            let Value::String(name) = name else {
                return Ok(Value::Nil);
            };
            let name = name.to_str()?.to_ascii_lowercase();
            exchange(&reader, Api::Var, |exchange| {
                variable(exchange, &name)
                    .map(|value| lua.create_string(value).map(Value::String))
                    .transpose()
                    .map(Option::unwrap_or_default)
            })
        })?,
    )?;
    let writer = Arc::clone(slot);
    meta.raw_set(
        "__newindex",
        lua.create_function(move |_, (_, name, value): (Table, LuaString, Value)| {
            let name = name.to_str()?.to_ascii_lowercase();
            let value = match &value {
                Value::Nil => None,
                other => Some(
                    String::from_utf8(bytes(other).ok_or_else(|| {
                        mlua::Error::runtime(format!(
                            "bad variable value of type {}",
                            other.type_name()
                        ))
                    })?)
                    .map_err(|_| mlua::Error::runtime("variable values must be UTF-8"))?,
                ),
            };
            exchange(&writer, Api::Var, |exchange| {
                if name == "args" || name == "query_string" {
                    exchange.request.args = value;
                    exchange.changes.args = true;
                    return Ok(());
                }
                if BUILT_IN.contains(&name.as_str())
                    || ["http_", "cookie_", "arg_"]
                        .iter()
                        .any(|prefix| name.starts_with(prefix))
                {
                    return Err(mlua::Error::runtime(format!(
                        "variable \"{name}\" not changeable"
                    )));
                }
                match value {
                    Some(value) => exchange.variables.insert(name, value),
                    None => exchange.variables.remove(&name),
                };
                Ok(())
            })
        })?,
    )?;
    proxy.set_metatable(Some(meta))?;
    Ok(proxy)
}
