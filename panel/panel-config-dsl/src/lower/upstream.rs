//! `upstream` blocks: their `server` nodes and health checks.

use super::{Expansion, Insertion, Lowerer};
use crate::{
    codes,
    schema::{Context, DirectiveSpec},
    values::Params,
    variables,
};
use panel_config_model::{Upstream, UpstreamNode};
use panel_dsl::{Argument, Body, Directive};
use panel_ir::{
    ActiveHealthCheck, HealthCheckProtocol, LoadBalancingPolicy, PassiveHealthPolicy,
    UpstreamConnectionPolicy, UpstreamTlsPolicy,
};
use std::collections::BTreeSet;
use uuid::Uuid;

impl<'a> Lowerer<'a> {
    pub(super) fn upstream(&mut self, file: &str, directive: &Directive, depth: usize) {
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
            balancer: Default::default(),
            id,
            name,
            nodes: Vec::new(),
            balancing: LoadBalancingPolicy::RoundRobin,
            host_header: None,
            tls: UpstreamTlsPolicy::default(),
            connection: UpstreamConnectionPolicy::default(),
            health_check: None,
            passive_health: None,
            retry: None,
            circuit_breaker: None,
            max_requests: None,
            queue: None,
            note: None,
            created_at: now,
            updated_at: now,
        };
        let Some(block) = directive.block() else {
            return;
        };
        self.with_scope(format!("upstreams/{}", upstream.id), |lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::Upstream,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, depth| {
                    lowerer.upstream_directive(file, directive, spec, depth, &mut upstream);
                },
            );
        });
        let origin = Self::origin(file, directive, depth);
        self.origins
            .insert(format!("upstreams/{}", upstream.id), origin.clone());
        self.upstreams.push((upstream, origin));
    }

    pub(super) fn upstream_directive(
        &mut self,
        file: &str,
        directive: &Directive,
        spec: &DirectiveSpec,
        depth: usize,
        upstream: &mut Upstream,
    ) {
        let empty = Argument::new("");
        let arg = directive.args.first().unwrap_or(&empty);
        let duration = |lowerer: &mut Self| {
            let value = lowerer.value(file, arg)?;
            lowerer.duration(file, arg, &value)
        };
        match spec.name {
            "id" => {}
            "balancer_by_lua_block" | "balancer_by_lua_file" => {
                if upstream.balancer.is_some() {
                    self.error(
                        file,
                        directive.name.span,
                        codes::DUPLICATE,
                        "the upstream already has balancer_by_lua",
                    );
                } else {
                    upstream.balancer = self.lua_code(file, directive);
                }
            }
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
                        self.origins.insert(
                            format!("upstreams/{}/nodes/{}", upstream.id, node.id),
                            Self::origin(file, directive, depth),
                        );
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
                            .expand(file, key_arg, key, Expansion::Request)
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
                        "sni" => upstream.tls.sni = self.expand(file, arg, value, Expansion::Text),
                        "ca" => {
                            upstream.tls.ca_secret_id =
                                self.expand(file, arg, value, Expansion::Text)
                        }
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
            "h2c" => upstream.connection.h2c = self.bool_arg(file, arg).unwrap_or_default(),
            "retry" => upstream.retry = self.retry(file, directive),
            "circuit_breaker" => upstream.circuit_breaker = self.circuit_breaker(file, directive),
            "max_requests" => {
                if let Some(value) = self.value(file, arg) {
                    upstream.max_requests = self.number(file, arg, &value, "a whole number");
                }
            }
            "queue" => upstream.queue = self.queue(file, directive),
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

    pub(super) fn node(&mut self, file: &str, directive: &Directive) -> Option<UpstreamNode> {
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
                "sni" => node.sni = self.expand(file, arg, value, Expansion::Text),
                "note" => node.note = Some((*value).to_owned()),
                "unix" => node.unix_socket = self.expand(file, arg, value, Expansion::Text),
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

    pub(super) fn health_check(
        &mut self,
        file: &str,
        directive: &Directive,
    ) -> Option<ActiveHealthCheck> {
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
                "path" => {
                    check.path = self
                        .expand(file, arg, value, Expansion::Text)
                        .unwrap_or_default()
                }
                "method" => check.method = value.to_ascii_uppercase(),
                "host" => check.host = self.expand(file, arg, value, Expansion::Text),
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
}

/// `host:port`, with IPv6 hosts in brackets.
pub(super) fn split_host_port(address: &str) -> Option<(String, u16)> {
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
