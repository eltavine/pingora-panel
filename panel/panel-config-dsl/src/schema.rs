//! Every directive of the language: where it may appear, its arguments,
//! whether it opens a block and whether it is deprecated. The same table
//! drives checking and editor completion.

use serde::Serialize;

/// The block a directive appears in.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Context {
    Main,
    Http,
    TlsProfile,
    Listener,
    Upstream,
    Server,
    Route,
}

impl Context {
    pub fn name(self) -> &'static str {
        match self {
            Self::Main => "the top level",
            Self::Http => "http",
            Self::TlsProfile => "tls_profile",
            Self::Listener => "listener",
            Self::Upstream => "upstream",
            Self::Server => "server",
            Self::Route => "route",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Deprecation {
    pub replacement: &'static str,
    /// The language version that no longer accepts the directive.
    pub removed_in: u32,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DirectiveSpec {
    pub name: &'static str,
    pub contexts: &'static [Context],
    /// The context its block opens, for block directives.
    pub block: Option<Context>,
    pub min_args: usize,
    /// `None` when any number of arguments is accepted.
    pub max_args: Option<usize>,
    /// How the arguments are written, for completion.
    pub syntax: &'static str,
    pub summary: &'static str,
    /// Whether the directive may appear more than once in one block.
    pub repeatable: bool,
    pub deprecated: Option<Deprecation>,
}

use Context::*;

const ACTION_CONTEXTS: &[Context] = &[Server, Route];

macro_rules! spec {
    ($name:literal in $contexts:expr, $block:expr, $min:literal..$max:expr, $repeat:literal,
     $syntax:literal, $summary:literal) => {
        DirectiveSpec {
            name: $name,
            contexts: $contexts,
            block: $block,
            min_args: $min,
            max_args: $max,
            syntax: $syntax,
            summary: $summary,
            repeatable: $repeat,
            deprecated: None,
        }
    };
}

pub static DIRECTIVES: &[DirectiveSpec] = &[
    spec!("language_version" in &[Main], None, 1..Some(1), false,
        "language_version 1;", "The language version the file is written in; must come first."),
    spec!("include" in &[Main, Http, Server, Upstream], None, 1..Some(1), true,
        "include sites/*.conf;", "Reads directives from other files of the configuration."),
    spec!("set" in &[Http, Server, Route], None, 2..Some(2), true,
        "set $name value;", "Defines a constant for this block and the blocks inside it."),
    spec!("http" in &[Main], Some(Http), 0..Some(0), false,
        "http { ... }", "Holds the listeners, TLS profiles, upstreams and servers."),
    spec!("tls_profile" in &[Http], Some(TlsProfile), 1..Some(1), true,
        "tls_profile <id> { ... }", "A certificate and key from the gateway's secret directory."),
    spec!("certificate" in &[TlsProfile], None, 1..Some(1), false,
        "certificate <file>;", "File name of the PEM certificate chain."),
    spec!("key" in &[TlsProfile], None, 1..Some(1), false,
        "key <file>;", "File name of the PEM private key."),
    spec!("min_protocol" in &[TlsProfile], None, 1..Some(1), false,
        "min_protocol TLSv1.2|TLSv1.3;", "The oldest TLS version accepted."),
    spec!("alpn" in &[TlsProfile], None, 1..None, false,
        "alpn h2 http/1.1;", "Protocols a listener using the profile offers; all it enables by default."),
    spec!("listener" in &[Http], Some(Listener), 1..Some(1), true,
        "listener <id> { ... }", "A socket the gateway accepts connections on."),
    spec!("address" in &[Listener], None, 1..Some(1), false,
        "address 0.0.0.0:80;", "The IP address and port."),
    spec!("protocols" in &[Listener], None, 1..Some(3), false,
        "protocols http1 http2;", "The HTTP versions served; http3 is reserved."),
    spec!("tls_profile" in &[Listener, Server], None, 1..Some(1), false,
        "tls_profile <id>;", "Serves HTTPS with this profile's certificate by default."),
    spec!("reuse_port" in &[Listener], None, 1..Some(1), false,
        "reuse_port on|off;", "Sets SO_REUSEPORT on the socket."),
    spec!("ipv6_only" in &[Listener], None, 1..Some(1), false,
        "ipv6_only on|off;", "For IPv6 addresses, whether IPv4 connections are refused."),
    spec!("default_server" in &[Listener], None, 1..Some(1), false,
        "default_server <server>;", "Serves hosts no server claims; they get 421 otherwise."),
    spec!("upstream" in &[Http], Some(Upstream), 1..Some(1), true,
        "upstream <name> { ... }", "A pool of backend nodes."),
    spec!("id" in &[Upstream, Server, Route], None, 1..Some(1), false,
        "id <uuid>;", "The stable identifier; added when missing."),
    spec!("server" in &[Upstream], None, 1..None, true,
        "server <host:port> [weight=N] [backup] [down] [tls] [sni=<name>] [id=<uuid>] [note=<text>];",
        "A backend node."),
    spec!("balance" in &[Upstream], None, 1..Some(2), false,
        "balance round_robin|random|hash [key=$client_ip|$uri|$http_<name>|$cookie_<name>];",
        "How requests are spread over the nodes."),
    spec!("host_header" in &[Upstream], None, 1..Some(1), false,
        "host_header <host>;", "Replaces the client's Host when forwarding."),
    spec!("tls" in &[Upstream], None, 1..None, false,
        "tls [verify=on|off] [verify_hostname=on|off] [sni=<name>] [ca=<file>];",
        "How TLS nodes are verified."),
    spec!("connect_timeout" in &[Upstream], None, 1..Some(1), false,
        "connect_timeout 5s;", "How long connecting to a node may take."),
    spec!("read_timeout" in &[Upstream], None, 1..Some(1), false,
        "read_timeout 30s;", "How long a node may stay silent while responding."),
    spec!("write_timeout" in &[Upstream], None, 1..Some(1), false,
        "write_timeout 30s;", "How long sending to a node may stall."),
    spec!("idle_timeout" in &[Upstream], None, 1..Some(1), false,
        "idle_timeout 60s;", "How long an idle pooled connection is kept."),
    spec!("keepalive" in &[Upstream], None, 1..Some(1), false,
        "keepalive on|off;", "Whether connections to nodes are reused."),
    spec!("max_connections" in &[Upstream], None, 1..Some(1), false,
        "max_connections 100;", "Concurrent requests per node; busy nodes are skipped."),
    spec!("http2" in &[Upstream], None, 1..Some(1), false,
        "http2 on|off;", "Speaks HTTP/2 to TLS nodes that negotiate it."),
    spec!("health_check" in &[Upstream], None, 1..None, false,
        "health_check http|tcp [path=/] [method=GET|HEAD] [host=<name>] [interval=5s] [timeout=1s] [rise=2] [fall=3] [status=200,204];",
        "Probes every node and takes failing ones out of rotation."),
    spec!("passive_health" in &[Upstream], None, 0..Some(2), false,
        "passive_health [fails=5] [eject=30s];", "Ejects a node after consecutive failed requests."),
    spec!("note" in &[Upstream, Server], None, 1..Some(1), false,
        "note <text>;", "A free-form remark."),
    spec!("server" in &[Http], Some(Server), 1..Some(1), true,
        "server <name> { ... }", "A website and the hosts it answers for."),
    spec!("server_name" in &[Server], None, 1..None, true,
        "server_name <host> ...;", "Hosts the server answers for; the first is primary."),
    spec!("alias" in &[Server], None, 1..None, true,
        "alias <host> ...;", "Hosts redirected to the primary host."),
    spec!("domain" in &[Server], None, 1..None, true,
        "domain <host> [primary] [alias] [off] [tls_profile=<id>];", "One host with all of its settings."),
    spec!("listen" in &[Server], None, 1..None, false,
        "listen <listener> ...;", "The listeners serving the server; all of them by default."),
    spec!("https_redirect" in &[Server], None, 1..Some(1), false,
        "https_redirect on|off;", "Redirects plain HTTP requests to HTTPS."),
    spec!("www_redirect" in &[Server], None, 1..Some(1), false,
        "www_redirect off|add|remove;", "Adds or removes the www label with a redirect."),
    spec!("enabled" in &[Server, Route], None, 1..Some(1), false,
        "enabled on|off;", "Whether it serves traffic."),
    spec!("group" in &[Server], None, 1..Some(1), false,
        "group <name>;", "A group to file the server under."),
    spec!("tags" in &[Server], None, 1..None, false,
        "tags <tag> ...;", "Labels for finding the server."),
    spec!("proxy" in ACTION_CONTEXTS, None, 1..Some(1), false,
        "proxy <upstream>;", "Forwards requests to an upstream."),
    spec!("root" in ACTION_CONTEXTS, None, 1..Some(3), false,
        "root <dir> [index=index.html,...] [spa=on|off];", "Serves files from a directory below the gateway's static root."),
    spec!("return" in ACTION_CONTEXTS, None, 2..Some(3), false,
        "return 301|302|303|307|308 <url> [preserve_path=on|off];", "Redirects requests."),
    spec!("respond" in ACTION_CONTEXTS, None, 1..Some(4), false,
        "respond <status> [body=<text>] [type=<content type>] [retry_after=<duration>];",
        "Answers with a fixed response."),
    spec!("route" in &[Server], Some(Route), 0..Some(1), true,
        "route [<name>] { ... }", "Sends matching requests to their own action."),
    spec!("match" in &[Route], None, 2..Some(3), false,
        "match exact|prefix|glob|regex <path> [host=<host>];", "The requests the route takes."),
    spec!("priority" in &[Route], None, 1..Some(1), false,
        "priority <n>;", "Lower priorities are evaluated first."),
    DirectiveSpec {
        deprecated: Some(Deprecation {
            replacement: "route",
            removed_in: 2,
        }),
        ..spec!("location" in &[Server], Some(Route), 1..Some(2), true,
            "location [=|^~|~|~*] <path> { ... }", "Accepted from NGINX and formatted as a route.")
    },
    DirectiveSpec {
        deprecated: Some(Deprecation {
            replacement: "proxy",
            removed_in: 2,
        }),
        ..spec!("proxy_pass" in ACTION_CONTEXTS, None, 1..Some(1), false,
            "proxy_pass http://<upstream>;", "Accepted from NGINX and formatted as proxy.")
    },
];

/// The directive named `name` in `context`, if any.
pub fn lookup(name: &str, context: Context) -> Option<&'static DirectiveSpec> {
    DIRECTIVES
        .iter()
        .find(|spec| spec.name == name && spec.contexts.contains(&context))
}

/// Every context `name` may appear in, to explain a misplaced directive.
pub fn contexts_of(name: &str) -> Vec<Context> {
    DIRECTIVES
        .iter()
        .filter(|spec| spec.name == name)
        .flat_map(|spec| spec.contexts.iter().copied())
        .collect()
}

/// The known directive closest to a misspelled one.
pub fn suggestion(name: &str, context: Context) -> Option<&'static str> {
    DIRECTIVES
        .iter()
        .filter(|spec| spec.contexts.contains(&context))
        .map(|spec| (distance(name, spec.name), spec.name))
        .filter(|(distance, candidate)| *distance <= candidate.len().div_ceil(3))
        .min()
        .map(|(_, candidate)| candidate)
}

/// Levenshtein distance over characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous + usize::from(ca != *cb);
            previous = row[j + 1];
            row[j + 1] = substitution.min(row[j] + 1).min(previous + 1);
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_by_context() {
        assert_eq!(lookup("server", Http).unwrap().block, Some(Server));
        assert_eq!(lookup("server", Upstream).unwrap().block, None);
        assert!(lookup("proxy", Http).is_none());
        assert_eq!(contexts_of("proxy"), vec![Server, Route]);
        assert_eq!(suggestion("server_nmae", Server), Some("server_name"));
        assert_eq!(suggestion("upsteam", Http), Some("upstream"));
        assert_eq!(suggestion("zzzzzz", Http), None);
    }

    #[test]
    fn each_name_appears_once_per_context() {
        for spec in DIRECTIVES {
            for context in spec.contexts {
                let matches = DIRECTIVES
                    .iter()
                    .filter(|other| other.name == spec.name && other.contexts.contains(context))
                    .count();
                assert_eq!(matches, 1, "{} in {context:?}", spec.name);
            }
        }
    }
}
