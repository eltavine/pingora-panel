//! What a request leaves in the logs (ADR 0025): access records as JSON
//! lines keyed by OpenTelemetry attribute names or in the Combined Log
//! Format, and error records as JSON lines.

use crate::{
    request_identity,
    template::{Facts, Template},
};
use chrono::{DateTime, SecondsFormat, Utc};
use http::{header, HeaderMap, Version};
use panel_errors::{PanelError, Result};
use panel_ir::{logging::REDACTED, AccessLog, AccessLogFormat, LogFiles, LoggingPolicy};
use panel_metrics::ErrorType;
use pingora_core::{Error, ErrorSource};
use pingora_http::RequestHeader;
use serde::Serialize;
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Write as _,
    net::{IpAddr, SocketAddr},
    sync::LazyLock,
    time::Duration,
};

/// The longest error message an error record keeps.
const MAX_MESSAGE: usize = 4096;

/// How the requests of a site or route are logged, resolved from every
/// scope.
#[derive(Clone, Debug)]
pub(crate) struct AccessPlan {
    pub enabled: bool,
    pub format: AccessLogFormat,
    /// Extra fields of JSON records.
    fields: Vec<(String, Template)>,
}

impl Default for AccessPlan {
    fn default() -> Self {
        Self {
            enabled: true,
            format: AccessLogFormat::Json,
            fields: Vec::new(),
        }
    }
}

impl AccessPlan {
    /// The plan without a snapshot: the defaults.
    pub(crate) fn fallback() -> &'static Self {
        static FALLBACK: LazyLock<AccessPlan> = LazyLock::new(AccessPlan::default);
        &FALLBACK
    }

    /// The settings of `scopes`, outermost first: the innermost that sets
    /// a value wins, and fields add up.
    pub(crate) fn resolve(scopes: &[&AccessLog]) -> Result<Self> {
        let mut plan = Self::default();
        let mut fields = BTreeMap::new();
        for scope in scopes {
            if let Some(enabled) = scope.enabled {
                plan.enabled = enabled;
            }
            if let Some(format) = scope.format {
                plan.format = format;
            }
            fields.extend(
                scope
                    .fields
                    .iter()
                    .map(|(name, template)| (name.as_str(), template.as_str())),
            );
        }
        plan.fields = fields
            .into_iter()
            .map(|(name, template)| {
                Template::parse(template)
                    .map(|template| (name.to_owned(), template))
                    .map_err(|error| {
                        PanelError::validation_failed(format!(
                            "log field {name} has an invalid template: {error}"
                        ))
                    })
            })
            .collect::<Result<_>>()?;
        Ok(plan)
    }
}

/// What the active snapshot keeps out of every record, and how its files
/// rotate.
#[derive(Clone, Debug)]
pub(crate) struct LoggingPlan {
    pub files: LogFiles,
    pub revision: Option<u64>,
    redacted_query: HashSet<String>,
    redacted_headers: HashSet<String>,
}

impl LoggingPlan {
    pub(crate) fn new(policy: &LoggingPolicy, revision: Option<u64>) -> Self {
        Self {
            files: policy.files,
            revision,
            redacted_query: policy.redacted_query().into_iter().collect(),
            redacted_headers: policy.redacted_headers().into_iter().collect(),
        }
    }

    /// The plan without a snapshot: the defaults.
    pub(crate) fn fallback() -> &'static Self {
        static FALLBACK: LazyLock<LoggingPlan> =
            LazyLock::new(|| LoggingPlan::new(&LoggingPolicy::default(), None));
        &FALLBACK
    }

    /// `query` with the values of sensitive parameters replaced, keys kept.
    pub(crate) fn query<'a>(&self, query: &'a str) -> Cow<'a, str> {
        let sensitive = |pair: &str| {
            pair.split_once('=')
                .is_some_and(|(key, _)| self.redacted_query.contains(key))
        };
        if !query.split('&').any(sensitive) {
            return Cow::Borrowed(query);
        }
        let pairs: Vec<Cow<'_, str>> = query
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((key, _)) if self.redacted_query.contains(key) => {
                    Cow::Owned(format!("{key}={REDACTED}"))
                }
                _ => Cow::Borrowed(pair),
            })
            .collect();
        Cow::Owned(pairs.join("&"))
    }
}

/// A request that is done, as the logs see it.
pub(crate) struct Served<'a> {
    /// The variables `set` and scripts gave the request.
    pub variables: &'a HashMap<String, String>,
    pub request: &'a RequestHeader,
    pub scheme: &'static str,
    pub host: Option<&'a str>,
    /// The client, after trusted proxies.
    pub client: Option<IpAddr>,
    /// The peer the connection came from.
    pub peer: Option<SocketAddr>,
    pub status: Option<u16>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub duration: Duration,
    pub error_type: Option<ErrorType>,
    pub listener: &'a str,
    pub site: Option<&'a str>,
    pub route: Option<&'a str>,
    pub upstream: Option<&'a str>,
    /// The upstream node, as `address:port`.
    pub node: Option<&'a str>,
}

/// A JSON object written one field at a time.
struct Record(Vec<u8>);

impl Record {
    fn new(timestamp: DateTime<Utc>, event: &str) -> Self {
        let mut record = Self(Vec::with_capacity(512));
        record.0.push(b'{');
        record.field(
            "timestamp",
            &timestamp.to_rfc3339_opts(SecondsFormat::Micros, true),
        );
        record.field("event.name", event);
        record
    }

    fn field<T: Serialize + ?Sized>(&mut self, key: &str, value: &T) {
        if self.0.len() > 1 {
            self.0.push(b',');
        }
        serde_json::to_writer(&mut self.0, key).expect("a string serializes");
        self.0.push(b':');
        serde_json::to_writer(&mut self.0, value).expect("record values serialize");
    }

    fn some<T: Serialize + ?Sized>(&mut self, key: &str, value: Option<&T>) {
        if let Some(value) = value {
            self.field(key, value);
        }
    }

    /// The fields every record has about where the request went.
    fn placement(&mut self, served: &Served<'_>, logging: &LoggingPlan) {
        self.some(
            "pingora_panel.request.id",
            request_identity::request_id(&served.request.headers),
        );
        self.field("pingora_panel.listener.id", served.listener);
        self.some("pingora_panel.site.id", served.site);
        self.some("pingora_panel.route.id", served.route);
        self.some("pingora_panel.revision.id", logging.revision.as_ref());
        self.some("pingora_panel.upstream.id", served.upstream);
        self.some("pingora_panel.upstream.node", served.node);
    }

    fn finish(mut self) -> Vec<u8> {
        self.0.extend_from_slice(b"}\n");
        self.0
    }
}

fn header(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// Where a Lua message comes from.
pub(crate) struct LuaPlace<'a> {
    pub request_id: Option<&'a str>,
    pub listener: &'a str,
    pub site: Option<&'a str>,
    pub route: Option<&'a str>,
    pub phase: &'static str,
    pub script: &'a str,
}

/// OpenTelemetry's severity for an `ngx.log` level.
fn lua_severity(level: &str) -> &'static str {
    match level {
        "debug" => "DEBUG",
        "info" | "notice" => "INFO",
        "warn" => "WARN",
        "error" => "ERROR",
        _ => "FATAL",
    }
}

/// A message a script logged, or the gateway logged about a script's run,
/// as a JSON line of the error log.
pub(crate) fn lua(
    place: &LuaPlace<'_>,
    level: &str,
    message: &str,
    logging: &LoggingPlan,
    now: DateTime<Utc>,
) -> Vec<u8> {
    let mut record = Record::new(now, "pingora_panel.lua");
    record.field("severity_text", lua_severity(level));
    record.field("pingora_panel.lua.level", level);
    record.field("message", message);
    record.field("pingora_panel.lua.phase", place.phase);
    record.field("pingora_panel.lua.script", place.script);
    record.some("pingora_panel.request.id", place.request_id);
    record.field("pingora_panel.listener.id", place.listener);
    record.some("pingora_panel.site.id", place.site);
    record.some("pingora_panel.route.id", place.route);
    record.some("pingora_panel.revision.id", logging.revision.as_ref());
    record.finish()
}

/// An access record as a JSON line.
pub(crate) fn json(
    served: &Served<'_>,
    plan: &AccessPlan,
    logging: &LoggingPlan,
    now: DateTime<Utc>,
) -> Vec<u8> {
    let request = served.request;
    let headers = &request.headers;
    let mut record = Record::new(now, "pingora_panel.access");
    record.some("trace_id", request_identity::trace_id(headers));
    let method = panel_metrics::method(&request.method);
    record.field("http.request.method", method);
    if method == "_OTHER" {
        record.field("http.request.method_original", request.method.as_str());
    }
    record.field("url.scheme", served.scheme);
    record.field("url.path", request.uri.path());
    if let Some(query) = request.uri.query() {
        record.field("url.query", &*logging.query(query));
    }
    record.some("server.address", served.host);
    record.some(
        "client.address",
        served.client.map(|ip| ip.to_string()).as_deref(),
    );
    if let Some(peer) = served.peer {
        record.field("network.peer.address", &peer.ip().to_string());
        record.field("network.peer.port", &peer.port());
    }
    record.some(
        "network.protocol.version",
        panel_metrics::protocol_version(request.version),
    );
    record.some("http.response.status_code", served.status.as_ref());
    record.field("http.request.body.size", &served.request_bytes);
    record.field("http.response.body.size", &served.response_bytes);
    record.some("user_agent.original", header(headers, header::USER_AGENT));
    if let Some(referer) = header(headers, header::REFERER) {
        record.field("http.request.header.referer", &[referer]);
    }
    record.some(
        "error.type",
        served.error_type.map(|error| error.to_string()).as_deref(),
    );
    record.field(
        "http.server.request.duration",
        &served.duration.as_secs_f64(),
    );
    record.placement(served, logging);
    if !plan.fields.is_empty() {
        let facts = Facts {
            host: served.host.unwrap_or_default(),
            uri: request.uri.path(),
            method: request.method.as_str(),
            scheme: served.scheme,
            client_ip: served.client,
            headers,
            upstream: served.node,
            variables: served.variables,
        };
        for (name, template) in &plan.fields {
            record.field(
                name,
                &template.render_redacted(&facts, &logging.redacted_headers),
            );
        }
    }
    record.finish()
}

/// Appends `text` as NGINX does: quotes, backslashes, control characters
/// and bytes outside ASCII as `\xHH`, so no request can forge a line.
fn escaped(line: &mut String, text: &str) {
    for byte in text.bytes() {
        if byte == b'"' || byte == b'\\' || !(0x20..0x7f).contains(&byte) {
            let _ = write!(line, "\\x{byte:02X}");
        } else {
            line.push(char::from(byte));
        }
    }
}

fn protocol(version: Version) -> &'static str {
    match version {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_2 => "HTTP/2.0",
        Version::HTTP_3 => "HTTP/3.0",
        _ => "HTTP/1.1",
    }
}

/// An access record in the Combined Log Format.
pub(crate) fn combined(served: &Served<'_>, logging: &LoggingPlan, now: DateTime<Utc>) -> Vec<u8> {
    let request = served.request;
    let mut line = String::with_capacity(256);
    match served.client {
        Some(client) => line.push_str(&client.to_string()),
        None => line.push('-'),
    }
    line.push_str(" - - [");
    line.push_str(&now.format("%d/%b/%Y:%H:%M:%S +0000").to_string());
    line.push_str("] \"");
    escaped(&mut line, request.method.as_str());
    line.push(' ');
    escaped(&mut line, request.uri.path());
    if let Some(query) = request.uri.query() {
        line.push('?');
        escaped(&mut line, &logging.query(query));
    }
    line.push(' ');
    line.push_str(protocol(request.version));
    line.push_str("\" ");
    line.push_str(&served.status.unwrap_or(0).to_string());
    line.push(' ');
    line.push_str(&served.response_bytes.to_string());
    for name in [header::REFERER, header::USER_AGENT] {
        line.push_str(" \"");
        match header(&request.headers, name) {
            Some(value) => escaped(&mut line, value),
            None => line.push('-'),
        }
        line.push('"');
    }
    line.push('\n');
    line.into_bytes()
}

/// An error record as a JSON line: a warning when the client caused it.
pub(crate) fn error(
    served: &Served<'_>,
    error: &Error,
    logging: &LoggingPlan,
    now: DateTime<Utc>,
) -> Vec<u8> {
    let request = served.request;
    let mut record = Record::new(now, "pingora_panel.error");
    let severity = if error.esource() == &ErrorSource::Downstream {
        "WARN"
    } else {
        "ERROR"
    };
    record.field("severity_text", severity);
    let mut message = error.to_string();
    if message.len() > MAX_MESSAGE {
        let mut end = MAX_MESSAGE;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    record.field("message", &message);
    record.some("trace_id", request_identity::trace_id(&request.headers));
    record.some(
        "error.type",
        served.error_type.map(|error| error.to_string()).as_deref(),
    );
    record.field(
        "http.request.method",
        panel_metrics::method(&request.method),
    );
    record.field("url.path", request.uri.path());
    record.some("server.address", served.host);
    record.some(
        "client.address",
        served.client.map(|ip| ip.to_string()).as_deref(),
    );
    record.some("http.response.status_code", served.status.as_ref());
    record.placement(served, logging);
    record.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_ir::logging::DEFAULT_REDACTED_QUERY;
    use pingora_core::ErrorType as Kind;
    use serde_json::Value;

    fn request(uri: &str) -> RequestHeader {
        let mut request = RequestHeader::build("GET", uri.as_bytes(), None).unwrap();
        request.insert_header("x-request-id", "req-7").unwrap();
        request.insert_header("user-agent", "curl/8").unwrap();
        request
            .insert_header(
                "traceparent",
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            )
            .unwrap();
        request
    }

    static NONE: LazyLock<HashMap<String, String>> = LazyLock::new(HashMap::new);

    fn served(request: &RequestHeader) -> Served<'_> {
        Served {
            variables: &NONE,
            request,
            scheme: "https",
            host: Some("shop.example"),
            client: Some("203.0.113.9".parse().unwrap()),
            peer: Some("10.0.0.1:52100".parse().unwrap()),
            status: Some(200),
            request_bytes: 0,
            response_bytes: 1234,
            duration: Duration::from_millis(12),
            error_type: None,
            listener: "https",
            site: Some("shop"),
            route: Some("checkout"),
            upstream: Some("shop-app"),
            node: Some("10.0.0.7:8080"),
        }
    }

    fn now() -> DateTime<Utc> {
        "2026-10-04T06:36:11.733123Z".parse().unwrap()
    }

    fn logging() -> LoggingPlan {
        LoggingPlan::new(&LoggingPolicy::default(), Some(42))
    }

    #[test]
    fn json_records_use_opentelemetry_names() {
        let request = request("/pay?order=7&sig=secret");
        let line = json(&served(&request), &AccessPlan::default(), &logging(), now());
        assert_eq!(line.last(), Some(&b'\n'));
        let record: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(record["timestamp"], "2026-10-04T06:36:11.733123Z");
        assert_eq!(record["event.name"], "pingora_panel.access");
        assert_eq!(record["trace_id"], "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(record["http.request.method"], "GET");
        assert_eq!(record["url.scheme"], "https");
        assert_eq!(record["url.path"], "/pay");
        assert_eq!(record["url.query"], "order=7&sig=REDACTED");
        assert_eq!(record["server.address"], "shop.example");
        assert_eq!(record["client.address"], "203.0.113.9");
        assert_eq!(record["network.peer.address"], "10.0.0.1");
        assert_eq!(record["network.peer.port"], 52100);
        assert_eq!(record["network.protocol.version"], "1.1");
        assert_eq!(record["http.response.status_code"], 200);
        assert_eq!(record["http.response.body.size"], 1234);
        assert_eq!(record["user_agent.original"], "curl/8");
        assert_eq!(record["http.server.request.duration"], 0.012);
        assert_eq!(record["pingora_panel.request.id"], "req-7");
        assert_eq!(record["pingora_panel.site.id"], "shop");
        assert_eq!(record["pingora_panel.route.id"], "checkout");
        assert_eq!(record["pingora_panel.revision.id"], 42);
        assert_eq!(record["pingora_panel.upstream.node"], "10.0.0.7:8080");
        assert!(record.get("error.type").is_none());
    }

    #[test]
    fn extra_fields_are_added_and_redacted() {
        let mut request = request("/");
        request.insert_header("x-tenant", "acme").unwrap();
        request
            .insert_header("authorization", "Bearer secret")
            .unwrap();
        let plan = AccessPlan::resolve(&[
            &AccessLog {
                fields: [("tenant".to_owned(), "$http_x_tenant".to_owned())].into(),
                ..AccessLog::default()
            },
            &AccessLog {
                fields: [("auth".to_owned(), "$http_authorization".to_owned())].into(),
                ..AccessLog::default()
            },
        ])
        .unwrap();
        let record: Value =
            serde_json::from_slice(&json(&served(&request), &plan, &logging(), now())).unwrap();
        assert_eq!(record["tenant"], "acme");
        assert_eq!(record["auth"], "REDACTED");
    }

    #[test]
    fn inner_scopes_win_and_fields_add_up() {
        let outer = AccessLog {
            format: Some(AccessLogFormat::Combined),
            fields: [("a".to_owned(), "1".to_owned())].into(),
            ..AccessLog::default()
        };
        let inner = AccessLog {
            enabled: Some(false),
            fields: [("b".to_owned(), "2".to_owned())].into(),
            ..AccessLog::default()
        };
        let plan = AccessPlan::resolve(&[&outer, &inner]).unwrap();
        assert!(!plan.enabled);
        assert_eq!(plan.format, AccessLogFormat::Combined);
        assert_eq!(plan.fields.len(), 2);
        assert!(AccessPlan::resolve(&[]).unwrap().enabled);
    }

    #[test]
    fn combined_lines_follow_the_format_and_escape_what_could_forge_a_line() {
        let mut request = request("/search?q=a%22b&X-Amz-Signature=abc");
        request
            .insert_header("referer", "https://example.com/\"x")
            .unwrap();
        let line = String::from_utf8(combined(&served(&request), &logging(), now())).unwrap();
        assert_eq!(
            line,
            "203.0.113.9 - - [04/Oct/2026:06:36:11 +0000] \
             \"GET /search?q=a%22b&X-Amz-Signature=REDACTED HTTP/1.1\" 200 1234 \
             \"https://example.com/\\x22x\" \"curl/8\"\n"
        );
        let mut escaped_line = String::new();
        escaped(&mut escaped_line, "a\nb\\c\"d\u{e9}");
        assert_eq!(escaped_line, "a\\x0Ab\\x5Cc\\x22d\\xC3\\xA9");
    }

    #[test]
    fn query_redaction_keeps_keys_and_matches_by_case() {
        let plan = logging();
        assert!(matches!(plan.query("a=1&b=2"), Cow::Borrowed("a=1&b=2")));
        assert_eq!(plan.query("SIG=1&sig=2&sig"), "SIG=1&sig=REDACTED&sig");
        for key in DEFAULT_REDACTED_QUERY {
            assert_eq!(plan.query(&format!("{key}=v")), format!("{key}=REDACTED"));
        }
    }

    #[test]
    fn errors_are_recorded_with_their_type_and_severity() {
        let request = request("/pay?sig=secret");
        let mut failed = served(&request);
        failed.status = Some(502);
        failed.error_type = Some(ErrorType::Named("connect_refused"));
        let upstream = Error::explain(Kind::ConnectRefused, "connect to 10.0.0.7:8080");
        let record: Value =
            serde_json::from_slice(&error(&failed, &upstream, &logging(), now())).unwrap();
        assert_eq!(record["event.name"], "pingora_panel.error");
        assert_eq!(record["severity_text"], "ERROR");
        assert_eq!(record["error.type"], "connect_refused");
        assert_eq!(record["url.path"], "/pay");
        assert!(record.get("url.query").is_none());
        assert!(record["message"]
            .as_str()
            .unwrap()
            .contains("10.0.0.7:8080"));

        let mut downstream = Error::new(Kind::ReadError);
        downstream.esource = ErrorSource::Downstream;
        let record: Value =
            serde_json::from_slice(&error(&failed, &downstream, &logging(), now())).unwrap();
        assert_eq!(record["severity_text"], "WARN");
    }
}
