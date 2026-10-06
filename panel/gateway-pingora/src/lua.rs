//! Lua handlers in the data plane (ADR 0039): the runtime of a snapshot,
//! the hooks its sites, routes and upstreams run, and how a run's exchange
//! is taken from a request and given back to it.

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use http::{header, HeaderMap, HeaderValue, Method};
use panel_errors::{PanelError, Result};
use panel_ir::RuntimeSnapshot;
use panel_lua::{Connection, Exchange, Host, Request, Runtime, SharedStore};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Instant, SystemTime},
};

/// Bytes of a request body the gateway holds for a request it proxies after
/// a script read it: what Pingora keeps to send the body again.
pub(crate) const PROXIED_BODY_LIMIT: usize = 64 << 10;

/// Bytes a decoded path is encoded back with when a script sets it: those
/// RFC 3986 §3.3 does not allow in a path segment.
const PATH: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

pub(crate) use panel_lua_ir::{Hook, HookIndex, Hooks, Variable};

/// A snapshot's VMs.
pub(crate) struct LuaPlan {
    pub runtime: Runtime,
    /// `lua off`: scripts stay but none runs.
    pub disabled: bool,
    /// `access_by_lua_no_postpone on`: access handlers run before the
    /// security policies.
    pub access_first: bool,
}

impl std::fmt::Debug for LuaPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LuaPlan")
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

/// Compiles `snapshot`'s scripts and starts `vms` VMs for them, with the
/// secrets cosockets' TLS terms name read from `secrets`. Syntax errors,
/// unreadable TLS terms and failing `init_by_lua` refuse the snapshot.
pub(crate) fn compile(
    snapshot: &RuntimeSnapshot,
    store: &SharedStore,
    vms: usize,
    secrets: &dyn crate::secrets::SecretSource,
) -> Result<(Option<Arc<LuaPlan>>, HookIndex)> {
    let read = |id: &str| secrets.read(id);
    let Some(compiled) = panel_lua_ir::compile_with_secrets(snapshot, vms, Some(&read))? else {
        return Ok((None, HookIndex::default()));
    };
    let (runtime, logs) =
        Runtime::start(&compiled.program, &compiled.settings, store).map_err(|failure| {
            PanelError::validation_failed(format!(
                "init_by_lua failed ({}): {}",
                failure.kind.name(),
                failure.message
            ))
        })?;
    for entry in logs {
        tracing::info!(event = "lua_log", phase = "init", level = entry.level.name(), message = %entry.message);
    }
    runtime.on_timer(|run| {
        for entry in &run.logs {
            tracing::info!(event = "lua_log", phase = run.phase.name(), vm = run.vm, level = entry.level.name(), message = %entry.message);
        }
        if let Some(failure) = &run.failure {
            tracing::warn!(event = "lua_failed", phase = run.phase.name(), vm = run.vm, premature = run.premature, kind = failure.kind.name(), message = %failure.message);
        }
    });
    Ok((
        Some(Arc::new(LuaPlan {
            runtime,
            disabled: compiled.disabled,
            access_first: snapshot.lua.access_first,
        })),
        compiled.index,
    ))
}

/// `$uri`: the normalized path with every percent-encoding decoded.
pub(crate) fn decoded_path(path: &str) -> String {
    percent_decode_str(path).decode_utf8_lossy().into_owned()
}

/// What the request looks like now, for the next handler to see.
pub(crate) fn sync_request(exchange: &mut Exchange, header: &RequestHeader, path: &str) {
    let request = &mut exchange.request;
    request.method = header.method.as_str().to_owned();
    request.uri = decoded_path(path);
    request.args = header.uri.query().map(str::to_owned);
    request.version = header.version;
    request.headers.clone_from(&header.headers);
}

/// The exchange a request's first handler starts from.
pub(crate) fn exchange(
    header: &RequestHeader,
    path: &str,
    client: Option<SocketAddr>,
    server: Option<SocketAddr>,
    tls: bool,
    host: &str,
    started: Instant,
) -> Exchange {
    let mut exchange = Exchange::new(
        Request::default(),
        Connection {
            client,
            server,
            tls,
            server_name: host.to_owned(),
            request_id: crate::request_identity::request_id(&header.headers)
                .unwrap_or_default()
                .to_owned(),
            started: SystemTime::now()
                .checked_sub(started.elapsed())
                .unwrap_or_else(SystemTime::now),
        },
    );
    // `$request_uri` stays the client's through rewrites and redirects.
    exchange.request.request_uri = header
        .uri
        .path_and_query()
        .map_or("/", |value| value.as_str())
        .to_owned();
    sync_request(&mut exchange, header, path);
    exchange
}

/// The fields to remove and the lines to append that turn `old` into `new`.
fn header_plan(
    old: &HeaderMap,
    new: &HeaderMap,
) -> (
    Vec<header::HeaderName>,
    Vec<(header::HeaderName, HeaderValue)>,
) {
    let changed = |name: &header::HeaderName| old.get_all(name).iter().ne(new.get_all(name).iter());
    let removed = old.keys().filter(|name| changed(name)).cloned().collect();
    let appended = new
        .keys()
        .filter(|name| changed(name))
        .flat_map(|name| {
            new.get_all(name)
                .iter()
                .map(move |value| (name.clone(), value.clone()))
        })
        .collect();
    (removed, appended)
}

fn header_error(error: impl std::fmt::Display) -> PanelError {
    PanelError::validation_failed(format!("a script set an invalid request: {error}"))
}

/// Gives the request line and header a handler changed back to the request.
/// Returns whether the path changed, so the route may be chosen again.
pub(crate) fn apply_request(header: &mut RequestHeader, exchange: &mut Exchange) -> Result<bool> {
    let changes = exchange.changes();
    let request = &exchange.request;
    if changes.method {
        let method = Method::from_bytes(request.method.as_bytes()).map_err(header_error)?;
        header.set_method(method);
    }
    if changes.uri || changes.args {
        let path = if changes.uri {
            utf8_percent_encode(&request.uri, PATH).to_string()
        } else {
            header.uri.path().to_owned()
        };
        let target = match &request.args {
            Some(args) if !args.is_empty() => format!("{path}?{args}"),
            _ => path,
        };
        header.set_uri(target.parse().map_err(header_error)?);
    }
    if changes.headers {
        let (removed, appended) = header_plan(&header.headers, &request.headers);
        for name in &removed {
            header.remove_header(name);
        }
        for (name, value) in appended {
            header.append_header(name, value).map_err(header_error)?;
        }
    }
    exchange.clear_changes();
    Ok(changes.uri)
}

/// Gives the status and header a header filter changed back to the
/// response.
pub(crate) fn apply_response(header: &mut ResponseHeader, exchange: &mut Exchange) -> Result<()> {
    let changes = exchange.changes();
    if changes.status && exchange.response.status != 0 {
        header
            .set_status(exchange.response.status)
            .map_err(header_error)?;
    }
    if changes.response_headers {
        let (removed, appended) = header_plan(&header.headers, &exchange.response.headers);
        for name in &removed {
            header.remove_header(name);
        }
        for (name, value) in appended {
            header.append_header(name, value).map_err(header_error)?;
        }
    }
    exchange.clear_changes();
    Ok(())
}

/// The response a handler answered with, as Pingora sends it.
pub(crate) fn response_header(exchange: &Exchange, body: usize) -> Result<ResponseHeader> {
    let status = match exchange.response.status {
        0 => 200,
        status => status,
    };
    let mut response = ResponseHeader::build(status, Some(exchange.response.headers.len() + 2))
        .map_err(header_error)?;
    for (name, value) in &exchange.response.headers {
        response
            .append_header(name.clone(), value.clone())
            .map_err(header_error)?;
    }
    if !matches!(status, 100..=199 | 204 | 304) {
        if exchange.default_type() && !response.headers.contains_key(header::CONTENT_TYPE) {
            response
                .insert_header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .map_err(header_error)?;
        }
        response
            .insert_header(header::CONTENT_LENGTH, body.to_string())
            .map_err(header_error)?;
    }
    Ok(response)
}

/// What a request a handler answered without output gets: nginx's default
/// page for the status, as plain text.
pub(crate) fn default_body(status: u16) -> Bytes {
    let reason = http::StatusCode::from_u16(status)
        .ok()
        .and_then(|status| status.canonical_reason())
        .unwrap_or("");
    Bytes::from(format!("{status} {reason}\n"))
}

/// Reads the request body for `ngx.req.read_body`.
pub(crate) struct SessionHost<'a> {
    pub session: &'a mut Session,
    /// The most the gateway holds: less for requests it proxies afterwards.
    pub limit: usize,
    /// What `ngx.req.socket` has read so far.
    pub streamed: usize,
}

#[async_trait]
impl Host for SessionHost<'_> {
    async fn read_body(&mut self, limit: usize) -> std::result::Result<Bytes, String> {
        let limit = limit.min(self.limit);
        let declared = self
            .session
            .req_header()
            .headers
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<usize>().ok());
        if declared.is_some_and(|declared| declared > limit) {
            return Err(format!(
                "the request body is larger than the {limit} bytes a script may read here"
            ));
        }
        self.session.as_mut().enable_retry_buffering();
        let mut body = BytesMut::new();
        loop {
            let chunk = self
                .session
                .read_request_body()
                .await
                .map_err(|error| format!("the request body could not be read: {error}"))?;
            let Some(chunk) = chunk else {
                break;
            };
            if body.len() + chunk.len() > limit {
                return Err(format!(
                    "the request body is larger than the {limit} bytes a script may read here"
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body.freeze())
    }

    async fn read_body_chunk(&mut self) -> std::result::Result<Option<Bytes>, String> {
        let chunk = self
            .session
            .read_request_body()
            .await
            .map_err(|error| format!("the request body could not be read: {error}"))?;
        if let Some(chunk) = &chunk {
            self.streamed += chunk.len();
            if self.streamed > self.limit {
                return Err(format!(
                    "the request body is larger than the {} bytes a script may read here",
                    self.limit
                ));
            }
        }
        Ok(chunk)
    }

    /// Watches the connection once the request body is in: the client
    /// closing it, or resetting its HTTP/2 stream, ends the wait. A body
    /// still arriving is left for the script to read.
    async fn closed(&mut self) {
        let session = self.session.as_mut();
        if !session.is_body_empty() && !session.is_body_done() {
            return std::future::pending().await;
        }
        if let Some(idle) = self.session.as_mut().watch_h2_stream_close() {
            let _ = idle.await;
            return;
        }
        if self.session.read_body_or_idle(true).await.is_ok() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_decode_for_scripts_and_encode_back() {
        assert_eq!(decoded_path("/a%20b/%C3%A9"), "/a b/é");
        assert_eq!(
            utf8_percent_encode("/new path/é?#", PATH).to_string(),
            "/new%20path/%C3%A9%3F%23"
        );
    }

    #[test]
    fn header_plans_touch_only_changed_fields() {
        let mut old = HeaderMap::new();
        old.insert("a", HeaderValue::from_static("1"));
        old.insert("b", HeaderValue::from_static("2"));
        let mut new = old.clone();
        new.remove("a");
        new.append("b", HeaderValue::from_static("3"));
        new.insert("c", HeaderValue::from_static("4"));
        let (removed, appended) = header_plan(&old, &new);
        assert_eq!(removed, ["a", "b"]);
        let appended: Vec<_> = appended
            .iter()
            .map(|(name, value)| (name.as_str(), value.to_str().unwrap()))
            .collect();
        assert_eq!(appended, [("b", "2"), ("b", "3"), ("c", "4")]);
    }
}
