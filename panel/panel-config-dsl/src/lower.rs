//! From the syntax trees of a configuration's files to the configuration
//! model, reporting every problem at its source.

use crate::{
    codes,
    schema::{self, Context, DirectiveSpec},
    source::{self, Sources, ENTRY},
    values::{self, Params},
    variables::{self, Piece, ENVIRONMENT_PREFIX},
    LANGUAGE_VERSION,
};
use chrono::{DateTime, Utc};
use panel_config_model::{
    validate, Action, ConfigModel, Domain, Listener, MatchKind, Route, RouteMatch, Site, Upstream,
    UpstreamNode,
};
use panel_domain::NormalizedHost;
use panel_dsl::{Argument, Body, Directive, Document, LineIndex, Span};
use panel_errors::{Diagnostic, DiagnosticSeverity};
use panel_ir::{
    ActiveHealthCheck, HealthCheckProtocol, ListenerProtocols, LoadBalancingPolicy,
    PassiveHealthPolicy, TlsProfile, UpstreamConnectionPolicy, UpstreamTlsPolicy, WwwRedirect,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use uuid::Uuid;

pub struct LowerOptions<'a> {
    /// Values for `${env:NAME}`; only names with the documented prefix are read.
    pub environment: &'a BTreeMap<String, String>,
    /// The model the configuration replaces: timestamps, favourites and
    /// deleted sites carry over from it by identifier.
    pub previous: Option<&'a ConfigModel>,
    pub now: DateTime<Utc>,
}

/// Where a resource is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Origin {
    pub file: String,
    /// The directive's span, from its name through its closing `}`.
    pub span: Span,
    /// The span with the comments leading into it.
    pub outer: Span,
    /// Nesting depth within its file.
    pub depth: usize,
}

/// Text that writes an identifier the service assigned back into a file:
/// an `id` line for a block, an `id=` parameter for a node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Insertion {
    pub file: String,
    pub offset: usize,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Lowered {
    pub model: ConfigModel,
    pub diagnostics: Vec<Diagnostic>,
    /// Where each resource is written, by its resource path such as
    /// `sites/<id>` or `sites/<id>/routes/<id>`.
    pub origins: BTreeMap<String, Origin>,
    /// Identifiers assigned to blocks and nodes written without one.
    pub insertions: Vec<Insertion>,
}

impl Lowered {
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    }

    pub fn is_valid(&self) -> bool {
        self.errors().next().is_none()
    }
}

/// Lowers `sources` into the model.
pub fn lower(sources: &Sources, options: &LowerOptions<'_>) -> Lowered {
    let mut lowerer = Lowerer::new(sources, options);
    lowerer.run();
    lowerer.finish()
}

enum ActionDraft {
    Ready(Action),
    Proxy {
        upstream: String,
        file: String,
        span: Span,
    },
}

struct RouteDraft {
    route: Route,
    priority_set: bool,
    action: Option<ActionDraft>,
    origin: Origin,
}

struct ServerDraft {
    site: Site,
    action: Option<ActionDraft>,
    routes: Vec<RouteDraft>,
    origin: Origin,
}

struct ListenerDraft {
    listener: Listener,
    default_server: Option<(String, String, Span)>,
}

struct Lowerer<'a> {
    sources: &'a Sources,
    options: &'a LowerOptions<'a>,
    documents: BTreeMap<String, Rc<Document>>,
    indexes: BTreeMap<String, LineIndex>,
    diagnostics: Vec<Diagnostic>,
    includes: Vec<String>,
    scopes: Vec<BTreeMap<String, String>>,
    profiles: Vec<(TlsProfile, Origin)>,
    listeners: Vec<ListenerDraft>,
    upstreams: Vec<(Upstream, Origin)>,
    servers: Vec<ServerDraft>,
    origins: BTreeMap<String, Origin>,
    insertions: Vec<Insertion>,
}

type Handler<'h, 'a> =
    &'h mut dyn FnMut(&mut Lowerer<'a>, &str, &Directive, &'static DirectiveSpec, usize);

impl<'a> Lowerer<'a> {
    fn new(sources: &'a Sources, options: &'a LowerOptions<'a>) -> Self {
        let mut lowerer = Self {
            sources,
            options,
            documents: BTreeMap::new(),
            indexes: BTreeMap::new(),
            diagnostics: Vec::new(),
            includes: Vec::new(),
            scopes: vec![BTreeMap::new()],
            profiles: Vec::new(),
            listeners: Vec::new(),
            upstreams: Vec::new(),
            servers: Vec::new(),
            origins: BTreeMap::new(),
            insertions: Vec::new(),
        };
        for (path, text) in sources.files() {
            let parsed = panel_dsl::parse(path, text);
            lowerer.diagnostics.extend(parsed.diagnostics);
            lowerer
                .documents
                .insert(path.to_owned(), Rc::new(parsed.document));
            lowerer
                .indexes
                .insert(path.to_owned(), LineIndex::new(text));
        }
        lowerer
    }

    fn span_text(&self, file: &str, span: Span) -> String {
        let text = self.sources.get(file).unwrap_or_default();
        self.indexes
            .get(file)
            .map_or_else(|| file.to_owned(), |index| index.describe(file, text, span))
    }

    fn report(&mut self, diagnostic: Diagnostic, file: &str, span: Span) {
        let mut diagnostic = diagnostic;
        diagnostic.source_span = Some(self.span_text(file, span));
        self.diagnostics.push(diagnostic);
    }

    fn error(&mut self, file: &str, span: Span, code: &str, message: impl Into<String>) {
        self.report(Diagnostic::error(code, message), file, span);
    }

    fn error_with_help(
        &mut self,
        file: &str,
        span: Span,
        code: &str,
        message: impl Into<String>,
        help: impl Into<String>,
    ) {
        self.report(Diagnostic::error(code, message).with_help(help), file, span);
    }

    fn run(&mut self) {
        let Some(document) = self.documents.get(ENTRY).cloned() else {
            self.diagnostics.push(Diagnostic::error(
                codes::INCLUDE,
                format!("the configuration has no {ENTRY}"),
            ));
            return;
        };
        match document.directives.first() {
            Some(first) if first.name.value == "language_version" => {}
            Some(first) => {
                let span = first.name.span;
                self.error_with_help(
                    ENTRY,
                    span,
                    codes::VERSION,
                    "the configuration must start with its language version",
                    format!("add `language_version {LANGUAGE_VERSION};` as the first line"),
                );
            }
            None if self.sources.files().all(|(_, text)| text.trim().is_empty()) => {}
            None => {}
        }
        self.includes.push(ENTRY.to_owned());
        let mut seen = BTreeSet::new();
        self.each(
            ENTRY,
            &document.directives,
            Context::Main,
            0,
            &mut seen,
            &mut |lowerer, file, directive, spec, depth| match spec.name {
                "language_version" => lowerer.language_version(file, directive),
                "http" => lowerer.http(file, directive, depth),
                _ => unreachable!("the schema allows nothing else at the top level"),
            },
        );
        self.includes.pop();
    }

    /// Checks each directive against the schema in `context`, expands
    /// includes and `set`, and hands the rest to `handle`.
    fn each(
        &mut self,
        file: &str,
        directives: &[Directive],
        context: Context,
        depth: usize,
        seen: &mut BTreeSet<&'static str>,
        handle: Handler<'_, 'a>,
    ) {
        for directive in directives {
            if matches!(directive.body, Body::Missing) {
                continue;
            }
            let name = directive.name.value.as_str();
            let Some(spec) = schema::lookup(name, context) else {
                self.misplaced(file, directive, context);
                continue;
            };
            let count = directive.args.len();
            if count < spec.min_args || spec.max_args.is_some_and(|max| count > max) {
                self.error_with_help(
                    file,
                    directive.span,
                    codes::ARGUMENTS,
                    format!(
                        "'{name}' does not take {count} argument{}",
                        if count == 1 { "" } else { "s" }
                    ),
                    format!("write it as `{}`", spec.syntax),
                );
                continue;
            }
            match (spec.block.is_some(), &directive.body) {
                (true, Body::Semicolon) => {
                    self.error_with_help(
                        file,
                        directive.span,
                        codes::ARGUMENTS,
                        format!("'{name}' needs a block"),
                        format!("write it as `{}`", spec.syntax),
                    );
                    continue;
                }
                (false, Body::Block(_)) => {
                    self.error_with_help(
                        file,
                        directive.span,
                        codes::ARGUMENTS,
                        format!("'{name}' does not take a block"),
                        format!("write it as `{}`", spec.syntax),
                    );
                    continue;
                }
                _ => {}
            }
            if let Some(deprecation) = spec.deprecated {
                let diagnostic = Diagnostic::warning(
                    codes::DEPRECATED,
                    format!(
                        "'{name}' is deprecated; it is read as '{}'",
                        deprecation.replacement
                    ),
                )
                .with_help(format!(
                    "use '{}'; language version {} no longer accepts '{name}'",
                    deprecation.replacement, deprecation.removed_in
                ));
                self.report(diagnostic, file, directive.name.span);
            }
            for arg in &directive.args {
                if let Some(intended) = values::quoted_parameter(arg) {
                    let diagnostic = Diagnostic::warning(
                        codes::QUOTES,
                        format!("the quotes in {:?} are part of the value", arg.value),
                    )
                    .with_help(format!("write it as {}", panel_dsl::quote(&intended)));
                    self.report(diagnostic, file, arg.span);
                }
            }
            if !spec.repeatable && !seen.insert(spec.name) {
                self.error(
                    file,
                    directive.name.span,
                    codes::DUPLICATE,
                    format!("'{name}' is already set in this block"),
                );
                continue;
            }
            match name {
                "include" => self.include(file, directive, context, seen, handle),
                "set" => self.set(file, directive),
                _ => handle(self, file, directive, spec, depth),
            }
        }
    }

    fn misplaced(&mut self, file: &str, directive: &Directive, context: Context) {
        let name = directive.name.value.as_str();
        let contexts = schema::contexts_of(name);
        if contexts.is_empty() {
            let mut diagnostic = Diagnostic::error(
                codes::UNKNOWN_DIRECTIVE,
                format!("unknown directive '{name}'"),
            );
            if let Some(suggestion) = schema::suggestion(name, context) {
                diagnostic = diagnostic.with_help(format!("did you mean '{suggestion}'?"));
            }
            self.report(diagnostic, file, directive.name.span);
        } else {
            let places: Vec<_> = contexts.iter().map(|context| context.name()).collect();
            self.error_with_help(
                file,
                directive.name.span,
                codes::CONTEXT,
                format!("'{name}' is not allowed in {}", context.name()),
                format!("it belongs in {}", places.join(" or ")),
            );
        }
    }

    fn include(
        &mut self,
        file: &str,
        directive: &Directive,
        context: Context,
        seen: &mut BTreeSet<&'static str>,
        handle: Handler<'_, 'a>,
    ) {
        let pattern = &directive.args[0];
        let matches = match self.sources.resolve(file, &pattern.value) {
            Ok(matches) => matches,
            Err(message) => {
                self.error(file, pattern.span, codes::INCLUDE, message);
                return;
            }
        };
        if matches.is_empty() && !source::is_glob(&pattern.value) {
            self.error(
                file,
                pattern.span,
                codes::INCLUDE,
                format!("{:?} is not a file of this configuration", pattern.value),
            );
            return;
        }
        for path in matches {
            if let Some(position) = self.includes.iter().position(|open| *open == path) {
                let mut chain = self.includes[position..].to_vec();
                chain.push(path.clone());
                self.error(
                    file,
                    pattern.span,
                    codes::INCLUDE,
                    format!(
                        "files include each other in a cycle: {}",
                        chain.join(" -> ")
                    ),
                );
                continue;
            }
            let Some(document) = self.documents.get(&path).cloned() else {
                continue;
            };
            self.includes.push(path.clone());
            self.each(&path, &document.directives, context, 0, seen, handle);
            self.includes.pop();
        }
    }

    fn set(&mut self, file: &str, directive: &Directive) {
        let name_arg = &directive.args[0];
        let Some(name) = name_arg
            .value
            .strip_prefix('$')
            .filter(|name| matches!(variables::pieces(name_arg.value.as_str()).as_deref(), Ok([Piece::Variable(variable)]) if variable == name))
        else {
            self.error(file, name_arg.span, codes::VARIABLE, format!("{:?} is not a variable name such as $name", name_arg.value));
            return;
        };
        if variables::is_request_variable(name) {
            self.error(
                file,
                name_arg.span,
                codes::VARIABLE,
                format!("${name} is a request variable and cannot be set"),
            );
            return;
        }
        let Some(value) = self.expand(file, &directive.args[1], &directive.args[1].value, false)
        else {
            return;
        };
        let name = name.to_owned();
        self.scopes.last_mut().expect("a scope").insert(name, value);
    }

    /// Resolves constants and environment references in `value`; request
    /// variables stay as written where `request` allows them.
    fn expand(&mut self, file: &str, arg: &Argument, value: &str, request: bool) -> Option<String> {
        let pieces = match variables::pieces(value) {
            Ok(pieces) => pieces,
            Err(message) => {
                self.error(file, arg.span, codes::VARIABLE, message);
                return None;
            }
        };
        let mut out = String::with_capacity(value.len());
        for piece in pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Environment(name) => {
                    if !name.starts_with(ENVIRONMENT_PREFIX) {
                        self.error_with_help(
                            file,
                            arg.span,
                            codes::VARIABLE,
                            format!("${{env:{name}}} is not readable"),
                            format!(
                                "only variables starting with {ENVIRONMENT_PREFIX} can be read"
                            ),
                        );
                        return None;
                    }
                    match self.options.environment.get(name) {
                        Some(found) => out.push_str(found),
                        None => {
                            self.error(
                                file,
                                arg.span,
                                codes::VARIABLE,
                                format!("the environment does not define {name}"),
                            );
                            return None;
                        }
                    }
                }
                Piece::Variable(name) => {
                    if let Some(found) = self.scopes.iter().rev().find_map(|scope| scope.get(name))
                    {
                        out.push_str(found);
                    } else if variables::is_request_variable(name) {
                        if !request {
                            self.error(
                                file,
                                arg.span,
                                codes::VARIABLE,
                                format!(
                                    "${name} is known only per request and cannot be used here"
                                ),
                            );
                            return None;
                        }
                        out.push('$');
                        out.push_str(name);
                    } else {
                        self.error(
                            file,
                            arg.span,
                            codes::VARIABLE,
                            format!("${name} is not defined"),
                        );
                        return None;
                    }
                }
            }
        }
        Some(out)
    }

    /// The expanded value of a whole argument.
    fn value(&mut self, file: &str, arg: &Argument) -> Option<String> {
        self.expand(file, arg, &arg.value.clone(), false)
    }

    /// An argument taken literally: names, references and free text.
    fn literal(arg: &Argument) -> String {
        arg.value.clone()
    }

    fn language_version(&mut self, file: &str, directive: &Directive) {
        let arg = &directive.args[0];
        if arg.value != LANGUAGE_VERSION.to_string() {
            self.error_with_help(
                file,
                arg.span,
                codes::VERSION,
                format!("language version {:?} is not supported", arg.value),
                format!("this release reads language version {LANGUAGE_VERSION}"),
            );
        }
    }

    fn with_scope(&mut self, run: impl FnOnce(&mut Self)) {
        self.scopes.push(BTreeMap::new());
        run(self);
        self.scopes.pop();
    }

    fn http(&mut self, file: &str, directive: &Directive, depth: usize) {
        let Some(block) = directive.block() else {
            return;
        };
        self.with_scope(|lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::Http,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, depth| match spec.name {
                    "tls_profile" => lowerer.tls_profile(file, directive, depth),
                    "listener" => lowerer.listener(file, directive, depth),
                    "upstream" => lowerer.upstream(file, directive, depth),
                    "server" => lowerer.server(file, directive, depth),
                    _ => unreachable!("the schema allows nothing else in http"),
                },
            );
        });
    }

    fn origin(file: &str, directive: &Directive, depth: usize) -> Origin {
        Origin {
            file: file.to_owned(),
            span: directive.span,
            outer: directive.span_with_leading(),
            depth,
        }
    }

    fn bool_arg(&mut self, file: &str, arg: &Argument) -> Option<bool> {
        let value = self.value(file, arg)?;
        let parsed = values::parse_bool(&value);
        if parsed.is_none() {
            self.error(
                file,
                arg.span,
                codes::TYPE,
                format!("{value:?} is not on or off"),
            );
        }
        parsed
    }

    fn number<T: std::str::FromStr>(
        &mut self,
        file: &str,
        arg: &Argument,
        value: &str,
        what: &str,
    ) -> Option<T> {
        let parsed = value
            .parse::<T>()
            .ok()
            .filter(|_| value.bytes().all(|byte| byte.is_ascii_digit()));
        if parsed.is_none() {
            self.error(
                file,
                arg.span,
                codes::TYPE,
                format!("{value:?} is not {what}"),
            );
        }
        parsed
    }

    fn duration(&mut self, file: &str, arg: &Argument, value: &str) -> Option<u64> {
        let parsed = values::parse_duration_ms(value);
        if parsed.is_none() {
            self.error_with_help(
                file,
                arg.span,
                codes::TYPE,
                format!("{value:?} is not a duration"),
                "write durations such as 500ms, 30s, 5m or 1h",
            );
        }
        parsed
    }

    fn host(&mut self, file: &str, arg: &Argument, value: &str) -> Option<NormalizedHost> {
        match NormalizedHost::new(value) {
            Ok(host) => Some(host),
            Err(error) => {
                self.error(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{value:?} is not a valid host: {error}"),
                );
                None
            }
        }
    }

    fn id(&mut self, file: &str, block: &[Directive]) -> Option<Uuid> {
        let directive = block
            .iter()
            .find(|directive| directive.name.value == "id" && !directive.args.is_empty())?;
        let arg = &directive.args[0];
        match Uuid::parse_str(&arg.value) {
            Ok(id) => Some(id),
            Err(_) => {
                self.error(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{:?} is not a UUID", arg.value),
                );
                None
            }
        }
    }

    /// The block's `id`, or a new one recorded so it can be written back
    /// on the line after the opening brace.
    fn identity(&mut self, file: &str, directive: &Directive, depth: usize) -> Uuid {
        let block = directive
            .block()
            .map_or(&[][..], |block| &block.directives[..]);
        self.id(file, block).unwrap_or_else(|| {
            let id = Uuid::now_v7();
            if let Some(block) = directive.block() {
                let text = self.sources.get(file).unwrap_or_default();
                let after = block.open.end;
                let (offset, text) = match text[after..].find('\n') {
                    Some(line_end) => (
                        after + line_end,
                        format!("\n{}id {id};", "    ".repeat(depth + 1)),
                    ),
                    None => (after, format!(" id {id};")),
                };
                self.insertions.push(Insertion {
                    file: file.to_owned(),
                    offset,
                    text,
                });
            }
            id
        })
    }

    fn tls_profile(&mut self, file: &str, directive: &Directive, depth: usize) {
        let id = Self::literal(&directive.args[0]);
        if self.profiles.iter().any(|(profile, _)| profile.id == id) {
            self.error(
                file,
                directive.args[0].span,
                codes::DUPLICATE,
                format!("TLS profile {id:?} is defined twice"),
            );
            return;
        }
        let mut profile = TlsProfile {
            id,
            certificate_secret_id: String::new(),
            private_key_secret_id: String::new(),
            min_protocol: "TLSv1.2".into(),
            alpn: BTreeSet::new(),
        };
        let Some(block) = directive.block() else {
            return;
        };
        self.with_scope(|lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::TlsProfile,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, _| {
                    let arg = &directive.args[0];
                    match spec.name {
                        "certificate" => {
                            profile.certificate_secret_id =
                                lowerer.value(file, arg).unwrap_or_default()
                        }
                        "key" => {
                            profile.private_key_secret_id =
                                lowerer.value(file, arg).unwrap_or_default()
                        }
                        "min_protocol" => {
                            if let Some(value) = lowerer.value(file, arg) {
                                if matches!(value.as_str(), "TLSv1.2" | "TLSv1.3") {
                                    profile.min_protocol = value;
                                } else {
                                    lowerer.error(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        format!("{value:?} is not TLSv1.2 or TLSv1.3"),
                                    );
                                }
                            }
                        }
                        "alpn" => {
                            for arg in &directive.args {
                                match arg.value.as_str() {
                                    "h2" | "http/1.1" => {
                                        profile.alpn.insert(arg.value.clone());
                                    }
                                    other => lowerer.error(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        format!("{other:?} is not h2 or http/1.1"),
                                    ),
                                }
                            }
                        }
                        _ => unreachable!(),
                    }
                },
            );
        });
        for (field, value) in [
            ("certificate", &profile.certificate_secret_id),
            ("key", &profile.private_key_secret_id),
        ] {
            if value.is_empty() {
                self.error(
                    file,
                    directive.span,
                    codes::ARGUMENTS,
                    format!("TLS profile {:?} needs '{field}'", profile.id),
                );
            }
        }
        let origin = Self::origin(file, directive, depth);
        self.origins
            .insert(format!("tls-profiles/{}", profile.id), origin.clone());
        self.profiles.push((profile, origin));
    }

    fn listener(&mut self, file: &str, directive: &Directive, depth: usize) {
        let id = Self::literal(&directive.args[0]);
        if self.listeners.iter().any(|draft| draft.listener.id == id) {
            self.error(
                file,
                directive.args[0].span,
                codes::DUPLICATE,
                format!("listener {id:?} is defined twice"),
            );
            return;
        }
        let mut listener = Listener {
            id,
            address: String::new(),
            tls_profile_id: None,
            protocols: ListenerProtocols::default(),
            reuse_port: false,
            ipv6_only: None,
            default_site_id: None,
        };
        let mut default_server = None;
        let Some(block) = directive.block() else {
            return;
        };
        self.with_scope(|lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::Listener,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, _| {
                    let arg = &directive.args[0];
                    match spec.name {
                        "address" => {
                            if let Some(value) = lowerer.value(file, arg) {
                                if values::parse_socket_address(&value).is_some() {
                                    listener.address = value;
                                } else {
                                    lowerer.error_with_help(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        format!("{value:?} is not an IP address and port"),
                                        "write addresses such as 0.0.0.0:80 or [::]:443",
                                    );
                                }
                            }
                        }
                        "protocols" => {
                            let mut protocols = ListenerProtocols {
                                http1: false,
                                http2: false,
                                http3: false,
                            };
                            for arg in &directive.args {
                                match arg.value.as_str() {
                                    "http1" => protocols.http1 = true,
                                    "http2" => protocols.http2 = true,
                                    "http3" => protocols.http3 = true,
                                    other => lowerer.error(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        format!("{other:?} is not http1, http2 or http3"),
                                    ),
                                }
                            }
                            listener.protocols = protocols;
                        }
                        "tls_profile" => listener.tls_profile_id = Some(Self::literal(arg)),
                        "reuse_port" => {
                            listener.reuse_port = lowerer.bool_arg(file, arg).unwrap_or_default()
                        }
                        "ipv6_only" => listener.ipv6_only = lowerer.bool_arg(file, arg),
                        "default_server" => {
                            default_server = Some((Self::literal(arg), file.to_owned(), arg.span))
                        }
                        _ => unreachable!(),
                    }
                },
            );
        });
        if listener.address.is_empty()
            && !directive
                .block()
                .is_some_and(|block| block.directives.iter().any(|d| d.name.value == "address"))
        {
            self.error(
                file,
                directive.span,
                codes::ARGUMENTS,
                format!("listener {:?} needs 'address'", listener.id),
            );
        }
        self.origins.insert(
            format!("listeners/{}", listener.id),
            Self::origin(file, directive, depth),
        );
        self.listeners.push(ListenerDraft {
            listener,
            default_server,
        });
    }

    fn upstream(&mut self, file: &str, directive: &Directive, depth: usize) {
        let name = Self::literal(&directive.args[0]);
        if self
            .upstreams
            .iter()
            .any(|(upstream, _)| upstream.name.eq_ignore_ascii_case(&name))
        {
            self.error(
                file,
                directive.args[0].span,
                codes::DUPLICATE,
                format!("upstream {name:?} is defined twice"),
            );
            return;
        }
        let id = self.identity(file, directive, depth);
        let now = self.options.now;
        let mut upstream = Upstream {
            id,
            name,
            nodes: Vec::new(),
            balancing: LoadBalancingPolicy::RoundRobin,
            host_header: None,
            tls: UpstreamTlsPolicy::default(),
            connection: UpstreamConnectionPolicy::default(),
            health_check: None,
            passive_health: None,
            note: None,
            created_at: now,
            updated_at: now,
        };
        let Some(block) = directive.block() else {
            return;
        };
        self.with_scope(|lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::Upstream,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, _| {
                    lowerer.upstream_directive(file, directive, spec, &mut upstream);
                },
            );
        });
        let origin = Self::origin(file, directive, depth);
        self.origins
            .insert(format!("upstreams/{}", upstream.id), origin.clone());
        self.upstreams.push((upstream, origin));
    }

    fn upstream_directive(
        &mut self,
        file: &str,
        directive: &Directive,
        spec: &DirectiveSpec,
        upstream: &mut Upstream,
    ) {
        let arg = &directive.args[0];
        let duration = |lowerer: &mut Self| {
            let value = lowerer.value(file, arg)?;
            lowerer.duration(file, arg, &value)
        };
        match spec.name {
            "id" => {}
            "server" => {
                if let Some(node) = self.node(file, directive) {
                    if upstream.nodes.iter().any(|other| other.id == node.id) {
                        self.error(
                            file,
                            directive.span,
                            codes::DUPLICATE,
                            format!("node id {} is used twice", node.id),
                        );
                    } else {
                        upstream.nodes.push(node);
                    }
                }
            }
            "balance" => {
                let params = Params::split(&directive.args);
                let kind = params.positional.first().map(|arg| arg.value.as_str());
                upstream.balancing = match (kind, params.named.get("key")) {
                    (Some("round_robin"), None) => LoadBalancingPolicy::RoundRobin,
                    (Some("random"), None) => LoadBalancingPolicy::Random,
                    (Some("hash"), Some((key, key_arg))) => {
                        let Some(key) = self
                            .expand(file, key_arg, key, true)
                            .and_then(|key| variables::hash_key(&key))
                        else {
                            self.error_with_help(
                                file,
                                key_arg.span,
                                codes::TYPE,
                                format!("{key:?} is not a hash key"),
                                "use $client_ip, $uri, $http_<name> or $cookie_<name>",
                            );
                            return;
                        };
                        LoadBalancingPolicy::ConsistentHash { key }
                    }
                    (Some("hash"), None) => {
                        self.error_with_help(
                            file,
                            directive.span,
                            codes::ARGUMENTS,
                            "hashing needs a key",
                            "write `balance hash key=$client_ip;`",
                        );
                        return;
                    }
                    _ => {
                        self.error_with_help(
                            file,
                            directive.span,
                            codes::ARGUMENTS,
                            "unknown balancing",
                            format!("write it as `{}`", spec.syntax),
                        );
                        return;
                    }
                };
            }
            "host_header" => upstream.host_header = self.value(file, arg),
            "tls" => {
                let params = Params::split(&directive.args);
                self.only_params(file, &params, &["verify", "verify_hostname", "sni", "ca"]);
                for (key, (value, arg)) in &params.named {
                    match *key {
                        "verify" => {
                            upstream.tls.verify_certificate =
                                self.flag(file, arg, value).unwrap_or(true)
                        }
                        "verify_hostname" => {
                            upstream.tls.verify_hostname =
                                self.flag(file, arg, value).unwrap_or(true)
                        }
                        "sni" => upstream.tls.sni = self.expand(file, arg, value, false),
                        "ca" => upstream.tls.ca_secret_id = self.expand(file, arg, value, false),
                        _ => {}
                    }
                }
            }
            "connect_timeout" => upstream.connection.connect_timeout_ms = duration(self),
            "read_timeout" => upstream.connection.read_timeout_ms = duration(self),
            "write_timeout" => upstream.connection.write_timeout_ms = duration(self),
            "idle_timeout" => upstream.connection.idle_timeout_ms = duration(self),
            "keepalive" => upstream.connection.keepalive = self.bool_arg(file, arg).unwrap_or(true),
            "max_connections" => {
                if let Some(value) = self.value(file, arg) {
                    upstream.connection.max_connections =
                        self.number(file, arg, &value, "a whole number");
                }
            }
            "http2" => upstream.connection.http2 = self.bool_arg(file, arg).unwrap_or_default(),
            "health_check" => upstream.health_check = self.health_check(file, directive),
            "passive_health" => {
                let params = Params::split(&directive.args);
                self.only_params(file, &params, &["fails", "eject"]);
                let mut policy = PassiveHealthPolicy {
                    failure_threshold: 5,
                    ejection_ms: 30_000,
                };
                if let Some((value, arg)) = params.named.get("fails") {
                    policy.failure_threshold =
                        self.number(file, arg, value, "a whole number").unwrap_or(5);
                }
                if let Some((value, arg)) = params.named.get("eject") {
                    policy.ejection_ms = self.duration(file, arg, value).unwrap_or(30_000);
                }
                upstream.passive_health = Some(policy);
            }
            "note" => upstream.note = Some(Self::literal(arg)),
            _ => unreachable!(),
        }
    }

    fn flag(&mut self, file: &str, arg: &Argument, value: &str) -> Option<bool> {
        let parsed = values::parse_bool(value);
        if parsed.is_none() {
            self.error(
                file,
                arg.span,
                codes::TYPE,
                format!("{value:?} is not on or off"),
            );
        }
        parsed
    }

    /// Reports parameters and positional arguments other than `allowed`.
    fn only_params(&mut self, file: &str, params: &Params<'_>, allowed: &[&str]) {
        for arg in &params.repeated {
            self.error(
                file,
                arg.span,
                codes::DUPLICATE,
                format!("{:?} repeats a parameter", arg.value),
            );
        }
        for (key, (_, arg)) in &params.named {
            if !allowed.contains(key) {
                self.error_with_help(
                    file,
                    arg.span,
                    codes::ARGUMENTS,
                    format!("unknown parameter {key:?}"),
                    format!("expected one of {}", allowed.join(", ")),
                );
            }
        }
    }

    fn node(&mut self, file: &str, directive: &Directive) -> Option<UpstreamNode> {
        let params = Params::split(&directive.args);
        let Some((address_arg, flags)) = params.positional.split_first() else {
            self.error(
                file,
                directive.span,
                codes::ARGUMENTS,
                "a node needs an address",
            );
            return None;
        };
        let address = self.value(file, address_arg)?;
        let Some((host, port)) = split_host_port(&address) else {
            self.error_with_help(
                file,
                address_arg.span,
                codes::TYPE,
                format!("{address:?} is not host:port"),
                "write nodes such as 10.0.0.1:8080, app.internal:80 or [2001:db8::1]:443",
            );
            return None;
        };
        let mut node = UpstreamNode {
            id: Uuid::nil(),
            host,
            port,
            tls: false,
            weight: 1,
            enabled: true,
            backup: false,
            sni: None,
            unix_socket: None,
            note: None,
        };
        for flag in flags {
            match flag.value.as_str() {
                "backup" => node.backup = true,
                "down" => node.enabled = false,
                "tls" => node.tls = true,
                other => self.error_with_help(
                    file,
                    flag.span,
                    codes::ARGUMENTS,
                    format!("unknown node flag {other:?}"),
                    "expected backup, down or tls",
                ),
            }
        }
        self.only_params(file, &params, &["weight", "sni", "id", "note", "unix"]);
        for (key, (value, arg)) in &params.named {
            match *key {
                "weight" => {
                    node.weight = self.number(file, arg, value, "a whole number").unwrap_or(1)
                }
                "sni" => node.sni = self.expand(file, arg, value, false),
                "note" => node.note = Some((*value).to_owned()),
                "unix" => node.unix_socket = self.expand(file, arg, value, false),
                "id" => match Uuid::parse_str(value) {
                    Ok(id) => node.id = id,
                    Err(_) => self.error(
                        file,
                        arg.span,
                        codes::TYPE,
                        format!("{value:?} is not a UUID"),
                    ),
                },
                _ => {}
            }
        }
        if node.id.is_nil() {
            node.id = Uuid::now_v7();
            if let Body::Semicolon = directive.body {
                self.insertions.push(Insertion {
                    file: file.to_owned(),
                    offset: directive.span.end - 1,
                    text: format!(" id={}", node.id),
                });
            }
        }
        Some(node)
    }

    fn health_check(&mut self, file: &str, directive: &Directive) -> Option<ActiveHealthCheck> {
        let params = Params::split(&directive.args);
        let protocol = match params.positional.first().map(|arg| arg.value.as_str()) {
            Some("http") => HealthCheckProtocol::Http,
            Some("tcp") => HealthCheckProtocol::Tcp,
            _ => {
                self.error_with_help(
                    file,
                    directive.span,
                    codes::ARGUMENTS,
                    "a health check is http or tcp",
                    "write `health_check http path=/healthz;`",
                );
                return None;
            }
        };
        if let Some(extra) = params.positional.get(1) {
            self.error(
                file,
                extra.span,
                codes::ARGUMENTS,
                format!("unexpected {:?}", extra.value),
            );
        }
        self.only_params(
            file,
            &params,
            &[
                "path", "method", "host", "interval", "timeout", "rise", "fall", "status",
            ],
        );
        let mut check = ActiveHealthCheck {
            protocol,
            path: "/".into(),
            method: "GET".into(),
            interval_ms: 5_000,
            timeout_ms: 1_000,
            healthy_threshold: 2,
            unhealthy_threshold: 3,
            expected_statuses: BTreeSet::new(),
            host: None,
        };
        for (key, (value, arg)) in &params.named {
            match *key {
                "path" => check.path = self.expand(file, arg, value, false).unwrap_or_default(),
                "method" => check.method = value.to_ascii_uppercase(),
                "host" => check.host = self.expand(file, arg, value, false),
                "interval" => check.interval_ms = self.duration(file, arg, value).unwrap_or(5_000),
                "timeout" => check.timeout_ms = self.duration(file, arg, value).unwrap_or(1_000),
                "rise" => {
                    check.healthy_threshold =
                        self.number(file, arg, value, "a whole number").unwrap_or(2)
                }
                "fall" => {
                    check.unhealthy_threshold =
                        self.number(file, arg, value, "a whole number").unwrap_or(3)
                }
                "status" => {
                    for status in value.split(',') {
                        if let Some(status) =
                            self.number::<u16>(file, arg, status, "an HTTP status")
                        {
                            check.expected_statuses.insert(status);
                        }
                    }
                }
                _ => {}
            }
        }
        Some(check)
    }

    fn server(&mut self, file: &str, directive: &Directive, depth: usize) {
        let name = Self::literal(&directive.args[0]);
        if self
            .servers
            .iter()
            .any(|draft| draft.site.name.eq_ignore_ascii_case(&name))
        {
            self.error(
                file,
                directive.args[0].span,
                codes::DUPLICATE,
                format!("server {name:?} is defined twice"),
            );
            return;
        }
        let id = self.identity(file, directive, depth);
        let now = self.options.now;
        let mut draft = ServerDraft {
            site: Site {
                id,
                name,
                action: placeholder_action(),
                enabled: true,
                domains: Vec::new(),
                routes: Vec::new(),
                listener_ids: BTreeSet::new(),
                https_redirect: false,
                www_redirect: WwwRedirect::None,
                tls_profile_id: None,
                group: None,
                tags: BTreeSet::new(),
                note: None,
                favorite: false,
                deleted_at: None,
                created_at: now,
                updated_at: now,
            },
            action: None,
            routes: Vec::new(),
            origin: Self::origin(file, directive, depth),
        };
        let Some(block) = directive.block() else {
            return;
        };
        let mut explicit_primary = false;
        self.with_scope(|lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::Server,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, depth| {
                    lowerer.server_directive(
                        file,
                        directive,
                        spec,
                        depth,
                        &mut draft,
                        &mut explicit_primary,
                    );
                },
            );
        });
        if !explicit_primary {
            if let Some(first) = draft
                .site
                .domains
                .iter_mut()
                .find(|domain| !domain.redirect)
            {
                first.primary = true;
            }
        }
        if draft.action.is_none() {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                format!("server {:?} has no action", draft.site.name),
                "add one of proxy, root, return or respond",
            );
        }
        self.origins
            .insert(format!("sites/{}", draft.site.id), draft.origin.clone());
        self.servers.push(draft);
    }

    fn add_domain(&mut self, file: &str, arg: &Argument, draft: &mut ServerDraft, domain: Domain) {
        if draft
            .site
            .domains
            .iter()
            .any(|other| other.host == domain.host)
        {
            self.error(
                file,
                arg.span,
                codes::DUPLICATE,
                format!("{} is listed twice in this server", domain.host),
            );
        } else {
            draft.site.domains.push(domain);
        }
    }

    fn server_directive(
        &mut self,
        file: &str,
        directive: &Directive,
        spec: &DirectiveSpec,
        depth: usize,
        draft: &mut ServerDraft,
        explicit_primary: &mut bool,
    ) {
        let arg = directive.args.first();
        match spec.name {
            "id" => {}
            "server_name" | "alias" => {
                for arg in &directive.args {
                    let Some(value) = self.value(file, arg) else {
                        continue;
                    };
                    if let Some(host) = self.host(file, arg, &value) {
                        let domain = Domain {
                            host,
                            enabled: true,
                            primary: false,
                            redirect: spec.name == "alias",
                            tls_profile_id: None,
                        };
                        self.add_domain(file, arg, draft, domain);
                    }
                }
            }
            "domain" => {
                let params = Params::split(&directive.args);
                let Some((host_arg, flags)) = params.positional.split_first() else {
                    return;
                };
                let Some(value) = self.value(file, host_arg) else {
                    return;
                };
                let Some(host) = self.host(file, host_arg, &value) else {
                    return;
                };
                let mut domain = Domain {
                    host,
                    enabled: true,
                    primary: false,
                    redirect: false,
                    tls_profile_id: None,
                };
                for flag in flags {
                    match flag.value.as_str() {
                        "primary" => {
                            domain.primary = true;
                            *explicit_primary = true;
                        }
                        "alias" => domain.redirect = true,
                        "off" => domain.enabled = false,
                        other => self.error_with_help(
                            file,
                            flag.span,
                            codes::ARGUMENTS,
                            format!("unknown domain flag {other:?}"),
                            "expected primary, alias or off",
                        ),
                    }
                }
                self.only_params(file, &params, &["tls_profile"]);
                if let Some((value, _)) = params.named.get("tls_profile") {
                    domain.tls_profile_id = Some((*value).to_owned());
                }
                self.add_domain(file, host_arg, draft, domain);
            }
            "listen" => draft
                .site
                .listener_ids
                .extend(directive.args.iter().map(Self::literal)),
            "tls_profile" => draft.site.tls_profile_id = arg.map(Self::literal),
            "https_redirect" => {
                draft.site.https_redirect = arg
                    .and_then(|arg| self.bool_arg(file, arg))
                    .unwrap_or_default()
            }
            "www_redirect" => {
                let Some(arg) = arg else { return };
                draft.site.www_redirect = match arg.value.as_str() {
                    "off" => WwwRedirect::None,
                    "add" => WwwRedirect::AddWww,
                    "remove" => WwwRedirect::RemoveWww,
                    other => {
                        self.error(
                            file,
                            arg.span,
                            codes::TYPE,
                            format!("{other:?} is not off, add or remove"),
                        );
                        return;
                    }
                };
            }
            "enabled" => {
                draft.site.enabled = arg.and_then(|arg| self.bool_arg(file, arg)).unwrap_or(true)
            }
            "group" => draft.site.group = arg.map(Self::literal),
            "tags" => draft
                .site
                .tags
                .extend(directive.args.iter().map(Self::literal)),
            "note" => draft.site.note = arg.map(Self::literal),
            "route" | "location" => {
                if let Some(route) = self.route(file, directive, depth, &draft.site) {
                    draft.routes.push(route);
                }
            }
            action => {
                let Some(found) = self.action(file, directive, action) else {
                    return;
                };
                if draft.action.is_some() {
                    self.error(
                        file,
                        directive.name.span,
                        codes::DUPLICATE,
                        "the server already has an action",
                    );
                } else {
                    draft.action = Some(found);
                }
            }
        }
    }

    fn route(
        &mut self,
        file: &str,
        directive: &Directive,
        depth: usize,
        site: &Site,
    ) -> Option<RouteDraft> {
        let location = directive.name.value == "location";
        let id = self.identity(file, directive, depth);
        let mut draft = RouteDraft {
            route: Route {
                id,
                name: None,
                enabled: true,
                priority: 0,
                matcher: RouteMatch {
                    kind: MatchKind::Prefix,
                    path: "/".into(),
                    host: None,
                },
                action: placeholder_action(),
            },
            priority_set: false,
            action: None,
            origin: Self::origin(file, directive, depth),
        };
        let mut matched = false;
        if location {
            let (modifier, path) = match directive.args.as_slice() {
                [path] => (None, path),
                [modifier, path] => (Some(modifier), path),
                _ => return None,
            };
            let (kind, prefix) = match modifier.map(|modifier| modifier.value.as_str()) {
                None | Some("^~") => (MatchKind::Prefix, ""),
                Some("=") => (MatchKind::Exact, ""),
                Some("~") => (MatchKind::Regex, ""),
                Some("~*") => (MatchKind::Regex, "(?i)"),
                Some(other) => {
                    let span = modifier.map_or(directive.span, |modifier| modifier.span);
                    self.error(
                        file,
                        span,
                        codes::TYPE,
                        format!("{other:?} is not a location modifier"),
                    );
                    return None;
                }
            };
            draft.route.matcher = RouteMatch {
                kind,
                path: format!("{prefix}{}", path.value),
                host: None,
            };
            matched = true;
        } else if let Some(name) = directive.args.first() {
            draft.route.name = Some(Self::literal(name));
        }
        let block = directive.block()?;
        self.with_scope(|lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::Route,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, _| {
                    let arg = &directive.args.first();
                    match spec.name {
                        "id" => {}
                        "match" => {
                            let params = Params::split(&directive.args);
                            lowerer.only_params(file, &params, &["host"]);
                            let [kind_arg, path_arg] = params.positional.as_slice() else {
                                lowerer.error_with_help(
                                    file,
                                    directive.span,
                                    codes::ARGUMENTS,
                                    "a match is a kind and a path",
                                    format!("write it as `{}`", spec.syntax),
                                );
                                return;
                            };
                            let kind = match kind_arg.value.as_str() {
                                "exact" => MatchKind::Exact,
                                "prefix" => MatchKind::Prefix,
                                "glob" => MatchKind::Glob,
                                "regex" => MatchKind::Regex,
                                other => {
                                    lowerer.error(
                                        file,
                                        kind_arg.span,
                                        codes::TYPE,
                                        format!("{other:?} is not exact, prefix, glob or regex"),
                                    );
                                    return;
                                }
                            };
                            let path = if kind == MatchKind::Regex {
                                path_arg.value.clone()
                            } else {
                                match lowerer.value(file, path_arg) {
                                    Some(path) => path,
                                    None => return,
                                }
                            };
                            let host = match params.named.get("host") {
                                Some((value, arg)) => {
                                    let Some(value) = lowerer.expand(file, arg, value, false)
                                    else {
                                        return;
                                    };
                                    lowerer.host(file, arg, &value)
                                }
                                None => None,
                            };
                            draft.route.matcher = RouteMatch { kind, path, host };
                            matched = true;
                        }
                        "priority" => {
                            if let Some(arg) = arg {
                                if let Some(value) = lowerer.value(file, arg) {
                                    draft.route.priority = lowerer
                                        .number(file, arg, &value, "a whole number")
                                        .unwrap_or_default();
                                    draft.priority_set = true;
                                }
                            }
                        }
                        "enabled" => {
                            draft.route.enabled = arg
                                .and_then(|arg| lowerer.bool_arg(file, arg))
                                .unwrap_or(true)
                        }
                        action => {
                            let Some(found) = lowerer.action(file, directive, action) else {
                                return;
                            };
                            if draft.action.is_some() {
                                lowerer.error(
                                    file,
                                    directive.name.span,
                                    codes::DUPLICATE,
                                    "the route already has an action",
                                );
                            } else {
                                draft.action = Some(found);
                            }
                        }
                    }
                },
            );
        });
        if !matched {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                "the route has no match",
                "add `match prefix /path;`",
            );
        }
        if draft.action.is_none() {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                "the route has no action",
                "add one of proxy, root, return or respond",
            );
        }
        self.origins.insert(
            format!("sites/{}/routes/{}", site.id, draft.route.id),
            draft.origin.clone(),
        );
        Some(draft)
    }

    fn action(&mut self, file: &str, directive: &Directive, name: &str) -> Option<ActionDraft> {
        let params = Params::split(&directive.args);
        let first = params.positional.first().copied();
        match name {
            "proxy" | "proxy_pass" => {
                let arg = first?;
                let value = Self::literal(arg);
                let upstream = if name == "proxy_pass" {
                    value
                        .strip_prefix("http://")
                        .or_else(|| value.strip_prefix("https://"))
                        .unwrap_or(&value)
                        .trim_end_matches('/')
                        .to_owned()
                } else {
                    value
                };
                Some(ActionDraft::Proxy {
                    upstream,
                    file: file.to_owned(),
                    span: arg.span,
                })
            }
            "root" => {
                self.only_params(file, &params, &["index", "spa"]);
                let root = self.value(file, first?)?;
                let mut index_files = vec!["index.html".to_owned()];
                if let Some((value, arg)) = params.named.get("index") {
                    index_files = self
                        .expand(file, arg, value, false)?
                        .split(',')
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
                let spa_fallback = match params.named.get("spa") {
                    Some((value, arg)) => self.flag(file, arg, value).unwrap_or_default(),
                    None => false,
                };
                Some(ActionDraft::Ready(Action::Static {
                    root,
                    index_files,
                    spa_fallback,
                }))
            }
            "return" => {
                self.only_params(file, &params, &["preserve_path"]);
                let [code_arg, location_arg] = params.positional.as_slice() else {
                    self.error_with_help(
                        file,
                        directive.span,
                        codes::ARGUMENTS,
                        "a redirect is a status and a URL",
                        "write `return 308 https://example.com;`",
                    );
                    return None;
                };
                let status: u16 =
                    self.number(file, code_arg, &code_arg.value, "a redirect status")?;
                if !matches!(status, 301 | 302 | 303 | 307 | 308) {
                    self.error_with_help(
                        file,
                        code_arg.span,
                        codes::TYPE,
                        format!("{status} is not a redirect status"),
                        "use 301, 302, 303, 307 or 308, or respond for other statuses",
                    );
                    return None;
                }
                let location = self.value(file, location_arg)?;
                let preserve_path = match params.named.get("preserve_path") {
                    Some((value, arg)) => self.flag(file, arg, value).unwrap_or(true),
                    None => true,
                };
                Some(ActionDraft::Ready(Action::Redirect {
                    location,
                    status,
                    preserve_path,
                }))
            }
            "respond" => {
                self.only_params(file, &params, &["body", "type", "retry_after"]);
                let code_arg = first?;
                let status: u16 = self.number(file, code_arg, &code_arg.value, "an HTTP status")?;
                if !(100..=599).contains(&status) {
                    self.error(
                        file,
                        code_arg.span,
                        codes::TYPE,
                        format!("{status} is not an HTTP status"),
                    );
                    return None;
                }
                if let Some(extra) = params.positional.get(1) {
                    self.error_with_help(
                        file,
                        extra.span,
                        codes::ARGUMENTS,
                        format!("unexpected {:?}", extra.value),
                        "write the body as body=<text>",
                    );
                }
                let body = match params.named.get("body") {
                    Some((value, arg)) => Some(self.expand(file, arg, value, false)?),
                    None => None,
                };
                let content_type = match params.named.get("type") {
                    Some((value, arg)) => Some(self.expand(file, arg, value, false)?),
                    None => None,
                };
                let retry_after_seconds = match params.named.get("retry_after") {
                    Some((value, arg)) => {
                        let ms = self.duration(file, arg, value)?;
                        if !ms.is_multiple_of(1_000) {
                            self.error(
                                file,
                                arg.span,
                                codes::TYPE,
                                "Retry-After counts whole seconds",
                            );
                            return None;
                        }
                        Some(u32::try_from(ms / 1_000).unwrap_or(u32::MAX))
                    }
                    None => None,
                };
                Some(ActionDraft::Ready(Action::Respond {
                    status,
                    body,
                    content_type,
                    retry_after_seconds,
                }))
            }
            _ => unreachable!("the schema has no other actions"),
        }
    }

    fn resolve(
        &mut self,
        draft: ActionDraft,
        upstreams: &BTreeMap<String, Uuid>,
    ) -> Option<Action> {
        match draft {
            ActionDraft::Ready(action) => Some(action),
            ActionDraft::Proxy {
                upstream,
                file,
                span,
            } => match upstreams.get(&upstream.to_lowercase()) {
                Some(id) => Some(Action::Proxy { upstream_id: *id }),
                None => {
                    self.error(
                        &file,
                        span,
                        codes::REFERENCE,
                        format!("no upstream is named {upstream:?}"),
                    );
                    None
                }
            },
        }
    }

    fn finish(mut self) -> Lowered {
        let upstream_ids: BTreeMap<String, Uuid> = self
            .upstreams
            .iter()
            .map(|(upstream, _)| (upstream.name.to_lowercase(), upstream.id))
            .collect();
        let site_ids: BTreeMap<String, Uuid> = self
            .servers
            .iter()
            .map(|draft| (draft.site.name.to_lowercase(), draft.site.id))
            .collect();

        let mut model = ConfigModel {
            tls_profiles: std::mem::take(&mut self.profiles)
                .into_iter()
                .map(|(profile, _)| profile)
                .collect(),
            upstreams: std::mem::take(&mut self.upstreams)
                .into_iter()
                .map(|(upstream, _)| upstream)
                .collect(),
            ..ConfigModel::default()
        };
        for mut draft in std::mem::take(&mut self.listeners) {
            if let Some((name, file, span)) = draft.default_server.take() {
                match site_ids.get(&name.to_lowercase()) {
                    Some(id) => draft.listener.default_site_id = Some(*id),
                    None => self.error(
                        &file,
                        span,
                        codes::REFERENCE,
                        format!("no server is named {name:?}"),
                    ),
                }
            }
            model.listeners.push(draft.listener);
        }
        for draft in std::mem::take(&mut self.servers) {
            let mut site = draft.site;
            if let Some(action) = draft
                .action
                .and_then(|action| self.resolve(action, &upstream_ids))
            {
                site.action = action;
            }
            for (index, route_draft) in draft.routes.into_iter().enumerate() {
                let mut route = route_draft.route;
                let priority_set = route_draft.priority_set;
                if let Some(action) = route_draft
                    .action
                    .and_then(|action| self.resolve(action, &upstream_ids))
                {
                    route.action = action;
                }
                if !priority_set {
                    route.priority = u32::try_from((index + 1) * 10).unwrap_or(u32::MAX);
                }
                site.routes.push(route);
            }
            model.sites.push(site);
        }
        self.carry_over(&mut model);

        let diagnostics = validate(&model);
        for diagnostic in diagnostics {
            let origin = diagnostic
                .resource_id
                .as_deref()
                .and_then(|resource| self.origin_of(resource))
                .cloned();
            match origin {
                Some(origin) => self.report(diagnostic, &origin.file, origin.span),
                None => self.diagnostics.push(diagnostic),
            }
        }
        crate::checks::routes(&model, &mut |site, route, diagnostic| {
            let resource = format!("sites/{site}/routes/{route}");
            match self.origins.get(&resource).cloned() {
                Some(origin) => self.report(diagnostic, &origin.file, origin.span),
                None => self.diagnostics.push(diagnostic),
            }
        });
        Lowered {
            model,
            diagnostics: self.diagnostics,
            origins: self.origins,
            insertions: self.insertions,
        }
    }

    /// The origin of `resource` or of the closest resource containing it.
    fn origin_of(&self, resource: &str) -> Option<&Origin> {
        let mut candidate = resource;
        loop {
            if let Some(origin) = self.origins.get(candidate) {
                return Some(origin);
            }
            candidate = &candidate[..candidate.rfind('/')?];
        }
    }

    /// Carries metadata the language does not express over from the model the
    /// configuration replaces, and keeps its deleted sites.
    fn carry_over(&self, model: &mut ConfigModel) {
        let Some(previous) = self.options.previous else {
            return;
        };
        for site in &mut model.sites {
            if let Some(old) = previous
                .sites
                .iter()
                .find(|old| old.id == site.id && !old.is_deleted())
            {
                site.created_at = old.created_at;
                site.favorite = old.favorite;
                let mut unchanged = old.clone();
                unchanged.updated_at = site.updated_at;
                if unchanged == *site {
                    site.updated_at = old.updated_at;
                }
            }
        }
        for upstream in &mut model.upstreams {
            if let Some(old) = previous.upstreams.iter().find(|old| old.id == upstream.id) {
                upstream.created_at = old.created_at;
                let mut unchanged = old.clone();
                unchanged.updated_at = upstream.updated_at;
                if unchanged == *upstream {
                    upstream.updated_at = old.updated_at;
                }
            }
        }
        let live: BTreeSet<Uuid> = model.sites.iter().map(|site| site.id).collect();
        model.sites.extend(
            previous
                .sites
                .iter()
                .filter(|site| site.is_deleted() && !live.contains(&site.id))
                .cloned(),
        );
    }
}

fn placeholder_action() -> Action {
    Action::Respond {
        status: 503,
        body: None,
        content_type: None,
        retry_after_seconds: None,
    }
}

/// `host:port`, with IPv6 hosts in brackets.
fn split_host_port(address: &str) -> Option<(String, u16)> {
    let (host, port) = if let Some(rest) = address.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        (host, port)
    } else {
        let (host, port) = address.rsplit_once(':')?;
        if host.contains(':') {
            return None;
        }
        (host, port)
    };
    if host.is_empty() || port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((
        host.to_owned(),
        port.parse().ok().filter(|port| *port != 0)?,
    ))
}
