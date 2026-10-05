//! HTTP policies (ADR 0037), compiled once per snapshot: field changes of
//! requests and responses, the `Server` field, the CORS protocol of the
//! Fetch Standard and compression.

use crate::template::{Facts, Template};
use async_trait::async_trait;
use http::{header, HeaderName, HeaderValue, Method, StatusCode};
use panel_errors::{PanelError, Result};
use panel_ir::{CompressionAlgorithm, HeaderPolicy, ServerHeader};
use pingora_core::{
    modules::http::{compression::ResponseCompression, HttpModule, HttpModuleBuilder, Module},
    protocols::http::compression::Algorithm,
};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::any::Any;

/// Request fields browsers send without a preflight asking for them.
const SAFELISTED_REQUEST_FIELDS: &[&str] = &[
    "accept",
    "accept-language",
    "content-language",
    "content-type",
    "range",
];
/// Methods a preflight never needs to allow.
const SAFELISTED_METHODS: &[&str] = &["GET", "HEAD", "POST"];

/// The levels codings compress at: fast enough for responses on the fly.
fn level(algorithm: CompressionAlgorithm) -> (Algorithm, u32) {
    match algorithm {
        CompressionAlgorithm::Gzip => (Algorithm::Gzip, 6),
        CompressionAlgorithm::Brotli => (Algorithm::Brotli, 4),
        CompressionAlgorithm::Zstd => (Algorithm::Zstd, 3),
    }
}

/// A field change with its value rendered for one request.
#[derive(Clone, Debug)]
pub(crate) enum FieldChange {
    Remove(HeaderName),
    Set(HeaderName, HeaderValue),
    Add(HeaderName, HeaderValue),
}

struct FieldChanges {
    remove: Vec<HeaderName>,
    set: Vec<(HeaderName, Template)>,
    add: Vec<(HeaderName, Template)>,
}

enum Server {
    Keep,
    Remove,
    Replace(HeaderValue),
}

pub(crate) struct HttpPolicy {
    request: FieldChanges,
    response: FieldChanges,
    server: Server,
    pub cors: Option<Cors>,
    pub compression: Option<Compression>,
}

pub(crate) struct Cors {
    origins: Vec<Origin>,
    any_origin: bool,
    methods: Vec<String>,
    headers: Vec<String>,
    any_header: bool,
    allow_methods: Option<HeaderValue>,
    expose: Option<HeaderValue>,
    credentials: bool,
    max_age: Option<HeaderValue>,
}

enum Origin {
    Exact(String),
    /// `scheme://*.parent[:port]`, one label below `parent`.
    Wildcard {
        prefix: String,
        suffix: String,
    },
}

pub(crate) struct Compression {
    algorithms: Vec<(Algorithm, u32)>,
    types: Vec<(String, String)>,
    min_bytes: u64,
}

fn invalid(policy: &str, detail: impl std::fmt::Display) -> PanelError {
    PanelError::validation_failed(format!("HTTP policy {policy} is invalid: {detail}"))
}

fn name(policy: &str, value: &str) -> Result<HeaderName> {
    HeaderName::from_bytes(value.as_bytes()).map_err(|_| invalid(policy, format!("{value:?}")))
}

fn joined(values: &[String]) -> Option<HeaderValue> {
    (!values.is_empty())
        .then(|| HeaderValue::from_str(&values.join(", ")).ok())
        .flatten()
}

impl FieldChanges {
    fn compile(
        policy: &str,
        remove: impl IntoIterator<Item = String>,
        set: impl IntoIterator<Item = (String, String)>,
        add: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self> {
        let template = |(field, value): (String, String)| {
            Ok::<_, PanelError>((
                name(policy, &field)?,
                Template::parse(&value).map_err(|error| invalid(policy, error))?,
            ))
        };
        Ok(Self {
            remove: remove
                .into_iter()
                .map(|field| name(policy, &field))
                .collect::<Result<_>>()?,
            set: set.into_iter().map(template).collect::<Result<_>>()?,
            add: add.into_iter().map(template).collect::<Result<_>>()?,
        })
    }

    fn render(&self, facts: &Facts<'_>, out: &mut Vec<FieldChange>) {
        let value =
            |template: &Template| HeaderValue::from_maybe_shared(template.render(facts)).ok();
        out.extend(self.remove.iter().cloned().map(FieldChange::Remove));
        out.extend(
            self.set.iter().filter_map(|(name, template)| {
                Some(FieldChange::Set(name.clone(), value(template)?))
            }),
        );
        out.extend(
            self.add.iter().filter_map(|(name, template)| {
                Some(FieldChange::Add(name.clone(), value(template)?))
            }),
        );
    }
}

impl HttpPolicy {
    pub(crate) fn compile(policy: &HeaderPolicy) -> Result<Self> {
        let id = policy.id.as_str();
        let pairs = |fields: &[panel_ir::HeaderField]| {
            fields
                .iter()
                .map(|field| (field.name.clone(), field.value.clone()))
                .collect::<Vec<_>>()
        };
        Ok(Self {
            request: FieldChanges::compile(
                id,
                policy.request_remove.iter().cloned(),
                policy.request_set.clone(),
                pairs(&policy.request_add),
            )?,
            response: FieldChanges::compile(
                id,
                policy.response_remove.iter().cloned(),
                policy.response_set.clone(),
                pairs(&policy.response_add),
            )?,
            server: match &policy.server {
                ServerHeader::Keep => Server::Keep,
                ServerHeader::Remove => Server::Remove,
                ServerHeader::Replace { value } => Server::Replace(
                    HeaderValue::from_str(value).map_err(|error| invalid(id, error))?,
                ),
            },
            cors: policy.cors.as_ref().map(Cors::compile),
            compression: policy.compression.as_ref().map(|compression| Compression {
                algorithms: compression.algorithms.iter().copied().map(level).collect(),
                types: compression
                    .types
                    .iter()
                    .filter_map(|media| {
                        let (kind, subtype) = media.split_once('/')?;
                        Some((kind.to_ascii_lowercase(), subtype.to_ascii_lowercase()))
                    })
                    .collect(),
                min_bytes: compression.min_bytes,
            }),
        })
    }

    /// Appends the request changes, rendered for this request.
    pub(crate) fn request_changes(&self, facts: &Facts<'_>, out: &mut Vec<FieldChange>) {
        self.request.render(facts, out);
    }

    /// Appends the response changes, rendered for this request.
    pub(crate) fn response_changes(&self, facts: &Facts<'_>, out: &mut Vec<FieldChange>) {
        self.response.render(facts, out);
    }

    fn server(&self) -> Option<&Server> {
        (!matches!(self.server, Server::Keep)).then_some(&self.server)
    }
}

/// Applies changes to request fields in order.
pub(crate) fn apply_to_request(
    request: &mut RequestHeader,
    changes: &[FieldChange],
) -> pingora_core::Result<()> {
    for change in changes {
        match change {
            FieldChange::Remove(name) => {
                request.remove_header(name);
            }
            FieldChange::Set(name, value) => request.insert_header(name.clone(), value.clone())?,
            FieldChange::Add(name, value) => {
                request.append_header(name.clone(), value.clone())?;
            }
        }
    }
    Ok(())
}

impl Cors {
    fn compile(cors: &panel_ir::CorsPolicy) -> Self {
        let origins = cors
            .allowed_origins
            .iter()
            .filter(|origin| origin.as_str() != "*")
            .map(|origin| match origin.split_once("://*.") {
                Some((scheme, rest)) => Origin::Wildcard {
                    prefix: format!("{scheme}://"),
                    suffix: format!(".{rest}"),
                },
                None => Origin::Exact(origin.clone()),
            })
            .collect();
        let any_header =
            cors.allowed_headers.iter().any(|name| name == "*") && !cors.allow_credentials;
        Self {
            origins,
            any_origin: cors.allowed_origins.iter().any(|origin| origin == "*"),
            methods: cors
                .allowed_methods
                .iter()
                .filter(|method| method.as_str() != "*")
                .cloned()
                .collect(),
            headers: cors
                .allowed_headers
                .iter()
                .filter(|name| name.as_str() != "*")
                .map(|name| name.to_ascii_lowercase())
                .collect(),
            any_header,
            allow_methods: joined(&cors.allowed_methods),
            expose: joined(&cors.exposed_headers),
            credentials: cors.allow_credentials,
            max_age: cors.max_age_seconds.map(HeaderValue::from),
        }
    }

    fn allows_origin(&self, origin: &str) -> bool {
        self.any_origin
            || self.origins.iter().any(|allowed| match allowed {
                Origin::Exact(exact) => exact == origin,
                Origin::Wildcard { prefix, suffix } => origin
                    .strip_prefix(prefix.as_str())
                    .and_then(|rest| rest.strip_suffix(suffix.as_str()))
                    .is_some_and(|label| !label.is_empty() && !label.contains(['.', ':', '/'])),
            })
    }

    /// The `Access-Control-Allow-Origin` value for an allowed origin.
    fn allow_origin(&self, origin: &str) -> Option<HeaderValue> {
        if self.any_origin && !self.credentials {
            Some(HeaderValue::from_static("*"))
        } else {
            HeaderValue::from_str(origin).ok()
        }
    }

    /// The answer to a preflight (Fetch Standard §3.2.5), or `None` when
    /// the request is not one.
    pub(crate) fn preflight(
        &self,
        request: &RequestHeader,
    ) -> Option<Vec<(HeaderName, HeaderValue)>> {
        if request.method != Method::OPTIONS {
            return None;
        }
        let origin = request.headers.get(header::ORIGIN)?.to_str().ok()?;
        let method = request
            .headers
            .get(header::ACCESS_CONTROL_REQUEST_METHOD)?
            .to_str()
            .ok()?;
        let requested: Vec<String> = request
            .headers
            .get_all(header::ACCESS_CONTROL_REQUEST_HEADERS)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(|name| name.trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect();
        let mut answer = vec![(
            header::VARY,
            HeaderValue::from_static(
                "Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
            ),
        )];
        let method_allowed = SAFELISTED_METHODS.contains(&method)
            || self.methods.iter().any(|allowed| allowed == method);
        let headers_allowed = requested.iter().all(|name| {
            self.any_header
                || SAFELISTED_REQUEST_FIELDS.contains(&name.as_str())
                || self.headers.contains(name)
        });
        let Some(allow_origin) = self
            .allows_origin(origin)
            .then(|| self.allow_origin(origin))
            .flatten()
            .filter(|_| method_allowed && headers_allowed)
        else {
            return Some(answer);
        };
        answer.push((header::ACCESS_CONTROL_ALLOW_ORIGIN, allow_origin));
        if let Some(methods) = &self.allow_methods {
            answer.push((header::ACCESS_CONTROL_ALLOW_METHODS, methods.clone()));
        }
        if !requested.is_empty() {
            if let Ok(names) = HeaderValue::from_str(&requested.join(", ")) {
                answer.push((header::ACCESS_CONTROL_ALLOW_HEADERS, names));
            }
        }
        if self.credentials {
            answer.push((
                header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            ));
        }
        if let Some(max_age) = &self.max_age {
            answer.push((header::ACCESS_CONTROL_MAX_AGE, max_age.clone()));
        }
        Some(answer)
    }

    /// The fields a response to an allowed cross-origin request carries.
    pub(crate) fn response(&self, request: &RequestHeader) -> Option<CorsResponse> {
        let origin = request.headers.get(header::ORIGIN)?.to_str().ok()?;
        let allow_origin = self
            .allows_origin(origin)
            .then(|| self.allow_origin(origin))
            .flatten()?;
        Some(CorsResponse {
            vary: allow_origin != "*",
            allow_origin,
            credentials: self.credentials,
            expose: self.expose.clone(),
        })
    }
}

pub(crate) struct CorsResponse {
    allow_origin: HeaderValue,
    vary: bool,
    credentials: bool,
    expose: Option<HeaderValue>,
}

/// Turns compression off for a request; Pingora's module starts on so that
/// it records the request's `Accept-Encoding`, and policies turn it back on.
pub(crate) fn disable_compression(session: &mut Session) {
    if let Some(compression) = session
        .downstream_modules_ctx
        .get_mut::<ResponseCompression>()
    {
        compression.adjust_level(0);
    }
}

impl Compression {
    /// Compresses the request's responses with the policy's codings only.
    pub(crate) fn prepare(&self, session: &mut Session) {
        if let Some(compression) = session
            .downstream_modules_ctx
            .get_mut::<ResponseCompression>()
        {
            compression.adjust_level(0);
            for (algorithm, level) in &self.algorithms {
                compression.adjust_algorithm_level(*algorithm, *level);
            }
        }
    }

    /// Keeps compression on for `response` when the policy covers it — a
    /// listed media type of at least the minimum size, not already encoded,
    /// not partial and not marked `no-transform` (RFC 9111 §5.2.2.6) — and
    /// turns it off otherwise.
    pub(crate) fn decide(
        &self,
        session: &mut Session,
        response: &mut ResponseHeader,
    ) -> pingora_core::Result<()> {
        if !self.covers(session, response) {
            disable_compression(session);
            return Ok(());
        }
        let varies = response
            .headers
            .get_all(header::VARY)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| {
                value
                    .split(',')
                    .any(|name| name.trim().eq_ignore_ascii_case("accept-encoding"))
            });
        if !varies {
            response.append_header(header::VARY, "Accept-Encoding")?;
        }
        Ok(())
    }

    fn covers(&self, session: &Session, response: &ResponseHeader) -> bool {
        let status = response.status;
        if status.is_informational()
            || matches!(
                status,
                StatusCode::NO_CONTENT | StatusCode::PARTIAL_CONTENT | StatusCode::NOT_MODIFIED
            )
            || session.req_header().method == Method::HEAD
        {
            return false;
        }
        let field = |name: header::HeaderName| {
            response
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_ascii_lowercase)
        };
        if field(header::CONTENT_ENCODING).is_some_and(|coding| coding != "identity")
            || field(header::CACHE_CONTROL).is_some_and(|control| control.contains("no-transform"))
            || field(header::CONTENT_LENGTH)
                .and_then(|length| length.parse::<u64>().ok())
                .is_some_and(|length| length < self.min_bytes)
        {
            return false;
        }
        field(header::CONTENT_TYPE)
            .and_then(|value| {
                let essence = value.split(';').next()?.trim().to_owned();
                let (kind, subtype) = essence.split_once('/')?;
                Some((kind.to_owned(), subtype.to_owned()))
            })
            .is_some_and(|(kind, subtype)| {
                self.types.iter().any(|(allowed_kind, allowed_subtype)| {
                    (allowed_kind == "*" || *allowed_kind == kind)
                        && (allowed_subtype == "*" || *allowed_subtype == subtype)
                })
            })
    }
}

/// Applies a request's response changes, `Server` and CORS fields to every
/// response it gets, whether proxied or generated by the gateway.
#[derive(Default)]
pub(crate) struct HttpPolicyModule {
    changes: Vec<FieldChange>,
    server: Option<ServerChange>,
    cors: Option<CorsResponse>,
}

enum ServerChange {
    Remove,
    Replace(HeaderValue),
}

impl HttpPolicyModule {
    /// Prepares the module for a request passing `policies`, site first.
    pub(crate) fn prepare(
        &mut self,
        policies: &[&HttpPolicy],
        facts: &Facts<'_>,
        request: &RequestHeader,
    ) {
        self.changes.clear();
        for policy in policies {
            policy.response_changes(facts, &mut self.changes);
        }
        self.server = policies
            .iter()
            .rev()
            .find_map(|policy| policy.server())
            .map(|server| match server {
                Server::Replace(value) => ServerChange::Replace(value.clone()),
                _ => ServerChange::Remove,
            });
        self.cors = policies
            .iter()
            .rev()
            .find_map(|policy| policy.cors.as_ref())
            .and_then(|cors| cors.response(request));
    }
}

#[async_trait]
impl HttpModule for HttpPolicyModule {
    async fn response_header_filter(
        &mut self,
        response: &mut ResponseHeader,
        _end_of_stream: bool,
    ) -> pingora_core::Result<()> {
        for change in &self.changes {
            match change {
                FieldChange::Remove(name) => {
                    response.remove_header(name);
                }
                FieldChange::Set(name, value) => {
                    response.insert_header(name.clone(), value.clone())?;
                }
                FieldChange::Add(name, value) => {
                    response.append_header(name.clone(), value.clone())?;
                }
            }
        }
        match &self.server {
            Some(ServerChange::Remove) => {
                response.remove_header(&header::SERVER);
            }
            Some(ServerChange::Replace(value)) => {
                response.insert_header(header::SERVER, value.clone())?;
            }
            None => {}
        }
        if let Some(cors) = &self.cors {
            response.insert_header(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                cors.allow_origin.clone(),
            )?;
            if cors.vary {
                response.append_header(header::VARY, "Origin")?;
            }
            if cors.credentials {
                response.insert_header(header::ACCESS_CONTROL_ALLOW_CREDENTIALS, "true")?;
            }
            if let Some(expose) = &cors.expose {
                response.insert_header(header::ACCESS_CONTROL_EXPOSE_HEADERS, expose.clone())?;
            }
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub(crate) struct HttpPolicyBuilder;

impl HttpModuleBuilder for HttpPolicyBuilder {
    fn init(&self) -> Module {
        Box::new(HttpPolicyModule::default())
    }
}
