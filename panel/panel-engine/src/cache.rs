//! Checks of cache policies and the cache store (ADR 0043): keys are
//! templates, times and sizes are bounded, references resolve, and
//! snapshots caching require their capability.

use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::{
    template::parse_template, CachePolicy, RuntimeSnapshot, MOST_MAX_AGE_SECONDS,
    PROXY_CACHE_CAPABILITY,
};
use std::collections::BTreeSet;

/// The most cache policies a snapshot has.
pub const MOST_CACHE_POLICIES: usize = 64;
/// The smallest and largest store a gateway keeps.
pub const LEAST_CACHE_BYTES: u64 = 1 << 20;
pub const MOST_CACHE_BYTES: u64 = 64 << 30;
/// The largest response a policy may store, as large as the gateway's
/// in-memory store weighs entries by.
pub const MOST_OBJECT_BYTES: u64 = 64 << 20;
const MOST_KEY_BYTES: usize = 1024;
const MOST_VARY_HEADERS: usize = 16;
const MOST_STATUS_TTLS: usize = 64;

pub(crate) fn validate_cache(snapshot: &RuntimeSnapshot, diagnostics: &mut Vec<Diagnostic>) {
    let mut report = |resource: &str, message: String| {
        diagnostics
            .push(Diagnostic::error(ErrorCode::VALIDATION_FAILED, message).with_resource(resource));
    };
    if snapshot.cache_policies.len() > MOST_CACHE_POLICIES {
        report(
            "cache_policies",
            format!(
                "the snapshot has {} cache policies, more than {MOST_CACHE_POLICIES}",
                snapshot.cache_policies.len()
            ),
        );
    }
    let mut ids = BTreeSet::new();
    for policy in &snapshot.cache_policies {
        if !crate::conditions::token(&policy.id) || policy.id.len() > 128 {
            report(
                &policy.id,
                format!("cache policy id {:?} is not a token", policy.id),
            );
        } else if !ids.insert(policy.id.as_str()) {
            report(
                &policy.id,
                format!("cache policy {} is defined twice", policy.id),
            );
        }
        for problem in cache_policy_problems(policy) {
            report(&policy.id, format!("cache policy {} {problem}", policy.id));
        }
    }
    let references = snapshot
        .sites
        .iter()
        .map(|site| (site.id.as_str(), "site", site.cache_policy_id.as_deref()))
        .chain(
            snapshot
                .routes
                .iter()
                .map(|route| (route.id.as_str(), "route", route.cache_policy_id.as_deref())),
        );
    for (owner, kind, policy) in references {
        if let Some(policy) = policy.filter(|policy| !ids.contains(policy)) {
            report(
                owner,
                format!("{kind} {owner} references unknown cache policy {policy}"),
            );
        }
    }
    if let Some(bytes) = snapshot
        .cache_max_bytes
        .filter(|bytes| !(LEAST_CACHE_BYTES..=MOST_CACHE_BYTES).contains(bytes))
    {
        report(
            "cache_max_bytes",
            format!(
                "the cache store of {bytes} bytes is not between {LEAST_CACHE_BYTES} and {MOST_CACHE_BYTES}"
            ),
        );
    }
    let caching = snapshot.cache_policies.iter().any(|policy| policy.enabled);
    let declared = snapshot
        .required_capabilities()
        .iter()
        .any(|capability| capability.name == PROXY_CACHE_CAPABILITY);
    if caching && !declared {
        report(
            "cache_policies",
            format!("cache policies are enabled without requiring {PROXY_CACHE_CAPABILITY}"),
        );
    }
}

/// What is wrong with `policy`, one line each.
pub fn cache_policy_problems(policy: &CachePolicy) -> Vec<String> {
    let mut found = Vec::new();
    if policy.ttl_seconds > MOST_MAX_AGE_SECONDS {
        found.push(format!(
            "keeps responses fresh for {} seconds, more than {MOST_MAX_AGE_SECONDS}",
            policy.ttl_seconds
        ));
    }
    if policy.status_ttls.len() > MOST_STATUS_TTLS {
        found.push(format!(
            "has times for {} statuses, more than {MOST_STATUS_TTLS}",
            policy.status_ttls.len()
        ));
    }
    for (status, seconds) in &policy.status_ttls {
        if !(100..=599).contains(status) {
            found.push(format!("has a time for {status}, which is not a status"));
        }
        if *seconds > MOST_MAX_AGE_SECONDS {
            found.push(format!(
                "keeps {status} responses fresh for {seconds} seconds, more than {MOST_MAX_AGE_SECONDS}"
            ));
        }
    }
    if let Some(key) = &policy.key {
        if key.is_empty() || key.len() > MOST_KEY_BYTES {
            found.push(format!(
                "has a key of {} bytes; it needs 1..={MOST_KEY_BYTES}",
                key.len()
            ));
        } else if let Err(error) = parse_template(key) {
            found.push(format!("has a key that is not a template: {error}"));
        }
    }
    if policy.vary_headers.len() > MOST_VARY_HEADERS {
        found.push(format!(
            "varies by {} fields, more than {MOST_VARY_HEADERS}",
            policy.vary_headers.len()
        ));
    }
    for name in &policy.vary_headers {
        if !crate::conditions::token(name) || name.bytes().any(|byte| byte.is_ascii_uppercase()) {
            found.push(format!(
                "varies by {name:?}, which is not a lowercase field name"
            ));
        }
    }
    for problem in crate::conditions::problems(&policy.bypass) {
        found.push(format!("bypasses requests that {problem}"));
    }
    for (seconds, what) in [
        (policy.stale_while_revalidate_seconds, "while revalidating"),
        (policy.stale_if_error_seconds, "when the upstream fails"),
    ] {
        if u64::from(seconds) > MOST_MAX_AGE_SECONDS {
            found.push(format!(
                "serves stale responses {what} for {seconds} seconds, more than {MOST_MAX_AGE_SECONDS}"
            ));
        }
    }
    if let Some(bytes) = policy
        .max_object_bytes
        .filter(|bytes| !(1..=MOST_OBJECT_BYTES).contains(bytes))
    {
        found.push(format!(
            "stores responses of up to {bytes} bytes; it needs 1..={MOST_OBJECT_BYTES}"
        ));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{RevisionId, RouteId, SiteId};
    use panel_ir::{
        CapabilityRequirement, RouteAction, RouteCondition, RouteMatcher, RouteSpec, SiteSpec,
        ValueTest,
    };

    #[test]
    fn sound_policies_pass_and_broken_ones_are_told() {
        let mut policy = CachePolicy::new("pages");
        policy.ttl_seconds = 3600;
        policy.status_ttls = [(404, 60)].into();
        policy.key = Some("$host$uri".into());
        policy.vary_headers = ["accept-language".to_owned()].into();
        policy.bypass = vec![RouteCondition::Cookie {
            name: "session".into(),
            test: ValueTest::Present,
        }];
        assert_eq!(cache_policy_problems(&policy), Vec::<String>::new());

        policy.status_ttls = [(42, 60), (500, MOST_MAX_AGE_SECONDS + 1)].into();
        policy.key = Some("$nope".into());
        policy.vary_headers = ["Accept".to_owned()].into();
        policy.max_object_bytes = Some(0);
        let found = cache_policy_problems(&policy);
        for expected in [
            "time for 42, which is not a status",
            "keeps 500 responses fresh for 2147483649 seconds",
            "key that is not a template",
            "varies by \"Accept\"",
            "stores responses of up to 0 bytes",
        ] {
            assert!(
                found.iter().any(|line| line.contains(expected)),
                "{expected:?} in {found:#?}"
            );
        }
    }

    #[test]
    fn references_resolve_and_caching_requires_its_capability() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        let mut site = SiteSpec::new(SiteId::new("shop").unwrap(), "shop", Vec::new());
        site.cache_policy_id = Some("pages".into());
        snapshot.sites.push(site);
        let mut route = RouteSpec::new(
            RouteId::new("api").unwrap(),
            SiteId::new("shop").unwrap(),
            1,
            RouteMatcher::ExactPath { path: "/".into() },
            RouteAction::Respond {
                status: 204,
                body: None,
                content_type: None,
                retry_after_seconds: None,
            },
        );
        route.cache_policy_id = Some("missing".into());
        snapshot.routes.push(route);
        snapshot.cache_policies.push(CachePolicy::new("pages"));
        snapshot.cache_max_bytes = Some(1024);
        let mut diagnostics = Vec::new();
        validate_cache(&snapshot, &mut diagnostics);
        let messages: Vec<_> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        for expected in [
            "route api references unknown cache policy missing",
            "cache store of 1024 bytes",
            "without requiring proxy.cache",
        ] {
            assert!(
                messages.iter().any(|message| message.contains(expected)),
                "{expected:?} in {messages:#?}"
            );
        }
        assert!(!messages.iter().any(|message| message.contains("site shop")));

        snapshot.routes[0].cache_policy_id = None;
        snapshot.cache_max_bytes = Some(64 << 20);
        snapshot
            .required_capabilities
            .push(CapabilityRequirement::new(PROXY_CACHE_CAPABILITY, "1"));
        let mut diagnostics = Vec::new();
        validate_cache(&snapshot, &mut diagnostics);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }
}
