//! `security_policy` blocks and the listener directives naming trusted
//! proxies.

use super::Lowerer;
use crate::{
    codes,
    schema::Context,
    values::{self, Params},
    variables,
};
use panel_config_model::SecurityPolicy;
use panel_domain::NormalizedHost;
use panel_dsl::{Argument, Directive};
use panel_ir::{BasicAuth, LimitedResponse, RateLimit, RateLimitKey, RealIpHeader, RefererRule};
use std::collections::BTreeSet;

/// The realm `basic_auth` asks for when the policy names none.
pub const DEFAULT_REALM: &str = "Restricted";

/// Rate periods written as a unit after `r/`.
pub const RATE_UNITS: [(&str, u64); 4] = [("s", 1), ("m", 60), ("h", 3600), ("d", 86_400)];

impl Lowerer<'_> {
    pub(super) fn security_policy(&mut self, file: &str, directive: &Directive, depth: usize) {
        let id = Self::literal(&directive.args[0]);
        if self.policies.iter().any(|(policy, _)| policy.id == id) {
            self.error(
                file,
                directive.args[0].span,
                codes::DUPLICATE,
                format!("security policy {id:?} is defined twice"),
            );
            return;
        }
        let mut policy = SecurityPolicy {
            id,
            ..SecurityPolicy::default()
        };
        if let Some(block) = directive.block() {
            self.with_scope(format!("security-policies/{}", policy.id), |lowerer| {
                let mut seen = BTreeSet::new();
                lowerer.each(
                    file,
                    &block.directives,
                    Context::SecurityPolicy,
                    depth + 1,
                    &mut seen,
                    &mut |lowerer, file, directive, spec, _| {
                        lowerer.policy_directive(file, directive, spec.name, &mut policy);
                    },
                );
            });
        }
        let origin = Self::origin(file, directive, depth);
        self.origins
            .insert(format!("security-policies/{}", policy.id), origin.clone());
        self.policies.push((policy, origin));
    }

    fn policy_directive(
        &mut self,
        file: &str,
        directive: &Directive,
        name: &str,
        policy: &mut SecurityPolicy,
    ) {
        let arg = &directive.args[0];
        match name {
            "allow" => {
                let networks = self.networks(file, &directive.args);
                policy.allowed_cidrs.extend(networks);
            }
            "deny" => {
                let networks = self.networks(file, &directive.args);
                policy.denied_cidrs.extend(networks);
            }
            "methods" => {
                for arg in &directive.args {
                    let method = &arg.value;
                    if method.is_empty()
                        || !method
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
                    {
                        self.error(
                            file,
                            arg.span,
                            codes::TYPE,
                            format!("{method:?} is not an HTTP method"),
                        );
                    } else {
                        policy.allowed_methods.push(method.to_ascii_uppercase());
                    }
                }
            }
            "deny_paths" => {
                for arg in &directive.args {
                    let Some(path) = self.value(file, arg) else {
                        continue;
                    };
                    if path.starts_with('/') {
                        policy.denied_path_prefixes.push(path);
                    } else {
                        self.error(
                            file,
                            arg.span,
                            codes::TYPE,
                            format!("{path:?} does not start with /"),
                        );
                    }
                }
            }
            "deny_user_agents" => policy
                .denied_user_agents
                .extend(directive.args.iter().map(|arg| arg.value.clone())),
            "referers" => {
                let mut rule = RefererRule {
                    allowed_hosts: Vec::new(),
                    allow_empty: false,
                };
                for arg in &directive.args {
                    if arg.value == "none" {
                        rule.allow_empty = true;
                        continue;
                    }
                    let Some(host) = self.value(file, arg) else {
                        continue;
                    };
                    let name = host.strip_prefix("*.").unwrap_or(&host);
                    if name.contains('*') || NormalizedHost::new(name).is_err() {
                        self.error_with_help(
                            file,
                            arg.span,
                            codes::TYPE,
                            format!("{host:?} is not a host"),
                            "write hosts such as example.com or *.example.com, and none for requests without a referer",
                        );
                    } else {
                        rule.allowed_hosts.push(host.to_ascii_lowercase());
                    }
                }
                policy.referer = Some(rule);
            }
            "basic_auth" => {
                let params = Params::split(&directive.args);
                self.only_params(file, &params, &["realm"]);
                let [users] = params.positional.as_slice() else {
                    self.error_with_help(
                        file,
                        directive.span,
                        codes::ARGUMENTS,
                        "basic_auth names one password file",
                        "write it as `basic_auth staff.htpasswd realm=Staff;`",
                    );
                    return;
                };
                let realm = match params.named.get("realm") {
                    Some((value, arg)) => self.text(file, arg, value),
                    None => Some(DEFAULT_REALM.to_owned()),
                };
                if let (Some(users_secret_id), Some(realm)) = (self.value(file, users), realm) {
                    policy.basic_auth = Some(BasicAuth {
                        realm,
                        users_secret_id,
                    });
                }
            }
            "max_header_size" => policy.max_header_bytes = self.size(file, arg),
            "max_body_size" => policy.max_body_bytes = self.size(file, arg),
            "body_timeout" => {
                let Some(value) = self.value(file, arg) else {
                    return;
                };
                let Some(ms) = self.duration(file, arg, &value) else {
                    return;
                };
                if ms == 0 || ms % 1000 != 0 {
                    self.error(
                        file,
                        arg.span,
                        codes::TYPE,
                        format!("{value:?} is not a whole number of seconds"),
                    );
                } else {
                    policy.body_timeout_seconds = Some(ms / 1000);
                }
            }
            "rate_limit" => {
                if let Some(limit) = self.rate_limit(file, directive) {
                    policy.rate_limits.push(limit);
                }
            }
            "max_concurrent" => {
                if let Some(value) = self.value(file, arg) {
                    policy.max_concurrent_requests =
                        self.number(file, arg, &value, "a whole number");
                }
            }
            "limited_response" => {
                let params = Params::split(&directive.args);
                self.only_params(file, &params, &["body", "type"]);
                let [status_arg] = params.positional.as_slice() else {
                    self.error_with_help(
                        file,
                        directive.span,
                        codes::ARGUMENTS,
                        "a limited response is a status with an optional body and type",
                        "write it as `limited_response 503 body=Busy type=text/plain;`",
                    );
                    return;
                };
                let Some(status) = self
                    .value(file, status_arg)
                    .and_then(|value| self.number::<u16>(file, status_arg, &value, "a status"))
                else {
                    return;
                };
                let body = match params.named.get("body") {
                    Some((value, arg)) => self.text(file, arg, value).unwrap_or_default(),
                    None => String::new(),
                };
                let content_type = params
                    .named
                    .get("type")
                    .and_then(|(value, arg)| self.text(file, arg, value));
                policy.limited_response = Some(LimitedResponse {
                    status,
                    body,
                    content_type,
                });
            }
            _ => unreachable!("the schema allows nothing else in security_policy"),
        }
    }

    /// Networks such as `10.0.0.0/8`, reporting the ones that are not.
    pub(super) fn networks(&mut self, file: &str, args: &[Argument]) -> Vec<String> {
        let mut networks = Vec::with_capacity(args.len());
        for arg in args {
            let Some(value) = self.value(file, arg) else {
                continue;
            };
            if values::parse_cidr(&value).is_some() {
                networks.push(value);
            } else {
                self.error_with_help(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{value:?} is not an IP network"),
                    "write networks such as 10.0.0.0/8 or 2001:db8::/32, or a single address",
                );
            }
        }
        networks
    }

    pub(super) fn real_ip_header(&mut self, file: &str, arg: &Argument) -> Option<RealIpHeader> {
        match arg.value.to_ascii_lowercase().as_str() {
            "x-forwarded-for" => Some(RealIpHeader::XForwardedFor),
            "x-real-ip" => Some(RealIpHeader::XRealIp),
            "forwarded" => Some(RealIpHeader::Forwarded),
            other => {
                self.error(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{other:?} is not x-forwarded-for, x-real-ip or forwarded"),
                );
                None
            }
        }
    }

    fn text(&mut self, file: &str, arg: &Argument, value: &str) -> Option<String> {
        self.expand(file, arg, value, super::Expansion::Text)
    }

    fn size(&mut self, file: &str, arg: &Argument) -> Option<u64> {
        let value = self.value(file, arg)?;
        let parsed = values::parse_size(&value).filter(|bytes| *bytes > 0);
        if parsed.is_none() {
            self.error_with_help(
                file,
                arg.span,
                codes::TYPE,
                format!("{value:?} is not a size"),
                "write sizes such as 8k, 10m or 1g",
            );
        }
        parsed
    }

    /// `rate_limit <n>r/<unit or duration> [burst=N] [key=...];`
    fn rate_limit(&mut self, file: &str, directive: &Directive) -> Option<RateLimit> {
        let params = Params::split(&directive.args);
        self.only_params(file, &params, &["burst", "key"]);
        let [rate_arg] = params.positional.as_slice() else {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                "a rate limit is one rate with optional parameters",
                "write it as `rate_limit 10r/s burst=20;`",
            );
            return None;
        };
        let rate = self.value(file, rate_arg)?;
        let Some((requests, per_seconds)) = parse_rate(&rate) else {
            self.error_with_help(
                file,
                rate_arg.span,
                codes::TYPE,
                format!("{rate:?} is not a rate"),
                "write rates such as 10r/s, 300r/m or 5r/10s",
            );
            return None;
        };
        let burst = match params.named.get("burst") {
            Some((value, arg)) => self.number(file, arg, value, "a whole number")?,
            None => 0,
        };
        let key = match params.named.get("key") {
            None => RateLimitKey::ClientAddress,
            Some((value, arg)) => match *value {
                "$client_ip" => RateLimitKey::ClientAddress,
                "$host" => RateLimitKey::Host,
                "$route" => RateLimitKey::Route,
                other => match variables::hash_key(other)
                    .as_deref()
                    .and_then(|key| key.strip_prefix("header:"))
                {
                    Some(name) => RateLimitKey::Header {
                        name: name.to_owned(),
                    },
                    None => {
                        self.error_with_help(
                            file,
                            arg.span,
                            codes::TYPE,
                            format!("{other:?} is not a rate limit key"),
                            "use $client_ip, $host, $route or $http_<name>",
                        );
                        return None;
                    }
                },
            },
        };
        Some(RateLimit {
            key,
            requests,
            per_seconds,
            burst,
        })
    }
}

/// `10r/s`, `300r/m`, `5r/10s`: requests and the seconds they are spread over.
pub fn parse_rate(value: &str) -> Option<(u64, u64)> {
    let (count, period) = value.split_once("r/")?;
    if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let requests = count.parse::<u64>().ok().filter(|requests| *requests > 0)?;
    let seconds = match RATE_UNITS.iter().find(|(unit, _)| *unit == period) {
        Some((_, seconds)) => *seconds,
        None => {
            let ms = values::parse_duration_ms(period)?;
            (ms > 0 && ms % 1000 == 0).then_some(ms / 1000)?
        }
    };
    Some((requests, seconds))
}

pub fn print_rate(requests: u64, per_seconds: u64) -> String {
    match RATE_UNITS
        .iter()
        .find(|(_, seconds)| *seconds == per_seconds)
    {
        Some((unit, _)) => format!("{requests}r/{unit}"),
        None => format!(
            "{requests}r/{}",
            values::print_duration_ms(per_seconds.saturating_mul(1000))
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_read_back_as_written() {
        for (text, rate) in [
            ("10r/s", (10, 1)),
            ("300r/m", (300, 60)),
            ("2r/h", (2, 3600)),
            ("1r/d", (1, 86_400)),
            ("5r/10s", (5, 10)),
        ] {
            assert_eq!(parse_rate(text), Some(rate), "{text}");
            assert_eq!(print_rate(rate.0, rate.1), text);
        }
        for invalid in ["0r/s", "10/s", "r/s", "10r/", "10r/500ms", "-1r/s", "10r/x"] {
            assert_eq!(parse_rate(invalid), None, "{invalid}");
        }
    }
}
