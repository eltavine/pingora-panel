//! The configuration model written in the language: the canonical text the
//! service stores and formats, with defaults left out.

use crate::{
    values::{print_bool, print_duration_ms},
    variables::{escape, print_hash_key},
    LANGUAGE_VERSION,
};
use panel_config_model::{
    Action, ConfigModel, Listener, Route, Site, TlsProfile, Upstream, UpstreamNode,
};
use panel_dsl::{Directive, Document, Trivia};
use panel_ir::{HealthCheckProtocol, ListenerProtocols, LoadBalancingPolicy, WwwRedirect};

/// The whole model as `main.conf`.
pub fn print(model: &ConfigModel) -> String {
    panel_dsl::format(&document(model))
}

pub fn document(model: &ConfigModel) -> Document {
    let mut http = Vec::new();
    http.extend(model.tls_profiles.iter().map(tls_profile));
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
        if index > 0 {
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
    if !profile.alpn.is_empty() {
        body.push(Directive::simple("alpn", profile.alpn.iter().cloned()));
    }
    Directive::with_block("tls_profile", [profile.id.clone()], body)
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
        _ => Directive::simple("respond", ["503"]),
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
    body.push(Directive::simple("priority", [route.priority.to_string()]));
    if !route.enabled {
        body.push(Directive::simple("enabled", ["off"]));
    }
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
    body.push(action(&site.action, model));
    for item in &site.routes {
        let mut directive = route(item, model);
        directive.leading.push(Trivia::BlankLine);
        body.push(directive);
    }
    Directive::with_block("server", [site.name.clone()], body)
}
