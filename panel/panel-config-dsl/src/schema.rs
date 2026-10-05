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
    SecurityPolicy,
    HttpPolicy,
    /// Inside an HTTP policy's `cors`.
    Cors,
    Listener,
    Upstream,
    Server,
    Route,
    /// Inside `any`, `all` and `not`.
    Conditions,
}

impl Context {
    pub fn name(self) -> &'static str {
        match self {
            Self::Main => "the top level",
            Self::Http => "http",
            Self::TlsProfile => "tls_profile",
            Self::SecurityPolicy => "security_policy",
            Self::HttpPolicy => "http_policy",
            Self::Cors => "cors",
            Self::Listener => "listener",
            Self::Upstream => "upstream",
            Self::Server => "server",
            Self::Route => "route",
            Self::Conditions => "any, all or not",
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
    /// Where the value comes from when the directive is not written here.
    pub inheritance: Option<&'static str>,
}

use Context::*;

const ACTION_CONTEXTS: &[Context] = &[Server, Route];
const CONDITION_CONTEXTS: &[Context] = &[Route, Conditions];

macro_rules! spec {
    ($name:literal in $contexts:expr, $block:expr, $min:literal..$max:expr, $repeat:literal,
     $syntax:literal, $summary:literal $(, inherits $rule:literal)?) => {
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
            inheritance: spec!(@rule $($rule)?),
        }
    };
    (@rule) => { None };
    (@rule $rule:literal) => { Some($rule) };
}

pub static DIRECTIVES: &[DirectiveSpec] = &[
    spec!("language_version" in &[Main], None, 1..Some(1), false,
        "language_version 1;", "The language version the file is written in; must come first."),
    spec!("include" in &[Main, Http, Server, Upstream], None, 1..Some(1), true,
        "include sites/*.conf;", "Reads directives from other files of the configuration."),
    spec!("set" in &[Http, Server, Route], None, 2..Some(2), true,
        "set $name value;", "Defines a constant for this block and the blocks inside it.",
        inherits "Visible in this block and the blocks inside it; a set in an inner block replaces it there."),
    spec!("http" in &[Main], Some(Http), 0..Some(0), false,
        "http { ... }", "Holds the listeners, TLS profiles, upstreams and servers."),
    spec!("access_log" in &[Http, Server, Route], None, 1..Some(2), false,
        "access_log [on|off] [format=json|combined];", "Whether requests are logged, and in which format.",
        inherits "On, in JSON, when not written; a route's setting wins over its server's, and a server's over the one in http."),
    spec!("log_field" in &[Http, Server, Route], None, 2..Some(2), true,
        "log_field <name> <template>;", "Adds a field to JSON access records, such as log_field tenant $http_x_tenant.",
        inherits "Fields add up from http to the server and the route; an inner field replaces one of the same name."),
    spec!("log_redact_query" in &[Http], None, 1..None, false,
        "log_redact_query <key> ...|off;", "Query parameters whose values are logged as REDACTED.",
        inherits "X-Amz-Signature, X-Amz-Credential, X-Amz-Security-Token, sig and X-Goog-Signature when not written; off redacts none."),
    spec!("log_redact_headers" in &[Http], None, 1..None, true,
        "log_redact_headers <name> ...;", "Headers logged as REDACTED besides authorization, proxy-authorization, cookie and set-cookie."),
    spec!("log_files" in &[Http], None, 1..Some(4), false,
        "log_files [max_size=100m] [rotate=daily|size] [keep=7d] [max_files=30];", "How log files rotate and how long rotated files are kept.",
        inherits "max_size=100m rotate=daily keep=7d max_files=30 when not written; keep=0 and max_files=0 keep every rotated file."),
    spec!("tls_profile" in &[Http], Some(TlsProfile), 1..Some(1), true,
        "tls_profile <id> { ... }", "A certificate and the TLS settings it is served with."),
    spec!("certificate_id" in &[TlsProfile], None, 1..Some(1), false,
        "certificate_id <id>;", "A certificate of the inventory, which the panel delivers to the gateway."),
    spec!("certificate" in &[TlsProfile], None, 1..Some(1), false,
        "certificate <file>;", "File name of a PEM certificate chain placed in the gateway's secret directory."),
    spec!("key" in &[TlsProfile], None, 1..Some(1), false,
        "key <file>;", "File name of the PEM private key placed next to it."),
    spec!("min_protocol" in &[TlsProfile], None, 1..Some(1), false,
        "min_protocol TLSv1.2|TLSv1.3;", "The oldest TLS version accepted.",
        inherits "TLSv1.2 when not written."),
    spec!("max_protocol" in &[TlsProfile], None, 1..Some(1), false,
        "max_protocol TLSv1.2|TLSv1.3;", "The newest TLS version accepted.",
        inherits "The newest supported version when not written; applies to listeners using the profile."),
    spec!("ciphers" in &[TlsProfile], None, 1..None, false,
        "ciphers TLS13_AES_256_GCM_SHA384 ...;", "IANA names of the cipher suites accepted.",
        inherits "Every supported AEAD suite with forward secrecy when not written; applies to listeners using the profile."),
    spec!("session_resumption" in &[TlsProfile], None, 1..Some(1), false,
        "session_resumption on|off;", "Whether clients resume sessions by session IDs and tickets.",
        inherits "On when not written; applies to listeners using the profile."),
    spec!("ocsp_stapling" in &[TlsProfile], None, 1..Some(1), false,
        "ocsp_stapling on|off;", "Reserved: recorded, but no OCSP responses are stapled yet.",
        inherits "Off when not written."),
    spec!("alpn" in &[TlsProfile], None, 1..None, false,
        "alpn h2 http/1.1;", "Protocols a listener using the profile offers; all it enables by default.",
        inherits "Without it, a listener offers every protocol it serves."),
    spec!("security_policy" in &[Http], Some(SecurityPolicy), 1..Some(1), true,
        "security_policy <id> { ... }", "Restrictions requests must pass before servers and routes using it act on them."),
    spec!("http_policy" in &[Http], Some(HttpPolicy), 1..Some(1), true,
        "http_policy <id> { ... }", "Field changes, the Server field, CORS and compression for servers and routes using it."),
    spec!("request_header" in &[HttpPolicy], None, 2..None, true,
        "request_header remove <name> ...|set <name> <value>|add <name> <value>;",
        "Changes request fields before requests go upstream; values are templates such as $host.",
        inherits "Removals apply first, then replacements, then additions; a route's policy applies after its server's."),
    spec!("response_header" in &[HttpPolicy], None, 2..None, true,
        "response_header remove <name> ...|set <name> <value>|add <name> <value>;",
        "Changes response fields, of proxied and generated responses alike, before they go to clients.",
        inherits "Removals apply first, then replacements, then additions; a route's policy applies after its server's."),
    spec!("server_header" in &[HttpPolicy], None, 1..Some(2), false,
        "server_header keep|remove|replace <value>;", "What becomes of the Server field of responses.",
        inherits "Kept when not written."),
    spec!("cors" in &[HttpPolicy], Some(Cors), 0..Some(0), false,
        "cors { ... }", "Allows cross-origin requests from the origins listed; the gateway answers their preflights.",
        inherits "A route's cors replaces its server's."),
    spec!("origins" in &[Cors], None, 1..None, true,
        "origins <origin> ...;", "Origins such as https://shop.example, https://*.shop.example, or *."),
    spec!("methods" in &[Cors], None, 1..None, true,
        "methods <method> ...;", "Methods a preflight allows besides GET, HEAD and POST."),
    spec!("headers" in &[Cors], None, 1..None, true,
        "headers <name> ...;", "Request fields a preflight allows."),
    spec!("expose" in &[Cors], None, 1..None, true,
        "expose <name> ...;", "Response fields scripts may read."),
    spec!("credentials" in &[Cors], None, 1..Some(1), false,
        "credentials on|off;", "Allows requests with cookies or authorization; the origin is echoed, never *.",
        inherits "Off when not written."),
    spec!("max_age" in &[Cors], None, 1..Some(1), false,
        "max_age <duration>;", "How long browsers may cache a preflight's answer, at most a day.",
        inherits "Browsers decide when not written."),
    spec!("compress" in &[HttpPolicy], None, 2..Some(5), false,
        "compress gzip|br|zstd ... types=<type>,... [min_size=<size>];",
        "Compresses responses of the listed media types with a coding the client accepts.",
        inherits "A route's compress replaces its server's; encoded, partial and no-transform responses are sent as they are."),
    spec!("allow" in &[SecurityPolicy], None, 1..None, true,
        "allow <network> ...;", "Client networks allowed; once any is written, other clients get 403."),
    spec!("deny" in &[SecurityPolicy], None, 1..None, true,
        "deny <network> ...;", "Client networks refused with 403, even when allowed."),
    spec!("methods" in &[SecurityPolicy], None, 1..None, false,
        "methods GET POST ...;", "Methods allowed; others get 405 with Allow. HEAD goes with GET.",
        inherits "Every method when not written."),
    spec!("deny_paths" in &[SecurityPolicy], None, 1..None, true,
        "deny_paths /admin /.git ...;", "Path prefixes refused with 403."),
    spec!("deny_user_agents" in &[SecurityPolicy], None, 1..None, true,
        "deny_user_agents <regex> ...;", "User agents refused with 403 when a case-insensitive regular expression matches."),
    spec!("referers" in &[SecurityPolicy], None, 1..None, false,
        "referers [none] <host> *.<domain> ...;", "Pages allowed to link here, others get 403; none allows requests without a referer."),
    spec!("basic_auth" in &[SecurityPolicy], None, 1..Some(2), false,
        "basic_auth <htpasswd file> [realm=<text>];", "Asks for a user of an htpasswd file in the gateway's secret directory; bcrypt and Argon2 hashes only."),
    spec!("max_header_size" in &[SecurityPolicy], None, 1..Some(1), false,
        "max_header_size 16k;", "The most bytes of request headers; larger requests get 431."),
    spec!("max_body_size" in &[SecurityPolicy], None, 1..Some(1), false,
        "max_body_size 10m;", "The largest request body; larger ones get 413."),
    spec!("body_timeout" in &[SecurityPolicy], None, 1..Some(1), false,
        "body_timeout 30s;", "The longest wait for the next part of a request body, in whole seconds; slower clients get 408."),
    spec!("rate_limit" in &[SecurityPolicy], None, 1..Some(3), true,
        "rate_limit 10r/s [burst=20] [key=$client_ip|$host|$route|$http_<name>];", "Requests allowed per key and period; more get 429."),
    spec!("max_concurrent" in &[SecurityPolicy], None, 1..Some(1), false,
        "max_concurrent 20;", "Requests one client address may have in progress; more get 429."),
    spec!("limited_response" in &[SecurityPolicy], None, 1..Some(3), false,
        "limited_response <status> [body=<text>] [type=<content type>];", "The answer to requests over a rate or concurrency limit.",
        inherits "429 with Retry-After when not written."),
    spec!("listener" in &[Http], Some(Listener), 1..Some(1), true,
        "listener <id> { ... }", "A socket the gateway accepts connections on."),
    spec!("address" in &[Listener], None, 1..Some(1), false,
        "address 0.0.0.0:80;", "The IP address and port."),
    spec!("protocols" in &[Listener], None, 1..Some(3), false,
        "protocols http1 http2;", "The HTTP versions served; http3 is reserved.",
        inherits "HTTP/1.1 and HTTP/2 when not written."),
    spec!("tls_profile" in &[Listener, Server], None, 1..Some(1), false,
        "tls_profile <id>;", "Serves HTTPS with this profile's certificate by default.",
        inherits "A host uses its domain's tls_profile=, else its server's tls_profile, else that of the listener the connection arrives on; a listener without one serves plain HTTP."),
    spec!("reuse_port" in &[Listener], None, 1..Some(1), false,
        "reuse_port on|off;", "Sets SO_REUSEPORT on the socket.",
        inherits "Off when not written."),
    spec!("ipv6_only" in &[Listener], None, 1..Some(1), false,
        "ipv6_only on|off;", "For IPv6 addresses, whether IPv4 connections are refused.",
        inherits "The operating system's setting when not written."),
    spec!("trusted_proxies" in &[Listener], None, 1..None, true,
        "trusted_proxies <network> ...;", "Proxies whose forwarding headers name the client; other peers' forwarding headers are dropped."),
    spec!("real_ip_header" in &[Listener], None, 1..Some(1), false,
        "real_ip_header x-forwarded-for|x-real-ip|forwarded;", "The header trusted proxies name the client in.",
        inherits "x-forwarded-for when not written."),
    spec!("request_head_timeout" in &[Listener], None, 1..Some(1), false,
        "request_head_timeout 30s;", "The longest a client may take to send a request head; a late first head gets 408.",
        inherits "30s when not written, counted from the connection's start for its first request and from the end of the previous one for later requests."),
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
        "How requests are spread over the nodes.",
        inherits "round_robin when not written."),
    spec!("host_header" in &[Upstream], None, 1..Some(1), false,
        "host_header <host>;", "Replaces the client's Host when forwarding.",
        inherits "The client's Host is kept when not written; health checks without host= send this one."),
    spec!("tls" in &[Upstream], None, 1..None, false,
        "tls [verify=on|off] [verify_hostname=on|off] [sni=<name>] [ca=<file>];",
        "How TLS nodes are verified.",
        inherits "Nodes with the tls flag send their own sni=, else this sni=, else their host name; certificates and host names are verified unless turned off."),
    spec!("connect_timeout" in &[Upstream], None, 1..Some(1), false,
        "connect_timeout 5s;", "How long connecting to a node may take."),
    spec!("read_timeout" in &[Upstream], None, 1..Some(1), false,
        "read_timeout 30s;", "How long a node may stay silent while responding."),
    spec!("write_timeout" in &[Upstream], None, 1..Some(1), false,
        "write_timeout 30s;", "How long sending to a node may stall."),
    spec!("idle_timeout" in &[Upstream], None, 1..Some(1), false,
        "idle_timeout 60s;", "How long an idle pooled connection is kept."),
    spec!("keepalive" in &[Upstream], None, 1..Some(1), false,
        "keepalive on|off;", "Whether connections to nodes are reused.",
        inherits "On when not written."),
    spec!("max_connections" in &[Upstream], None, 1..Some(1), false,
        "max_connections 100;", "Concurrent requests per node; busy nodes are skipped.",
        inherits "Unlimited when not written."),
    spec!("http2" in &[Upstream], None, 1..Some(1), false,
        "http2 on|off;", "Speaks HTTP/2 to TLS nodes that negotiate it.",
        inherits "Off when not written; only nodes with the tls flag negotiate it."),
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
        "listen <listener> ...;", "The listeners serving the server; all of them by default.",
        inherits "Without it, the server is served on every listener; its routes are served wherever it is."),
    spec!("https_redirect" in &[Server], None, 1..Some(1), false,
        "https_redirect on|off;", "Redirects plain HTTP requests to HTTPS.",
        inherits "Off when not written; applies to every host and route of the server."),
    spec!("hsts" in &[Server], None, 1..Some(3), false,
        "hsts max_age=365d [include_subdomains] [preload];", "Sends Strict-Transport-Security with HTTPS responses.",
        inherits "Not sent when not written; applies to every host of the server."),
    spec!("security_policy" in &[Server, Route], None, 1..Some(1), false,
        "security_policy <id>;", "Requests pass the policy before the action.",
        inherits "A route's requests pass the server's policy first, then the route's own."),
    spec!("http_policy" in &[Server, Route], None, 1..Some(1), false,
        "http_policy <id>;", "Requests and responses go through the HTTP policy.",
        inherits "A route's requests go through the server's policy first, then the route's own."),
    spec!("www_redirect" in &[Server], None, 1..Some(1), false,
        "www_redirect off|add|remove;", "Adds or removes the www label with a redirect.",
        inherits "Off when not written; applies to every host and route of the server."),
    spec!("enabled" in &[Server, Route], None, 1..Some(1), false,
        "enabled on|off;", "Whether it serves traffic.",
        inherits "On when not written; a route serves only while its server is enabled too."),
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
    spec!("method" in CONDITION_CONTEXTS, None, 1..None, true,
        "method <method> ...;", "Takes requests with one of the methods, compared case-sensitively."),
    spec!("host" in CONDITION_CONTEXTS, None, 1..None, true,
        "host <host> ...;", "Takes requests for one of the hosts, or for *.parent wildcards of one label."),
    spec!("header" in CONDITION_CONTEXTS, None, 2..Some(4), true,
        "header <name> present|absent|<op> <value> [ignore_case];",
        "Takes requests whose header, by its case-insensitive name, passes a test: =, ^=, $=, *=, ~ or ~*."),
    spec!("query" in CONDITION_CONTEXTS, None, 2..Some(4), true,
        "query <name> present|absent|<op> <value> [ignore_case];",
        "Takes requests whose query parameter passes a test; a repeated parameter passes when any value does."),
    spec!("cookie" in CONDITION_CONTEXTS, None, 2..Some(4), true,
        "cookie <name> present|absent|<op> <value> [ignore_case];",
        "Takes requests whose cookie passes a test."),
    spec!("client" in CONDITION_CONTEXTS, None, 1..None, true,
        "client <address or network> ...;", "Takes requests from clients, after trusted proxies, in one of the networks."),
    spec!("user_agent" in CONDITION_CONTEXTS, None, 1..Some(3), true,
        "user_agent present|absent|<op> <value> [ignore_case];", "Takes requests whose User-Agent passes a test."),
    spec!("referer" in CONDITION_CONTEXTS, None, 1..Some(3), true,
        "referer present|absent|<op> <value> [ignore_case];", "Takes requests whose Referer passes a test."),
    spec!("content_type" in CONDITION_CONTEXTS, None, 1..None, true,
        "content_type <type> ...;", "Takes requests of one of the media types, such as application/json or text/*."),
    spec!("any" in CONDITION_CONTEXTS, Some(Conditions), 0..Some(0), true,
        "any { ... }", "Takes requests that meet at least one of the conditions inside."),
    spec!("all" in CONDITION_CONTEXTS, Some(Conditions), 0..Some(0), true,
        "all { ... }", "Takes requests that meet every condition inside."),
    spec!("not" in CONDITION_CONTEXTS, Some(Conditions), 0..Some(0), true,
        "not { ... }", "Takes requests that do not meet the conditions inside."),
    spec!("priority" in &[Route], None, 1..Some(1), false,
        "priority <n>;", "Lower priorities are evaluated first.",
        inherits "Without it, routes take 10, 20, 30 and so on in the order they are written."),
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
