//! Converts a documented subset of NGINX configuration into the language.
//! `server`, `listen`, `server_name`, `location` with its modifiers,
//! `proxy_pass`, `root`, `index`, `try_files`, `return` and `upstream` are
//! carried over; every other directive is reported at its position rather
//! than dropped silently.

use crate::{Sources, LANGUAGE_VERSION};
use globset::GlobBuilder;
use panel_dsl::{Body, Directive, Document, LineIndex};
use panel_errors::Diagnostic;
use std::collections::{BTreeMap, BTreeSet};

/// Report codes of the import.
pub mod codes {
    /// A directive that is not carried over.
    pub const UNSUPPORTED: &str = "NGINX_UNSUPPORTED";
    /// A directive carried over with a different meaning; check the result.
    pub const CHANGED: &str = "NGINX_CHANGED";
}

/// Directives the gateway sets up itself, so they have no counterpart.
const PROCESS: &[&str] = &[
    "daemon",
    "env",
    "error_log",
    "events",
    "load_module",
    "lock_file",
    "master_process",
    "pcre_jit",
    "pid",
    "thread_pool",
    "user",
    "worker_cpu_affinity",
    "worker_priority",
    "worker_processes",
    "worker_rlimit_core",
    "worker_rlimit_nofile",
    "worker_shutdown_timeout",
    "working_directory",
];

const REDIRECTS: &[&str] = &["301", "302", "303", "307", "308"];

/// The converted files and what did not carry over.
#[derive(Clone, Debug)]
pub struct NginxImport {
    pub sources: Sources,
    /// Directives not carried over, or carried over with another meaning,
    /// at their position in the NGINX files.
    pub report: Vec<Diagnostic>,
}

/// A directive with the file it was written in.
#[derive(Clone)]
struct Located {
    file: String,
    directive: Directive,
}

#[derive(Default)]
struct Converted {
    listeners: BTreeMap<String, (String, Vec<Directive>)>,
    upstreams: Vec<Directive>,
    upstream_names: BTreeSet<String>,
    servers: Vec<Directive>,
    server_names: BTreeSet<String>,
}

struct Importer<'a> {
    files: &'a BTreeMap<String, String>,
    indexes: BTreeMap<&'a str, LineIndex>,
    entry_directory: String,
    report: Vec<Diagnostic>,
    out: Converted,
}

/// A route converted from a `location`, ordered as NGINX would match it.
struct Route {
    rank: (u8, i64),
    directives: Vec<Directive>,
}

/// Converts `entry`, following its includes among `files`.
pub fn import_nginx(files: &BTreeMap<String, String>, entry: &str) -> Result<NginxImport, String> {
    let Some(text) = files.get(entry) else {
        return Err(format!("there is no file {entry:?}"));
    };
    let mut importer = Importer {
        files,
        indexes: files
            .iter()
            .map(|(path, text)| (path.as_str(), LineIndex::new(text)))
            .collect(),
        entry_directory: entry
            .rsplit_once('/')
            .map_or(String::new(), |(directory, _)| format!("{directory}/")),
        report: Vec::new(),
        out: Converted::default(),
    };
    let top = importer.parse(entry, text, &mut Vec::new());
    for located in top {
        match located.directive.name.value.as_str() {
            "http" => {
                let inner = importer.block(&located);
                importer.http(inner);
            }
            name if PROCESS.contains(&name) => importer.unsupported(
                &located,
                format!("'{name}' is not carried over: the gateway manages its own processes"),
            ),
            name => importer.unsupported(&located, format!("'{name}' is not supported")),
        }
    }
    if importer.out.listeners.is_empty() && !importer.out.servers.is_empty() {
        importer
            .out
            .listeners
            .insert("http-80".into(), ("0.0.0.0:80".into(), Vec::new()));
        importer.report.push(Diagnostic::warning(
            codes::CHANGED,
            "no server has a supported 'listen'; the servers listen on 0.0.0.0:80",
        ));
    }
    let Converted {
        listeners,
        upstreams,
        servers,
        ..
    } = importer.out;
    let mut http: Vec<Directive> = listeners
        .into_iter()
        .map(|(id, (address, extra))| {
            let mut children = vec![Directive::simple("address", [address])];
            children.extend(extra);
            Directive::with_block("listener", [id], children)
        })
        .collect();
    http.extend(upstreams);
    http.extend(servers);
    let document = Document {
        directives: vec![
            Directive::simple("language_version", [LANGUAGE_VERSION.to_string()]),
            Directive::with_block("http", Vec::<String>::new(), http),
        ],
        trailing: Vec::new(),
    };
    Ok(NginxImport {
        sources: Sources::single(panel_dsl::format(&document)),
        report: importer.report,
    })
}

fn children(directive: &Directive) -> &[Directive] {
    match &directive.body {
        Body::Block(block) => &block.directives,
        _ => &[],
    }
}

fn args(directive: &Directive) -> Vec<&str> {
    directive
        .args
        .iter()
        .map(|arg| arg.value.as_str())
        .collect()
}

/// A name the language accepts for a server, upstream or listener.
fn identifier(value: &str) -> String {
    let name: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches('-').to_owned();
    if name.is_empty() {
        "imported".to_owned()
    } else {
        name
    }
}

impl<'a> Importer<'a> {
    /// The directives of `text`, with includes replaced by the directives of
    /// the files they name.
    fn parse(&mut self, file: &str, text: &str, stack: &mut Vec<String>) -> Vec<Located> {
        let parsed = panel_dsl::parse(file, text);
        self.report.extend(parsed.diagnostics);
        stack.push(file.to_owned());
        let located = self.expand(file, &parsed.document.directives, stack);
        stack.pop();
        located
    }

    fn expand(
        &mut self,
        file: &str,
        directives: &[Directive],
        stack: &mut Vec<String>,
    ) -> Vec<Located> {
        let mut out = Vec::new();
        for directive in directives {
            if directive.name.value != "include" {
                out.push(Located {
                    file: file.to_owned(),
                    directive: directive.clone(),
                });
                continue;
            }
            let located = Located {
                file: file.to_owned(),
                directive: directive.clone(),
            };
            for pattern in args(directive) {
                let matched = self.resolve(pattern);
                if matched.is_empty() && !pattern.contains(['*', '?', '[']) {
                    self.unsupported(
                        &located,
                        format!("the included file {pattern:?} was not provided"),
                    );
                }
                for path in matched {
                    if stack.contains(&path) {
                        self.unsupported(&located, format!("{path:?} includes itself"));
                        continue;
                    }
                    let text = self.files[&path].clone();
                    let parsed = panel_dsl::parse(&path, &text);
                    self.report.extend(parsed.diagnostics);
                    stack.push(path.clone());
                    out.extend(self.expand(&path, &parsed.document.directives, stack));
                    stack.pop();
                }
            }
        }
        out
    }

    /// Provided files an include pattern names: as written, or relative to
    /// the directory of the entry file, where NGINX resolves relative paths.
    fn resolve(&self, pattern: &str) -> Vec<String> {
        let candidates = [
            pattern.to_owned(),
            format!("{}{pattern}", self.entry_directory),
            pattern
                .strip_prefix(&self.entry_directory)
                .unwrap_or(pattern)
                .to_owned(),
        ];
        let mut found = BTreeSet::new();
        for candidate in candidates {
            let Ok(glob) = GlobBuilder::new(&candidate).literal_separator(true).build() else {
                continue;
            };
            let matcher = glob.compile_matcher();
            found.extend(
                self.files
                    .keys()
                    .filter(|path| matcher.is_match(path.as_str()))
                    .cloned(),
            );
        }
        found.into_iter().collect()
    }

    fn block(&mut self, located: &Located) -> Vec<Located> {
        let file = located.file.clone();
        self.expand(&file, children(&located.directive), &mut vec![file.clone()])
    }

    fn span(&self, located: &Located) -> Option<String> {
        let text = self.files.get(&located.file)?;
        let index = self.indexes.get(located.file.as_str())?;
        Some(index.describe(&located.file, text, located.directive.span))
    }

    fn note(&mut self, located: &Located, code: &str, message: String) {
        let mut diagnostic = Diagnostic::warning(code, message);
        diagnostic.source_span = self.span(located);
        self.report.push(diagnostic);
    }

    fn unsupported(&mut self, located: &Located, message: String) {
        self.note(located, codes::UNSUPPORTED, message);
    }

    fn changed(&mut self, located: &Located, message: String) {
        self.note(located, codes::CHANGED, message);
    }

    fn http(&mut self, directives: Vec<Located>) {
        for located in directives {
            match located.directive.name.value.as_str() {
                "upstream" => self.upstream(&located),
                "server" => self.server(&located),
                name => self.unsupported(&located, format!("'{name}' is not supported")),
            }
        }
    }

    fn upstream(&mut self, located: &Located) {
        let Some(name) = located
            .directive
            .args
            .first()
            .map(|arg| identifier(&arg.value))
        else {
            self.unsupported(located, "an upstream needs a name".into());
            return;
        };
        let mut converted = Vec::new();
        let mut passive: Option<(String, String)> = None;
        for inner in self.block(located) {
            let values = args(&inner.directive);
            match inner.directive.name.value.as_str() {
                "server" => {
                    let Some((address, rest)) = values.split_first() else {
                        continue;
                    };
                    if address.starts_with("unix:") {
                        self.unsupported(&inner, "Unix socket upstreams are not supported".into());
                        continue;
                    }
                    let address = if address.rsplit_once(':').is_some_and(|(_, port)| {
                        port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty()
                    }) {
                        (*address).to_owned()
                    } else {
                        format!("{address}:80")
                    };
                    let mut node = vec![address];
                    for option in rest {
                        match option.split_once('=') {
                            Some(("weight", weight)) => node.push(format!("weight={weight}")),
                            Some(("max_fails", fails)) => {
                                passive.get_or_insert_with(|| ("1".into(), "10s".into())).0 =
                                    fails.to_string();
                            }
                            Some(("fail_timeout", timeout)) => {
                                passive.get_or_insert_with(|| ("1".into(), "10s".into())).1 =
                                    timeout.to_string();
                            }
                            None if *option == "backup" || *option == "down" => {
                                node.push((*option).to_owned())
                            }
                            _ => self.unsupported(
                                &inner,
                                format!("the node option {option:?} is not supported"),
                            ),
                        }
                    }
                    converted.push(Directive::simple("server", node));
                }
                "ip_hash" => {
                    converted.push(Directive::simple("balance", ["hash", "key=$client_ip"]))
                }
                "random" => converted.push(Directive::simple("balance", ["random"])),
                "hash" => match values.first().and_then(|key| hash_key(key)) {
                    Some(key) => converted.push(Directive::simple(
                        "balance",
                        ["hash".to_owned(), format!("key={key}")],
                    )),
                    None => self.unsupported(
                        &inner,
                        format!(
                            "hashing by {:?} is not supported",
                            values.first().unwrap_or(&"")
                        ),
                    ),
                },
                "keepalive" => {
                    converted.push(Directive::simple("keepalive", ["on"]));
                    self.changed(
                        &inner,
                        "'keepalive' turns connection reuse on; its pool size is not carried over"
                            .into(),
                    );
                }
                name => {
                    self.unsupported(&inner, format!("'{name}' is not supported in an upstream"))
                }
            }
        }
        if let Some((fails, eject)) = passive {
            converted.push(Directive::simple(
                "passive_health",
                [format!("fails={fails}"), format!("eject={eject}")],
            ));
        }
        self.out.upstream_names.insert(name.clone());
        self.out
            .upstreams
            .push(Directive::with_block("upstream", [name], converted));
    }

    fn listener(&mut self, located: &Located, values: &[&str], server: &str) -> Option<String> {
        let (address, options) = values.split_first()?;
        if options.contains(&"ssl") || options.contains(&"quic") {
            self.unsupported(
                located,
                "TLS listeners are not carried over; add a TLS profile and an HTTPS listener"
                    .into(),
            );
            return None;
        }
        let address = if address.chars().all(|c| c.is_ascii_digit()) {
            format!("0.0.0.0:{address}")
        } else if let Some(port) = address.strip_prefix("*:") {
            format!("0.0.0.0:{port}")
        } else {
            (*address).to_owned()
        };
        let Ok(socket) = address.parse::<std::net::SocketAddr>() else {
            self.unsupported(
                located,
                format!("the listen address {address:?} is not an IP address and port"),
            );
            return None;
        };
        let id = match socket.ip() {
            ip if ip.is_unspecified() && socket.is_ipv4() => format!("http-{}", socket.port()),
            ip if ip.is_unspecified() => format!("http-v6-{}", socket.port()),
            ip => identifier(&format!("http-{ip}-{}", socket.port())),
        };
        // NGINX binds wildcard IPv6 sockets IPv6-only unless told otherwise.
        let ipv6_only =
            socket.is_ipv6() && socket.ip().is_unspecified() && !options.contains(&"ipv6only=off");
        let entry = self.out.listeners.entry(id.clone()).or_insert_with(|| {
            let extra = if ipv6_only {
                vec![Directive::simple("ipv6_only", ["on"])]
            } else {
                Vec::new()
            };
            (socket.to_string(), extra)
        });
        let mut unsupported = Vec::new();
        for option in options {
            match *option {
                "default_server" | "default" => {
                    if !entry
                        .1
                        .iter()
                        .any(|directive| directive.name.value == "default_server")
                    {
                        entry.1.push(Directive::simple("default_server", [server]));
                    }
                }
                "reuseport" => {
                    if !entry
                        .1
                        .iter()
                        .any(|directive| directive.name.value == "reuse_port")
                    {
                        entry.1.push(Directive::simple("reuse_port", ["on"]));
                    }
                }
                "http2" | "ipv6only=on" | "ipv6only=off" => {}
                other => unsupported.push(other.to_owned()),
            }
        }
        for option in unsupported {
            self.unsupported(
                located,
                format!("the listen option {option:?} is not supported"),
            );
        }
        Some(id)
    }

    fn server(&mut self, located: &Located) {
        let directives = self.block(located);
        let hosts: Vec<String> = directives
            .iter()
            .filter(|inner| inner.directive.name.value == "server_name")
            .flat_map(|inner| {
                args(&inner.directive)
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        let base = hosts
            .iter()
            .find(|host| *host != "_" && !host.is_empty())
            .map_or_else(
                || "default".to_owned(),
                |host| identifier(host.trim_start_matches("*.")),
            );
        let mut name = base.clone();
        let mut counter = 2;
        while !self.out.server_names.insert(name.clone()) {
            name = format!("{base}-{counter}");
            counter += 1;
        }

        let mut body = Vec::new();
        let mut listens = Vec::new();
        let mut listened = false;
        let mut action: Option<Vec<Directive>> = None;
        let mut routes = Vec::new();
        // Locations inherit the server's files wherever they are written.
        let mut static_files: Option<(String, Vec<String>, bool)> = None;
        for inner in directives.iter().filter(|inner| {
            matches!(
                inner.directive.name.value.as_str(),
                "root" | "index" | "try_files"
            )
        }) {
            self.static_files(inner, &mut static_files);
        }
        for inner in &directives {
            let values = args(&inner.directive);
            match inner.directive.name.value.as_str() {
                "server_name" => {
                    let mut names = Vec::new();
                    for host in values {
                        if host == "_" || host.is_empty() {
                            continue;
                        }
                        if host.starts_with('~') {
                            self.unsupported(
                                inner,
                                format!("the regular expression name {host:?} is not supported"),
                            );
                        } else {
                            names.push(host.to_owned());
                        }
                    }
                    if !names.is_empty() {
                        body.push(Directive::simple("server_name", names));
                    }
                }
                "listen" => {
                    listened = true;
                    if let Some(id) = self.listener(inner, &values, &name) {
                        if !listens.contains(&id) {
                            listens.push(id);
                        }
                    }
                }
                "location" => {
                    if let Some(route) = self.location(inner, &mut action, static_files.as_ref()) {
                        routes.push(route);
                    }
                }
                "return" => match self.redirect(inner) {
                    Some(converted) if is_https_redirect(&converted) => body.extend(converted),
                    Some(converted) => action = Some(converted),
                    None => {}
                },
                "root" | "index" | "try_files" => {}
                name if name.starts_with("ssl_") => self.unsupported(
                    inner,
                    format!("'{name}' is not carried over; certificates are set in TLS profiles"),
                ),
                name => self.unsupported(inner, format!("'{name}' is not supported in a server")),
            }
        }
        if listened && listens.is_empty() {
            self.unsupported(
                located,
                format!("the server {name:?} only listens with TLS and is not carried over"),
            );
            self.out.server_names.remove(&name);
            return;
        }
        if !listens.is_empty() {
            body.push(Directive::simple("listen", listens));
        }
        if let Some((root, index, spa)) = static_files {
            if action.is_none() {
                action = Some(vec![static_root(&root, &index, spa)]);
            }
        }
        body.extend(action.unwrap_or_default());
        routes.sort_by_key(|route| route.rank);
        for (position, route) in routes.into_iter().enumerate() {
            let mut children = route.directives;
            children.insert(
                1,
                Directive::simple("priority", [((position + 1) * 10).to_string()]),
            );
            body.push(Directive::with_block(
                "route",
                Vec::<String>::new(),
                children,
            ));
        }
        self.out
            .servers
            .push(Directive::with_block("server", [name], body));
    }

    fn static_files(&mut self, located: &Located, state: &mut Option<(String, Vec<String>, bool)>) {
        let values = args(&located.directive);
        let entry = state.get_or_insert_with(|| (String::new(), Vec::new(), false));
        match located.directive.name.value.as_str() {
            "root" => {
                let Some(path) = values.first() else { return };
                let relative = path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default();
                entry.0 = if relative.is_empty() {
                    "html".to_owned()
                } else {
                    relative.to_owned()
                };
                self.changed(
                    located,
                    format!(
                        "static files are served from the gateway's static directory: copy {path} to its {:?} directory",
                        entry.0
                    ),
                );
            }
            "index" => entry.1 = values.iter().map(|value| (*value).to_owned()).collect(),
            _ => match values.as_slice() {
                ["$uri", .., last] if last.ends_with(".html") && last.starts_with('/') => {
                    entry.2 = true
                }
                ["$uri", .., last] if last.starts_with('=') => {}
                _ => self.unsupported(
                    located,
                    format!("'try_files {}' is not supported", values.join(" ")),
                ),
            },
        }
    }

    /// A `return`, as a redirect, a response, or HTTPS redirection.
    fn redirect(&mut self, located: &Located) -> Option<Vec<Directive>> {
        let values = args(&located.directive);
        let (code, target) = match values.as_slice() {
            [url]
                if url.starts_with("http://")
                    || url.starts_with("https://")
                    || url.starts_with("$scheme") =>
            {
                ("302", Some(*url))
            }
            [code] => (*code, None),
            [code, target] => (*code, Some(*target)),
            _ => {
                self.unsupported(located, "this 'return' is not supported".into());
                return None;
            }
        };
        if REDIRECTS.contains(&code) {
            let url = target.unwrap_or_default();
            if url == "https://$host$request_uri" || url == "https://$server_name$request_uri" {
                return Some(vec![Directive::simple("https_redirect", ["on"])]);
            }
            if url.contains('$') {
                self.unsupported(
                    located,
                    format!(
                        "the redirect target {url:?} uses variables, which are not supported yet"
                    ),
                );
                return None;
            }
            return Some(vec![Directive::simple("return", [code, url])]);
        }
        if code == "444" {
            self.unsupported(
                located,
                "closing the connection without a response is not supported".into(),
            );
            return None;
        }
        if !code.chars().all(|c| c.is_ascii_digit()) {
            self.unsupported(located, format!("{code:?} is not a status code"));
            return None;
        }
        let mut respond = vec![code.to_owned()];
        if let Some(body) = target {
            if body.contains('$') {
                self.unsupported(
                    located,
                    "response bodies with variables are not supported yet".into(),
                );
                return None;
            }
            respond.push(format!("body={body}"));
        }
        Some(vec![Directive::simple("respond", respond)])
    }

    fn proxy(&mut self, located: &Located) -> Option<Directive> {
        let url = args(&located.directive)
            .first()
            .copied()
            .unwrap_or_default()
            .to_owned();
        let (tls, rest) = match url.split_once("://") {
            Some(("http", rest)) => (false, rest),
            Some(("https", rest)) => (true, rest),
            _ => {
                self.unsupported(located, format!("proxying to {url:?} is not supported"));
                return None;
            }
        };
        let (target, path) = rest
            .split_once('/')
            .map_or((rest, ""), |(target, path)| (target, path));
        if !path.is_empty() || rest.ends_with('/') || target.contains('$') {
            self.unsupported(
                located,
                format!(
                    "{url:?} rewrites the request path or uses variables, which is not supported"
                ),
            );
            return None;
        }
        let name = if self.out.upstream_names.contains(target) {
            target.to_owned()
        } else {
            let name = identifier(target);
            if self.out.upstream_names.insert(name.clone()) {
                let address = if target.contains(':') {
                    target.to_owned()
                } else {
                    format!("{target}:{}", if tls { 443 } else { 80 })
                };
                self.out.upstreams.push(Directive::with_block(
                    "upstream",
                    [name.clone()],
                    vec![Directive::simple("server", [address])],
                ));
            }
            name
        };
        if tls {
            if let Some(upstream) = self
                .out
                .upstreams
                .iter_mut()
                .find(|upstream| upstream.args.first().is_some_and(|arg| arg.value == name))
            {
                if let Body::Block(block) = &mut upstream.body {
                    if !block
                        .directives
                        .iter()
                        .any(|directive| directive.name.value == "tls")
                    {
                        block
                            .directives
                            .push(Directive::simple("tls", Vec::<String>::new()));
                    }
                }
            }
        }
        Some(Directive::simple("proxy", [name]))
    }

    /// A `location` as a route, or as the server's own action for `/`.
    fn location(
        &mut self,
        located: &Located,
        server_action: &mut Option<Vec<Directive>>,
        inherited: Option<&(String, Vec<String>, bool)>,
    ) -> Option<Route> {
        let values = args(&located.directive);
        let (kind, path, rank) = match values.as_slice() {
            ["=", path] => ("exact", (*path).to_owned(), (0, 0)),
            ["^~", path] => ("prefix", (*path).to_owned(), (1, -(path.len() as i64))),
            ["~", path] => ("regex", (*path).to_owned(), (2, 0)),
            ["~*", path] => ("regex", format!("(?i){path}"), (2, 0)),
            [path] if path.starts_with('@') => {
                self.unsupported(
                    located,
                    format!("the named location {path:?} is not supported"),
                );
                return None;
            }
            [path] => ("prefix", (*path).to_owned(), (3, -(path.len() as i64))),
            _ => {
                self.unsupported(located, "this 'location' is not supported".into());
                return None;
            }
        };
        let mut action = None;
        let mut static_files = inherited.cloned();
        // A location whose proxy or return cannot be carried over is left
        // out rather than serving files in its place.
        let mut directed = false;
        for inner in self.block(located) {
            match inner.directive.name.value.as_str() {
                "proxy_pass" => {
                    directed = true;
                    action = self.proxy(&inner).map(|directive| vec![directive]);
                }
                "return" => match self.redirect(&inner) {
                    Some(converted) if is_https_redirect(&converted) => self.unsupported(
                        &inner,
                        "redirecting a location to HTTPS is not supported; redirect the whole server"
                            .into(),
                    ),
                    converted => {
                        directed = true;
                        action = converted;
                    }
                },
                "root" | "index" | "try_files" => self.static_files(&inner, &mut static_files),
                "location" => self.unsupported(&inner, "nested locations are not supported".into()),
                "alias" => self.unsupported(&inner, "'alias' is not supported; use 'root'".into()),
                name => {
                    self.unsupported(&inner, format!("'{name}' is not supported in a location"))
                }
            }
        }
        if action.is_none() && !directed {
            if let Some((root, index, spa)) = static_files {
                action = Some(vec![static_root(&root, &index, spa)]);
            }
        }
        let Some(action) = action else {
            self.unsupported(
                located,
                "this location has no supported action and is not carried over".into(),
            );
            return None;
        };
        if kind == "prefix" && path == "/" && rank.0 == 3 && server_action.is_none() {
            *server_action = Some(action);
            return None;
        }
        let mut directives = vec![Directive::simple("match", [kind.to_owned(), path])];
        directives.extend(action);
        Some(Route { rank, directives })
    }
}

fn is_https_redirect(directives: &[Directive]) -> bool {
    matches!(directives, [directive] if directive.name.value == "https_redirect")
}

fn static_root(root: &str, index: &[String], spa: bool) -> Directive {
    let mut values = vec![if root.is_empty() {
        "html".to_owned()
    } else {
        root.to_owned()
    }];
    if !index.is_empty() {
        values.push(format!("index={}", index.join(",")));
    }
    if spa {
        values.push("spa=on".to_owned());
    }
    Directive::simple("root", values)
}

/// The language's hash key for an NGINX hash key.
fn hash_key(key: &str) -> Option<String> {
    match key {
        "$remote_addr" | "$binary_remote_addr" => Some("$client_ip".to_owned()),
        "$request_uri" | "$uri" => Some("$uri".to_owned()),
        key if key.starts_with("$http_") || key.starts_with("$cookie_") => Some(key.to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_safe_names() {
        assert_eq!(identifier("shop.example.com"), "shop-example-com");
        assert_eq!(identifier("..."), "imported");
        assert_eq!(hash_key("$remote_addr").as_deref(), Some("$client_ip"));
        assert_eq!(hash_key("$args"), None);
    }
}
