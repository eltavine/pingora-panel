//! Security policies of the editable configuration (ADR 0017): what they
//! hold, how they are checked and what the gateway receives.

use crate::validate::is_token;
use panel_ir::{BasicAuth, LimitedResponse, RateLimit, RateLimitKey, RefererRule};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// The longest wait between body reads a policy may set.
pub const MAX_BODY_TIMEOUT_SECONDS: u64 = 3600;
/// The longest rate limit period, a day.
pub const MAX_RATE_PERIOD_SECONDS: u64 = 86_400;

/// Restrictions requests must pass, written once and named by sites and
/// routes; a request passes its site's policy and then its route's.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SecurityPolicy {
    /// Lowercase letters, digits, dots, hyphens and underscores.
    pub id: String,
    /// Client networks in CIDR notation; when any are listed, other
    /// clients are refused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_cidrs: Vec<String>,
    /// Client networks refused even when allowed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_cidrs: Vec<String>,
    /// Methods allowed; empty allows every method, and HEAD goes with GET.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_methods: Vec<String>,
    /// Paths refused when they start with one of these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_path_prefixes: Vec<String>,
    /// User agents refused when one of these case-insensitive regular
    /// expressions matches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_user_agents: Vec<String>,
    /// Which pages may link to the resources (hotlink protection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referer: Option<RefererRule>,
    /// Requires a user from an htpasswd file in the gateway's secret
    /// directory, with bcrypt or Argon2 hashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic_auth: Option<BasicAuth>,
    /// The most bytes of request headers, answered with 431 above it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_header_bytes: Option<u64>,
    /// The largest request body, answered with 413 above it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_bytes: Option<u64>,
    /// The longest wait for the next part of a request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_timeout_seconds: Option<u64>,
    /// Token buckets; requests over one are answered with 429.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rate_limits: Vec<RateLimit>,
    /// The most requests one client address may have in progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_requests: Option<u64>,
    /// The answer to requests over a limit, instead of 429.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limited_response: Option<LimitedResponse>,
}

impl SecurityPolicy {
    /// The policy as the gateway receives it.
    pub fn compile(&self) -> panel_ir::SecurityPolicy {
        panel_ir::SecurityPolicy {
            id: self.id.clone(),
            allowed_cidrs: self.allowed_cidrs.iter().cloned().collect(),
            denied_cidrs: self.denied_cidrs.iter().cloned().collect(),
            request_rate_per_second: None,
            allowed_methods: self
                .allowed_methods
                .iter()
                .map(|method| method.to_ascii_uppercase())
                .collect(),
            denied_path_prefixes: self.denied_path_prefixes.clone(),
            denied_user_agents: self.denied_user_agents.clone(),
            referer: self.referer.clone().map(|rule| RefererRule {
                allowed_hosts: rule
                    .allowed_hosts
                    .iter()
                    .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
                    .collect(),
                ..rule
            }),
            basic_auth: self.basic_auth.clone(),
            max_header_bytes: self.max_header_bytes,
            max_body_bytes: self.max_body_bytes,
            body_timeout_ms: self
                .body_timeout_seconds
                .map(|seconds| seconds.saturating_mul(1000)),
            rate_limits: self.rate_limits.clone(),
            max_concurrent_requests: self.max_concurrent_requests,
            limited_response: self.limited_response.clone(),
        }
    }

    /// Every problem with the policy on its own, as messages.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for cidr in self.allowed_cidrs.iter().chain(&self.denied_cidrs) {
            if !is_cidr(cidr) {
                problems.push(format!("{cidr:?} is not a CIDR network such as 10.0.0.0/8"));
            }
        }
        for method in &self.allowed_methods {
            if method.is_empty() || !method.bytes().all(is_tchar) {
                problems.push(format!("{method:?} is not an HTTP method"));
            }
        }
        for prefix in &self.denied_path_prefixes {
            if !prefix.starts_with('/') {
                problems.push(format!("denied path {prefix:?} must start with /"));
            }
        }
        for pattern in &self.denied_user_agents {
            if let Err(error) = regex::RegexBuilder::new(pattern)
                .size_limit(1 << 20)
                .build()
            {
                problems.push(format!(
                    "user agent pattern {pattern:?} is invalid: {error}"
                ));
            }
        }
        if let Some(rule) = &self.referer {
            if rule.allowed_hosts.is_empty() && !rule.allow_empty {
                problems.push("a referer rule must allow some host or requests without one".into());
            }
            for host in &rule.allowed_hosts {
                let name = host.strip_prefix("*.").unwrap_or(host);
                if panel_domain::NormalizedHost::new(name).is_err() || name.contains('*') {
                    problems.push(format!("referring host {host:?} is not a host or *.host"));
                }
            }
        }
        if let Some(auth) = &self.basic_auth {
            if auth.realm.trim().is_empty()
                || auth.realm.len() > 128
                || auth
                    .realm
                    .chars()
                    .any(|character| character.is_control() || character == '"')
            {
                problems.push("the Basic authentication realm must be 1 to 128 printable characters without quotes".into());
            }
            if !is_token(&auth.users_secret_id) || auth.users_secret_id.starts_with('.') {
                problems.push(format!(
                    "the password file {:?} must be a plain file name",
                    auth.users_secret_id
                ));
            }
        }
        for (limit, name) in [
            (self.max_header_bytes, "header size limit"),
            (self.max_body_bytes, "body size limit"),
            (self.max_concurrent_requests, "concurrent request limit"),
        ] {
            if limit == Some(0) {
                problems.push(format!("the {name} must be positive"));
            }
        }
        if let Some(seconds) = self.body_timeout_seconds {
            if !(1..=MAX_BODY_TIMEOUT_SECONDS).contains(&seconds) {
                problems.push(format!(
                    "the body timeout must be 1 to {MAX_BODY_TIMEOUT_SECONDS} seconds"
                ));
            }
        }
        for limit in &self.rate_limits {
            if limit.requests == 0 {
                problems.push("a rate limit must allow at least one request".into());
            }
            if !(1..=MAX_RATE_PERIOD_SECONDS).contains(&limit.per_seconds) {
                problems.push(format!(
                    "a rate limit period must be 1 to {MAX_RATE_PERIOD_SECONDS} seconds"
                ));
            }
            if let RateLimitKey::Header { name } = &limit.key {
                if name.is_empty() || !name.bytes().all(is_tchar) {
                    problems.push(format!("{name:?} is not a header name"));
                }
            }
        }
        if let Some(response) = &self.limited_response {
            if !(400..=599).contains(&response.status) {
                problems.push("the limited response status must be 400 to 599".into());
            }
            if response.body.len() > 64 * 1024 {
                problems.push("the limited response body is larger than 64 KiB".into());
            }
        }
        problems
    }
}

/// RFC 9110 §5.6.2 token characters.
fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

/// A network such as `10.0.0.0/8`, or a single address.
pub(crate) fn is_cidr(value: &str) -> bool {
    let (address, prefix) = value
        .split_once('/')
        .map_or((value, None), |(address, prefix)| (address, Some(prefix)));
    let Ok(address) = address.parse::<IpAddr>() else {
        return false;
    };
    let max = if address.is_ipv4() { 32 } else { 128 };
    prefix.is_none_or(|bits| {
        !bits.is_empty()
            && bits.bytes().all(|byte| byte.is_ascii_digit())
            && bits.parse::<u8>().is_ok_and(|bits| bits <= max)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_report_each_problem() {
        let policy = SecurityPolicy {
            id: "edge".into(),
            allowed_cidrs: vec!["10.0.0.0/8".into(), "10.0.0.0/40".into()],
            allowed_methods: vec!["get".into(), "GE T".into()],
            denied_path_prefixes: vec!["admin".into()],
            denied_user_agents: vec!["(".into()],
            referer: Some(RefererRule {
                allowed_hosts: vec!["*.example.com".into(), "*bad".into()],
                allow_empty: false,
            }),
            basic_auth: Some(BasicAuth {
                realm: "Staff \"x\"".into(),
                users_secret_id: "../users".into(),
            }),
            body_timeout_seconds: Some(0),
            rate_limits: vec![RateLimit {
                key: RateLimitKey::Header {
                    name: "x key".into(),
                },
                requests: 0,
                per_seconds: 0,
                burst: 0,
            }],
            limited_response: Some(LimitedResponse {
                status: 200,
                body: String::new(),
                content_type: None,
            }),
            ..SecurityPolicy::default()
        };
        assert_eq!(policy.problems().len(), 12, "{:#?}", policy.problems());
        let compiled = policy.compile();
        assert!(compiled.allowed_methods.contains("GET"));
        assert_eq!(compiled.body_timeout_ms, Some(0));
        assert!(SecurityPolicy {
            id: "ok".into(),
            denied_cidrs: vec!["::/0".into()],
            ..SecurityPolicy::default()
        }
        .problems()
        .is_empty());
    }
}
