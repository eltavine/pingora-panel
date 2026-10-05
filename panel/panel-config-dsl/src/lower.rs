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
    validate, Action, ConfigModel, HttpPolicy, Listener, LuaConfig, Route, SecurityPolicy, Site,
    TlsProfile, Upstream,
};
use panel_domain::NormalizedHost;
use panel_dsl::{Argument, Body, Directive, Document, LineIndex, Span};
use panel_errors::{Diagnostic, DiagnosticSeverity};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use uuid::Uuid;

mod conditions;
mod http;
mod listener;
mod logging;
mod lua;
mod resilience;
mod route;
mod security;
mod server;
mod tls;
mod upstream;

pub(crate) use http::CODINGS;
pub(crate) use logging::{print_access, print_policy};
pub(crate) use lua::{
    print_terms as print_lua_terms, runs as lua_runs, DEFAULTS as LUA_DEFAULTS,
    GROUPS as LUA_GROUPS, TERMS as LUA_TERMS,
};
pub(crate) use resilience::{print_breaker, print_queue, print_retry};
pub(crate) use security::{print_rate, DEFAULT_REALM};

/// How references in a value are resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Expansion {
    /// Plain text: request variables are refused.
    Text,
    /// A value the gateway reads per request, such as a hash key.
    Request,
    /// A template the gateway fills in per request.
    Template,
}

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

/// A directive written in a block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Written {
    pub directive: &'static str,
    pub file: String,
    /// From the directive's name through its `;`.
    pub span: Span,
}

/// A constant visible in a block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Constant {
    pub name: String,
    pub value: String,
    pub file: String,
    /// The `set` directive.
    pub span: Span,
    /// The resource path of the block that sets it, or `http`.
    pub block: String,
}

#[derive(Clone, Debug)]
pub struct Lowered {
    pub model: ConfigModel,
    pub diagnostics: Vec<Diagnostic>,
    /// Where each resource is written, by its resource path such as
    /// `sites/<id>`, `sites/<id>/routes/<id>`, `sites/<id>/domains/<host>`
    /// or `upstreams/<id>/nodes/<id>`.
    pub origins: BTreeMap<String, Origin>,
    /// Identifiers assigned to blocks and nodes written without one.
    pub insertions: Vec<Insertion>,
    /// The directives of each block other than blocks, `include` and `set`,
    /// by resource path, in the order they are read.
    pub written: BTreeMap<String, Vec<Written>>,
    /// The constants visible at the end of each block, by resource path.
    pub constants: BTreeMap<String, Vec<Constant>>,
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

struct Defined {
    value: String,
    file: String,
    span: Span,
    used: bool,
}

/// A block being read and the constants it sets.
struct Scope {
    /// The block's resource path, `http`, or empty at the top level.
    block: String,
    constants: BTreeMap<String, Defined>,
}

struct Lowerer<'a> {
    sources: &'a Sources,
    options: &'a LowerOptions<'a>,
    documents: BTreeMap<String, Rc<Document>>,
    indexes: BTreeMap<String, LineIndex>,
    diagnostics: Vec<Diagnostic>,
    includes: Vec<String>,
    scopes: Vec<Scope>,
    profiles: Vec<(TlsProfile, Origin)>,
    policies: Vec<(SecurityPolicy, Origin)>,
    http_policies: Vec<(HttpPolicy, Origin)>,
    listeners: Vec<ListenerDraft>,
    upstreams: Vec<(Upstream, Origin)>,
    servers: Vec<ServerDraft>,
    /// The logging directives of `http`.
    logging: panel_ir::LoggingPolicy,
    /// The Lua directives of `http` and the configuration's Lua files.
    lua: LuaConfig,
    origins: BTreeMap<String, Origin>,
    insertions: Vec<Insertion>,
    written: BTreeMap<String, Vec<Written>>,
    constants: BTreeMap<String, Vec<Constant>>,
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
            scopes: vec![Scope {
                block: String::new(),
                constants: BTreeMap::new(),
            }],
            profiles: Vec::new(),
            policies: Vec::new(),
            http_policies: Vec::new(),
            listeners: Vec::new(),
            upstreams: Vec::new(),
            servers: Vec::new(),
            logging: panel_ir::LoggingPolicy::default(),
            lua: LuaConfig::default(),
            origins: BTreeMap::new(),
            insertions: Vec::new(),
            written: BTreeMap::new(),
            constants: BTreeMap::new(),
        };
        for (path, text) in sources.files() {
            lowerer
                .indexes
                .insert(path.to_owned(), LineIndex::new(text));
            if source::is_lua(path) {
                continue;
            }
            let parsed = panel_dsl::parse(path, text);
            lowerer.diagnostics.extend(parsed.diagnostics);
            lowerer
                .documents
                .insert(path.to_owned(), Rc::new(parsed.document));
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

    /// Reports `diagnostic` on the whole `line` of `file`, or on the file.
    fn report_line(&mut self, diagnostic: Diagnostic, file: &str, line: Option<u32>) {
        let text = self.sources.get(file).unwrap_or_default();
        let span = line.and_then(|line| {
            let start = self.indexes.get(file)?.offset(text, line as usize, 1)?;
            let end = text[start..]
                .find('\n')
                .map_or(text.len(), |end| start + end);
            Some(Span::new(start, end))
        });
        match span {
            Some(span) => self.report(diagnostic, file, span),
            None => {
                let mut diagnostic = diagnostic;
                if self.sources.get(file).is_some() {
                    diagnostic.source_span = Some(file.to_owned());
                }
                self.diagnostics.push(diagnostic);
            }
        }
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
                _ => {
                    let block = &self.scopes.last().expect("a scope").block;
                    if spec.block.is_none() && !block.is_empty() {
                        self.written
                            .entry(block.clone())
                            .or_default()
                            .push(Written {
                                directive: spec.name,
                                file: file.to_owned(),
                                span: directive.span,
                            });
                    }
                    handle(self, file, directive, spec, depth)
                }
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
            if let Some(reason) = schema::refusal(name) {
                diagnostic = Diagnostic::error(
                    codes::UNKNOWN_DIRECTIVE,
                    format!("'{name}' is not available"),
                )
                .with_help(reason);
            } else if let Some(suggestion) = schema::suggestion(name, context) {
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
            if source::is_lua(&path) {
                if !source::is_glob(&pattern.value) {
                    self.error_with_help(
                        file,
                        pattern.span,
                        codes::INCLUDE,
                        format!("{path} is Lua code, not configuration"),
                        "run it with a *_by_lua_file directive",
                    );
                }
                continue;
            }
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
        let Some(value) = self.expand(
            file,
            &directive.args[1],
            &directive.args[1].value,
            Expansion::Text,
        ) else {
            return;
        };
        let replaced = self.scopes.last_mut().expect("a scope").constants.insert(
            name.to_owned(),
            Defined {
                value,
                file: file.to_owned(),
                span: directive.span,
                used: false,
            },
        );
        if let Some(unused) = replaced.filter(|replaced| !replaced.used) {
            let diagnostic = Diagnostic::warning(
                codes::NO_EFFECT,
                format!("this value of ${name} is never used: it is set again before any use"),
            )
            .with_help("remove this set");
            self.report(diagnostic, &unused.file, unused.span);
        }
    }

    /// Every constant visible in the innermost block; inner ones replace
    /// outer ones of the same name.
    fn visible(&self) -> Vec<Constant> {
        let mut visible = BTreeMap::new();
        for scope in &self.scopes {
            for (name, defined) in &scope.constants {
                visible.insert(
                    name.clone(),
                    Constant {
                        name: name.clone(),
                        value: defined.value.clone(),
                        file: defined.file.clone(),
                        span: defined.span,
                        block: scope.block.clone(),
                    },
                );
            }
        }
        visible.into_values().collect()
    }

    /// Resolves constants and environment references in `value`. Request
    /// variables stay as written in [`Expansion::Request`] and
    /// [`Expansion::Template`]; a template also keeps every other dollar
    /// escaped, so the result reads back as the same template.
    fn expand(
        &mut self,
        file: &str,
        arg: &Argument,
        value: &str,
        mode: Expansion,
    ) -> Option<String> {
        let pieces = match variables::pieces(value) {
            Ok(pieces) => pieces,
            Err(message) => {
                self.error(file, arg.span, codes::VARIABLE, message);
                return None;
            }
        };
        let literal = |text: &str| {
            if mode == Expansion::Template {
                text.replace('$', "$$")
            } else {
                text.to_owned()
            }
        };
        let mut out = String::with_capacity(value.len());
        // A variable name is braced when a name character follows it.
        let mut open_variable: Option<String> = None;
        let push = |out: &mut String, open: &mut Option<String>, text: &str| {
            if let Some(name) = open.take() {
                if text
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                {
                    out.push_str("${");
                    out.push_str(&name);
                    out.push('}');
                } else {
                    out.push('$');
                    out.push_str(&name);
                }
            }
            out.push_str(text);
        };
        for piece in pieces {
            match piece {
                Piece::Text(text) => push(&mut out, &mut open_variable, &literal(text)),
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
                        Some(found) => push(&mut out, &mut open_variable, &literal(found)),
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
                    if let Some(found) = self
                        .scopes
                        .iter_mut()
                        .rev()
                        .find_map(|scope| scope.constants.get_mut(name))
                    {
                        found.used = true;
                        let found = literal(&found.value);
                        push(&mut out, &mut open_variable, &found);
                    } else if variables::is_request_variable(name) {
                        if mode == Expansion::Text {
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
                        push(&mut out, &mut open_variable, "");
                        open_variable = Some(name.to_owned());
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
        push(&mut out, &mut open_variable, "");
        Some(out)
    }

    /// The expanded value of a whole argument.
    fn value(&mut self, file: &str, arg: &Argument) -> Option<String> {
        self.expand(file, arg, &arg.value.clone(), Expansion::Text)
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

    /// Reads a block, `block` being its resource path, in a scope of its own.
    fn with_scope(&mut self, block: String, run: impl FnOnce(&mut Self)) {
        self.scopes.push(Scope {
            block,
            constants: BTreeMap::new(),
        });
        run(self);
        let visible = self.visible();
        let scope = self.scopes.pop().expect("a scope");
        for (name, defined) in scope.constants.into_iter().filter(|(_, d)| !d.used) {
            let diagnostic =
                Diagnostic::warning(codes::NO_EFFECT, format!("${name} is never used"))
                    .with_help(format!("remove the set, or refer to it as ${name}"));
            self.report(diagnostic, &defined.file, defined.span);
        }
        if !visible.is_empty() {
            self.constants.insert(scope.block, visible);
        }
    }

    fn http(&mut self, file: &str, directive: &Directive, depth: usize) {
        let Some(block) = directive.block() else {
            return;
        };
        self.with_scope("http".into(), |lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::Http,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, depth| match spec.name {
                    "tls_profile" => lowerer.tls_profile(file, directive, depth),
                    "security_policy" => lowerer.security_policy(file, directive, depth),
                    "http_policy" => lowerer.http_policy(file, directive, depth),
                    "listener" => lowerer.listener(file, directive, depth),
                    "upstream" => lowerer.upstream(file, directive, depth),
                    "server" => lowerer.server(file, directive, depth),
                    "access_log" | "log_field" | "log_redact_query" | "log_redact_headers"
                    | "log_files" => lowerer.logging(file, directive),
                    name if lua::HTTP.contains(&name) => lowerer.lua_http(file, directive, depth),
                    name if lua::SCOPE.contains(&name) => {
                        lowerer
                            .origins
                            .entry("lua".into())
                            .or_insert_with(|| Self::origin(file, directive, depth));
                        let mut scope = std::mem::take(&mut lowerer.lua.http);
                        lowerer.lua_scope(file, directive, &mut scope, "http");
                        lowerer.lua.http = scope;
                    }
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
        self.lua_files();
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
            security_policies: std::mem::take(&mut self.policies)
                .into_iter()
                .map(|(policy, _)| policy)
                .collect(),
            http_policies: std::mem::take(&mut self.http_policies)
                .into_iter()
                .map(|(policy, _)| policy)
                .collect(),
            upstreams: std::mem::take(&mut self.upstreams)
                .into_iter()
                .map(|(upstream, _)| upstream)
                .collect(),
            logging: std::mem::take(&mut self.logging),
            lua: std::mem::take(&mut self.lua),
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
        crate::scripts::check(&model, &mut |file, line, diagnostic| {
            self.report_line(diagnostic, file, line);
        });
        crate::checks::routes(&model, &mut |site, route, diagnostic| {
            let resource = format!("sites/{site}/routes/{route}");
            match self.origins.get(&resource).cloned() {
                Some(origin) => self.report(diagnostic, &origin.file, origin.span),
                None => self.diagnostics.push(diagnostic),
            }
        });
        crate::checks::exposure(&model, &mut |resource, diagnostic| match self
            .origin_of(&resource)
            .cloned()
        {
            Some(origin) => self.report(diagnostic, &origin.file, origin.span),
            None => self.diagnostics.push(diagnostic),
        });
        for (file, span, diagnostic) in
            crate::inheritance::check(&model, &self.written, &self.origins)
        {
            self.report(diagnostic, &file, span);
        }
        Lowered {
            model,
            diagnostics: self.diagnostics,
            origins: self.origins,
            insertions: self.insertions,
            written: self.written,
            constants: self.constants,
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
