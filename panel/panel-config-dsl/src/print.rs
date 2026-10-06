//! The configuration model written in the language: the canonical text the
//! service stores and formats, with defaults left out.

use crate::{
    lower::{
        print_access, print_breaker, print_lua_terms, print_policy, print_queue, print_rate,
        print_retry, CODINGS, DEFAULT_REALM,
    },
    source::{Sources, ENTRY},
    values::{print_bool, print_duration_ms, print_size},
    variables::{escape, print_hash_key},
    LANGUAGE_VERSION,
};
use panel_config_model::{
    Action, ConfigModel, FieldChanges, HttpPolicy, Listener, LuaCode, LuaConfig, LuaScope, Route,
    RouteCondition, SecurityPolicy, Site, TlsProfile, Upstream, UpstreamNode, ValueTest,
};
use panel_dsl::{Directive, Document, Trivia};
use panel_ir::{
    HealthCheckProtocol, ListenerProtocols, LoadBalancingPolicy, RateLimitKey, RealIpHeader,
    ServerHeader, WwwRedirect,
};

/// The whole model as `main.conf`.
pub fn print(model: &ConfigModel) -> String {
    panel_dsl::format(&document(model))
}

/// The whole model as files: `main.conf` and the Lua files under `lua/`.
pub fn print_sources(model: &ConfigModel) -> Sources {
    let mut sources = Sources::single(print(model));
    for (path, text) in &model.lua.files {
        if path != ENTRY {
            sources.insert(path.clone(), text.clone());
        }
    }
    sources
}

pub fn document(model: &ConfigModel) -> Document {
    let mut http = print_policy(&model.logging);
    http.extend(lua_http(&model.lua));
    let settings = http.len();
    http.extend(model.tls_profiles.iter().map(tls_profile));
    http.extend(model.security_policies.iter().map(security_policy));
    http.extend(model.http_policies.iter().map(http_policy));
    http.extend(
        model
            .listeners
            .iter()
            .map(|listener| self::listener(listener, model)),
    );
    http.extend(model.upstreams.iter().map(upstream));
    http.extend(
        model
            .sites
            .iter()
            .filter(|site| !site.is_deleted())
            .map(|site| server(site, model)),
    );
    for (index, directive) in http.iter_mut().enumerate() {
        if index > 0 && index >= settings {
            directive.leading.push(Trivia::BlankLine);
        }
    }
    let mut block = Directive::with_block("http", Vec::<String>::new(), http);
    block.leading.push(Trivia::BlankLine);
    Document {
        directives: vec![
            Directive::simple("language_version", [LANGUAGE_VERSION.to_string()]),
            block,
        ],
        trailing: Vec::new(),
    }
}

fn expanded(value: &str) -> String {
    escape(value).into_owned()
}

/// The directive that runs `code` in `phase`.
fn lua_handler(phase: &str, code: &LuaCode) -> Directive {
    match code {
        LuaCode::Inline { code, .. } => {
            Directive::with_lua(&format!("{phase}_by_lua_block"), code.clone())
        }
        LuaCode::File { path } => {
            Directive::simple(&format!("{phase}_by_lua_file"), [expanded(path)])
        }
    }
}

/// The terms a level sets, then its handlers.
fn lua_scope(scope: &LuaScope) -> Vec<Directive> {
    let mut body: Vec<Directive> = print_lua_terms(scope)
        .into_iter()
        .map(|(name, args)| Directive::simple(name, args))
        .collect();
    body.extend(
        scope
            .handlers()
            .map(|(phase, code)| lua_handler(phase, code)),
    );
    body
}

/// The Lua directives of `http`.
pub(crate) fn lua_http(lua: &LuaConfig) -> Vec<Directive> {
    let mut body = Vec::new();
    if lua.disabled {
        body.push(Directive::simple("lua", ["off"]));
    }
    if let Some(bytes) = lua.memory_limit_bytes {
        body.push(Directive::simple("lua_memory_limit", [print_size(bytes)]));
    }
    if let Some(on) = lua.access_no_postpone {
        body.push(Directive::simple(
            "access_by_lua_no_postpone",
            [crate::values::print_bool(on)],
        ));
    }
    for (name, value) in [
        ("lua_max_pending_timers", lua.max_pending_timers),
        ("lua_max_running_timers", lua.max_running_timers),
        ("lua_regex_cache_max_entries", lua.regex_cache_max_entries),
        ("lua_regex_match_limit", lua.regex_match_limit),
    ] {
        if let Some(value) = value {
            body.push(Directive::simple(name, [value.to_string()]));
        }
    }
    for dict in &lua.shared_dicts {
        body.push(Directive::simple(
            "lua_shared_dict",
            [dict.name.clone(), print_size(dict.capacity_bytes)],
        ));
    }
    let mut scope = lua_scope(&lua.http);
    let handlers = scope.split_off(print_lua_terms(&lua.http).len());
    body.extend(scope);
    if let Some(code) = &lua.init {
        body.push(lua_handler("init", code));
    }
    if let Some(code) = &lua.init_worker {
        body.push(lua_handler("init_worker", code));
    }
    if let Some(code) = &lua.exit_worker {
        body.push(lua_handler("exit_worker", code));
    }
    body.extend(handlers);
    body
}

pub fn tls_profile(profile: &TlsProfile) -> Directive {
    let mut body = match &profile.certificate_id {
        Some(id) => vec![Directive::simple("certificate_id", [id.to_string()])],
        None => vec![
            Directive::simple("certificate", [expanded(&profile.certificate_secret_id)]),
            Directive::simple("key", [expanded(&profile.private_key_secret_id)]),
        ],
    };
    if profile.min_protocol != "TLSv1.2" {
        body.push(Directive::simple(
            "min_protocol",
            [profile.min_protocol.clone()],
        ));
    }
    if let Some(max) = &profile.max_protocol {
        body.push(Directive::simple("max_protocol", [max.clone()]));
    }
    if !profile.cipher_suites.is_empty() {
        body.push(Directive::simple(
            "ciphers",
            profile.cipher_suites.iter().cloned(),
        ));
    }
    if !profile.session_resumption {
        body.push(Directive::simple("session_resumption", ["off"]));
    }
    if profile.ocsp_stapling {
        body.push(Directive::simple("ocsp_stapling", ["on"]));
    }
    if !profile.alpn.is_empty() {
        body.push(Directive::simple("alpn", profile.alpn.iter().cloned()));
    }
    Directive::with_block("tls_profile", [profile.id.clone()], body)
}

pub fn http_policy(policy: &HttpPolicy) -> Directive {
    let mut body = Vec::new();
    let mut changes = |name: &str, changes: &FieldChanges| {
        if !changes.remove.is_empty() {
            body.push(Directive::simple(
                name,
                std::iter::once("remove".to_owned()).chain(changes.remove.iter().cloned()),
            ));
        }
        for (operation, fields) in [("set", &changes.set), ("add", &changes.add)] {
            for field in fields {
                body.push(Directive::simple(
                    name,
                    [
                        operation.to_owned(),
                        field.name.clone(),
                        field.value.clone(),
                    ],
                ));
            }
        }
    };
    changes("request_header", &policy.request);
    changes("response_header", &policy.response);
    match &policy.server {
        ServerHeader::Keep => {}
        ServerHeader::Remove => body.push(Directive::simple("server_header", ["remove"])),
        ServerHeader::Replace { value } => body.push(Directive::simple(
            "server_header",
            ["replace".to_owned(), expanded(value)],
        )),
    }
    if let Some(cors) = &policy.cors {
        let mut cors_body = Vec::new();
        for (name, values) in [
            ("origins", &cors.allowed_origins),
            ("methods", &cors.allowed_methods),
            ("headers", &cors.allowed_headers),
            ("expose", &cors.exposed_headers),
        ] {
            if !values.is_empty() {
                cors_body.push(Directive::simple(
                    name,
                    values.iter().map(|value| expanded(value)),
                ));
            }
        }
        if cors.allow_credentials {
            cors_body.push(Directive::simple("credentials", ["on"]));
        }
        if let Some(seconds) = cors.max_age_seconds {
            cors_body.push(Directive::simple(
                "max_age",
                [print_duration_ms(u64::from(seconds) * 1_000)],
            ));
        }
        body.push(Directive::with_block(
            "cors",
            Vec::<String>::new(),
            cors_body,
        ));
    }
    if let Some(compression) = &policy.compression {
        let mut args: Vec<String> = CODINGS
            .iter()
            .filter(|(_, algorithm)| compression.algorithms.contains(algorithm))
            .map(|(name, _)| (*name).to_owned())
            .collect();
        args.push(format!("types={}", expanded(&compression.types.join(","))));
        if compression.min_bytes > 0 {
            args.push(format!("min_size={}", print_size(compression.min_bytes)));
        }
        body.push(Directive::simple("compress", args));
    }
    Directive::with_block("http_policy", [policy.id.clone()], body)
}

pub fn security_policy(policy: &SecurityPolicy) -> Directive {
    let mut body = Vec::new();
    let mut list = |name: &str, values: Vec<String>| {
        if !values.is_empty() {
            body.push(Directive::simple(name, values));
        }
    };
    list("allow", policy.allowed_cidrs.clone());
    list("deny", policy.denied_cidrs.clone());
    list("methods", policy.allowed_methods.clone());
    list(
        "deny_paths",
        policy
            .denied_path_prefixes
            .iter()
            .map(|path| expanded(path))
            .collect(),
    );
    list("deny_user_agents", policy.denied_user_agents.clone());
    if let Some(rule) = &policy.referer {
        let mut hosts: Vec<String> = rule
            .allow_empty
            .then(|| "none".to_owned())
            .into_iter()
            .collect();
        hosts.extend(rule.allowed_hosts.iter().map(|host| expanded(host)));
        list("referers", hosts);
    }
    if let Some(auth) = &policy.basic_auth {
        let mut args = vec![expanded(&auth.users_secret_id)];
        if auth.realm != DEFAULT_REALM {
            args.push(format!("realm={}", expanded(&auth.realm)));
        }
        body.push(Directive::simple("basic_auth", args));
    }
    if let Some(bytes) = policy.max_header_bytes {
        body.push(Directive::simple("max_header_size", [print_size(bytes)]));
    }
    if let Some(bytes) = policy.max_body_bytes {
        body.push(Directive::simple("max_body_size", [print_size(bytes)]));
    }
    if let Some(seconds) = policy.body_timeout_seconds {
        body.push(Directive::simple(
            "body_timeout",
            [print_duration_ms(seconds.saturating_mul(1_000))],
        ));
    }
    for limit in &policy.rate_limits {
        let mut args = vec![print_rate(limit.requests, limit.per_seconds)];
        if limit.burst > 0 {
            args.push(format!("burst={}", limit.burst));
        }
        match &limit.key {
            RateLimitKey::ClientAddress => {}
            RateLimitKey::Host => args.push("key=$host".into()),
            RateLimitKey::Route => args.push("key=$route".into()),
            RateLimitKey::Header { name } => {
                args.push(format!("key={}", print_hash_key(&format!("header:{name}"))));
            }
            _ => {}
        }
        body.push(Directive::simple("rate_limit", args));
    }
    if let Some(max) = policy.max_concurrent_requests {
        body.push(Directive::simple("max_concurrent", [max.to_string()]));
    }
    if let Some(response) = &policy.limited_response {
        let mut args = vec![response.status.to_string()];
        if !response.body.is_empty() {
            args.push(format!("body={}", expanded(&response.body)));
        }
        if let Some(content_type) = &response.content_type {
            args.push(format!("type={}", expanded(content_type)));
        }
        body.push(Directive::simple("limited_response", args));
    }
    Directive::with_block("security_policy", [policy.id.clone()], body)
}

pub fn listener(listener: &Listener, model: &ConfigModel) -> Directive {
    let mut body = vec![Directive::simple("address", [expanded(&listener.address)])];
    if listener.protocols != ListenerProtocols::default() {
        let protocols = [
            (listener.protocols.http1, "http1"),
            (listener.protocols.http2, "http2"),
            (listener.protocols.http3, "http3"),
        ];
        body.push(Directive::simple(
            "protocols",
            protocols
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, name)| *name),
        ));
    }
    if let Some(profile) = &listener.tls_profile_id {
        body.push(Directive::simple("tls_profile", [profile.clone()]));
    }
    if listener.reuse_port {
        body.push(Directive::simple("reuse_port", ["on"]));
    }
    if let Some(ipv6_only) = listener.ipv6_only {
        body.push(Directive::simple("ipv6_only", [print_bool(ipv6_only)]));
    }
    if !listener.trusted_proxies.is_empty() {
        body.push(Directive::simple(
            "trusted_proxies",
            listener.trusted_proxies.iter().cloned(),
        ));
    }
    if let Some(seconds) = listener.request_head_timeout_seconds {
        body.push(Directive::simple(
            "request_head_timeout",
            [print_duration_ms(seconds.saturating_mul(1_000))],
        ));
    }
    match listener.real_ip_header {
        RealIpHeader::XForwardedFor => {}
        RealIpHeader::XRealIp => body.push(Directive::simple("real_ip_header", ["x-real-ip"])),
        RealIpHeader::Forwarded => body.push(Directive::simple("real_ip_header", ["forwarded"])),
        _ => {}
    }
    if let Some(site) = listener
        .default_site_id
        .and_then(|id| model.sites.iter().find(|site| site.id == id))
    {
        body.push(Directive::simple("default_server", [site.name.clone()]));
    }
    Directive::with_block("listener", [listener.id.clone()], body)
}

fn address(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn node(node: &UpstreamNode) -> Directive {
    let mut args = vec![expanded(&address(&node.host, node.port))];
    if node.weight != 1 {
        args.push(format!("weight={}", node.weight));
    }
    if node.backup {
        args.push("backup".into());
    }
    if !node.enabled {
        args.push("down".into());
    }
    if node.tls {
        args.push("tls".into());
    }
    if let Some(sni) = &node.sni {
        args.push(format!("sni={}", expanded(sni)));
    }
    if let Some(unix) = &node.unix_socket {
        args.push(format!("unix={}", expanded(unix)));
    }
    if let Some(note) = &node.note {
        args.push(format!("note={note}"));
    }
    args.push(format!("id={}", node.id));
    Directive::simple("server", args)
}

pub fn upstream(upstream: &Upstream) -> Directive {
    let mut body = vec![Directive::simple("id", [upstream.id.to_string()])];
    body.extend(upstream.nodes.iter().map(node));
    match &upstream.balancing {
        LoadBalancingPolicy::RoundRobin => {}
        LoadBalancingPolicy::Random => body.push(Directive::simple("balance", ["random"])),
        LoadBalancingPolicy::ConsistentHash { key } => body.push(Directive::simple(
            "balance",
            ["hash".to_owned(), format!("key={}", print_hash_key(key))],
        )),
    }
    if let Some(host) = &upstream.host_header {
        body.push(Directive::simple("host_header", [expanded(host)]));
    }
    let tls = &upstream.tls;
    let mut tls_args = Vec::new();
    if !tls.verify_certificate {
        tls_args.push("verify=off".to_owned());
    }
    if !tls.verify_hostname {
        tls_args.push("verify_hostname=off".to_owned());
    }
    if let Some(sni) = &tls.sni {
        tls_args.push(format!("sni={}", expanded(sni)));
    }
    if let Some(ca) = &tls.ca_secret_id {
        tls_args.push(format!("ca={}", expanded(ca)));
    }
    if !tls_args.is_empty() {
        body.push(Directive::simple("tls", tls_args));
    }
    let connection = &upstream.connection;
    for (name, value) in [
        ("connect_timeout", connection.connect_timeout_ms),
        ("read_timeout", connection.read_timeout_ms),
        ("write_timeout", connection.write_timeout_ms),
        ("idle_timeout", connection.idle_timeout_ms),
    ] {
        if let Some(ms) = value {
            body.push(Directive::simple(name, [print_duration_ms(ms)]));
        }
    }
    if !connection.keepalive {
        body.push(Directive::simple("keepalive", ["off"]));
    }
    if let Some(max) = connection.max_connections {
        body.push(Directive::simple("max_connections", [max.to_string()]));
    }
    if connection.http2 {
        body.push(Directive::simple("http2", ["on"]));
    }
    if connection.h2c {
        body.push(Directive::simple("h2c", ["on"]));
    }
    if let Some(check) = &upstream.health_check {
        let mut args = Vec::new();
        match check.protocol {
            HealthCheckProtocol::Http => {
                args.push("http".to_owned());
                args.push(format!("path={}", expanded(&check.path)));
                args.push(format!("method={}", check.method));
                if let Some(host) = &check.host {
                    args.push(format!("host={}", expanded(host)));
                }
            }
            HealthCheckProtocol::Tcp => args.push("tcp".to_owned()),
        }
        args.push(format!("interval={}", print_duration_ms(check.interval_ms)));
        args.push(format!("timeout={}", print_duration_ms(check.timeout_ms)));
        args.push(format!("rise={}", check.healthy_threshold));
        args.push(format!("fall={}", check.unhealthy_threshold));
        if !check.expected_statuses.is_empty() {
            let statuses: Vec<String> =
                check.expected_statuses.iter().map(u16::to_string).collect();
            args.push(format!("status={}", statuses.join(",")));
        }
        body.push(Directive::simple("health_check", args));
    }
    if let Some(passive) = &upstream.passive_health {
        body.push(Directive::simple(
            "passive_health",
            [
                format!("fails={}", passive.failure_threshold),
                format!("eject={}", print_duration_ms(passive.ejection_ms)),
            ],
        ));
    }
    if let Some(retry) = &upstream.retry {
        body.push(Directive::simple("retry", print_retry(retry)));
    }
    if let Some(breaker) = &upstream.circuit_breaker {
        body.push(Directive::simple("circuit_breaker", print_breaker(breaker)));
    }
    if let Some(max) = upstream.max_requests {
        body.push(Directive::simple("max_requests", [max.to_string()]));
    }
    if let Some(queue) = &upstream.queue {
        body.push(Directive::simple("queue", print_queue(queue)));
    }
    if let Some(code) = &upstream.balancer {
        body.push(lua_handler("balancer", code));
    }
    if let Some(note) = &upstream.note {
        body.push(Directive::simple("note", [note.clone()]));
    }
    Directive::with_block("upstream", [upstream.name.clone()], body)
}

fn action(action: &Action, model: &ConfigModel) -> Directive {
    match action {
        Action::Proxy { upstream_id } => {
            let name = model
                .upstreams
                .iter()
                .find(|upstream| upstream.id == *upstream_id)
                .map_or_else(|| upstream_id.to_string(), |upstream| upstream.name.clone());
            Directive::simple("proxy", [name])
        }
        Action::Static {
            root,
            index_files,
            spa_fallback,
        } => {
            let mut args = vec![expanded(root)];
            if index_files.as_slice() != ["index.html"] {
                args.push(format!("index={}", expanded(&index_files.join(","))));
            }
            if *spa_fallback {
                args.push("spa=on".into());
            }
            Directive::simple("root", args)
        }
        Action::Redirect {
            location,
            status,
            preserve_path,
        } => {
            let mut args = vec![status.to_string(), location.clone()];
            if !preserve_path {
                args.push("preserve_path=off".into());
            }
            Directive::simple("return", args)
        }
        Action::Respond {
            status,
            body,
            content_type,
            retry_after_seconds,
        } => {
            let mut args = vec![status.to_string()];
            if let Some(body) = body {
                args.push(format!("body={body}"));
            }
            if let Some(content_type) = content_type {
                args.push(format!("type={}", expanded(content_type)));
            }
            if let Some(seconds) = retry_after_seconds {
                args.push(format!(
                    "retry_after={}",
                    print_duration_ms(u64::from(*seconds) * 1_000)
                ));
            }
            Directive::simple("respond", args)
        }
        Action::Lua { code } => lua_handler("content", code),
        _ => Directive::simple("respond", ["503"]),
    }
}

/// A route condition as the directive that writes it.
fn condition(condition: &RouteCondition) -> Directive {
    let test = |test: &ValueTest| -> Vec<String> {
        let compared = |operator: &str, value: &str, ignore_case: bool| {
            let mut args = vec![operator.to_owned(), expanded(value)];
            if ignore_case {
                args.push("ignore_case".to_owned());
            }
            args
        };
        match test {
            ValueTest::Present => vec!["present".to_owned()],
            ValueTest::Absent => vec!["absent".to_owned()],
            ValueTest::Equals { value, ignore_case } => compared("=", value, *ignore_case),
            ValueTest::Prefix { value, ignore_case } => compared("^=", value, *ignore_case),
            ValueTest::Suffix { value, ignore_case } => compared("$=", value, *ignore_case),
            ValueTest::Contains { value, ignore_case } => compared("*=", value, *ignore_case),
            ValueTest::Regex {
                pattern,
                ignore_case,
            } => vec![
                if *ignore_case { "~*" } else { "~" }.to_owned(),
                pattern.clone(),
            ],
        }
    };
    let named = |name: &str, value: &ValueTest| {
        std::iter::once(expanded(name))
            .chain(test(value))
            .collect::<Vec<_>>()
    };
    let group = |name: &str, conditions: &[RouteCondition]| {
        Directive::with_block(
            name,
            Vec::<String>::new(),
            conditions.iter().map(self::condition).collect(),
        )
    };
    match condition {
        RouteCondition::Method { methods } => {
            Directive::simple("method", methods.iter().map(|method| expanded(method)))
        }
        RouteCondition::Host { hosts } => {
            Directive::simple("host", hosts.iter().map(ToString::to_string))
        }
        RouteCondition::Header { name, test } => Directive::simple("header", named(name, test)),
        RouteCondition::Query { name, test } => Directive::simple("query", named(name, test)),
        RouteCondition::Cookie { name, test } => Directive::simple("cookie", named(name, test)),
        RouteCondition::Client { networks } => {
            Directive::simple("client", networks.iter().map(|network| expanded(network)))
        }
        RouteCondition::UserAgent { test: value } => Directive::simple("user_agent", test(value)),
        RouteCondition::Referer { test: value } => Directive::simple("referer", test(value)),
        RouteCondition::ContentType { types } => {
            Directive::simple("content_type", types.iter().map(|media| expanded(media)))
        }
        RouteCondition::All { conditions } => group("all", conditions),
        RouteCondition::Any { conditions } => group("any", conditions),
        RouteCondition::Not { condition: inner } => match inner.as_ref() {
            RouteCondition::All { conditions } if conditions.len() != 1 => group("not", conditions),
            other => group("not", std::slice::from_ref(other)),
        },
    }
}

fn route(route: &Route, model: &ConfigModel) -> Directive {
    let mut body = vec![Directive::simple("id", [route.id.to_string()])];
    let matcher = &route.matcher;
    let kind = match matcher.kind {
        panel_config_model::MatchKind::Exact => "exact",
        panel_config_model::MatchKind::Prefix => "prefix",
        panel_config_model::MatchKind::Glob => "glob",
        panel_config_model::MatchKind::Regex => "regex",
        _ => "prefix",
    };
    let path = if matcher.kind == panel_config_model::MatchKind::Regex {
        matcher.path.clone()
    } else {
        expanded(&matcher.path)
    };
    let mut args = vec![kind.to_owned(), path];
    if let Some(host) = &matcher.host {
        args.push(format!("host={host}"));
    }
    body.push(Directive::simple("match", args));
    body.extend(matcher.conditions.iter().map(condition));
    body.push(Directive::simple("priority", [route.priority.to_string()]));
    if !route.enabled {
        body.push(Directive::simple("enabled", ["off"]));
    }
    if let Some(policy) = &route.security_policy_id {
        body.push(Directive::simple("security_policy", [policy.clone()]));
    }
    if let Some(policy) = &route.http_policy_id {
        body.push(Directive::simple("http_policy", [policy.clone()]));
    }
    body.extend(print_access(&route.access_log));
    body.extend(lua_scope(&route.lua));
    body.push(action(&route.action, model));
    Directive::with_block("route", route.name.clone(), body)
}

pub fn server(site: &Site, model: &ConfigModel) -> Directive {
    let mut body = vec![Directive::simple("id", [site.id.to_string()])];
    let plain =
        |domain: &&panel_config_model::Domain| domain.enabled && domain.tls_profile_id.is_none();
    let has_primary = site.domains.iter().any(|domain| domain.primary);
    // Without an explicit `domain ... primary`, the first name is primary.
    let mut names: Vec<&panel_config_model::Domain> = site
        .domains
        .iter()
        .filter(|domain| plain(domain) && !domain.redirect)
        .collect();
    names.sort_by_key(|domain| !domain.primary);
    let primary_is_named = names.first().is_some_and(|domain| domain.primary) || !has_primary;
    if !names.is_empty() {
        body.push(Directive::simple(
            "server_name",
            names.iter().map(|domain| domain.host.to_string()),
        ));
    }
    let aliases: Vec<String> = site
        .domains
        .iter()
        .filter(|domain| plain(domain) && domain.redirect)
        .map(|domain| domain.host.to_string())
        .collect();
    if !aliases.is_empty() {
        body.push(Directive::simple("alias", aliases));
    }
    for domain in site.domains.iter().filter(|domain| !plain(domain)) {
        let mut args = vec![domain.host.to_string()];
        if domain.primary && !primary_is_named {
            args.push("primary".into());
        }
        if domain.redirect {
            args.push("alias".into());
        }
        if !domain.enabled {
            args.push("off".into());
        }
        if let Some(profile) = &domain.tls_profile_id {
            args.push(format!("tls_profile={profile}"));
        }
        body.push(Directive::simple("domain", args));
    }
    if !site.listener_ids.is_empty() {
        body.push(Directive::simple(
            "listen",
            site.listener_ids.iter().cloned(),
        ));
    }
    if let Some(profile) = &site.tls_profile_id {
        body.push(Directive::simple("tls_profile", [profile.clone()]));
    }
    if site.https_redirect {
        body.push(Directive::simple("https_redirect", ["on"]));
    }
    if let Some(hsts) = &site.hsts {
        let mut args = vec![format!(
            "max_age={}",
            print_duration_ms(hsts.max_age_seconds.saturating_mul(1_000))
        )];
        if hsts.include_subdomains {
            args.push("include_subdomains".into());
        }
        if hsts.preload {
            args.push("preload".into());
        }
        body.push(Directive::simple("hsts", args));
    }
    if let Some(policy) = &site.security_policy_id {
        body.push(Directive::simple("security_policy", [policy.clone()]));
    }
    if let Some(policy) = &site.http_policy_id {
        body.push(Directive::simple("http_policy", [policy.clone()]));
    }
    body.extend(print_access(&site.access_log));
    match site.www_redirect {
        WwwRedirect::None => {}
        WwwRedirect::AddWww => body.push(Directive::simple("www_redirect", ["add"])),
        WwwRedirect::RemoveWww => body.push(Directive::simple("www_redirect", ["remove"])),
    }
    if !site.enabled {
        body.push(Directive::simple("enabled", ["off"]));
    }
    if let Some(group) = &site.group {
        body.push(Directive::simple("group", [group.clone()]));
    }
    if !site.tags.is_empty() {
        body.push(Directive::simple("tags", site.tags.iter().cloned()));
    }
    if let Some(note) = &site.note {
        body.push(Directive::simple("note", [note.clone()]));
    }
    body.extend(lua_scope(&site.lua));
    body.push(action(&site.action, model));
    for item in &site.routes {
        let mut directive = route(item, model);
        directive.leading.push(Trivia::BlankLine);
        body.push(directive);
    }
    Directive::with_block("server", [site.name.clone()], body)
}
