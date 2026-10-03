//! Checks that speak in operator terms — names, references, duplicates and
//! value syntax. Engine-neutral IR validation still runs on the compiled
//! snapshot; this layer exists so problems point at the resource to fix.

use crate::model::{Action, ConfigModel, MatchKind, Route, Site};
use panel_domain::EndpointAddress;
use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::template::parse_template;
use panel_ir::tls::{suite_version, SuiteVersion, PROTOCOLS};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    net::SocketAddr,
    path::{Component, Path},
};
use uuid::Uuid;

const MAX_NAME_BYTES: usize = 128;
const MAX_LABEL_BYTES: usize = 64;
const MAX_NOTE_BYTES: usize = 2048;
pub(crate) const REDIRECT_STATUSES: [u16; 5] = [301, 302, 303, 307, 308];

struct Report(Vec<Diagnostic>);

impl Report {
    fn error(&mut self, resource: impl Into<String>, message: impl Into<String>) {
        self.0
            .push(Diagnostic::error(ErrorCode::VALIDATION_FAILED, message).with_resource(resource));
    }
}

/// Every problem in `model`; an empty result means it may be compiled.
pub fn validate(model: &ConfigModel) -> Vec<Diagnostic> {
    let mut report = Report(Vec::new());
    let tls_profiles = validate_tls_profiles(model, &mut report);
    let policies = validate_security_policies(model, &mut report);
    let upstreams = validate_upstreams(model, &mut report);
    let live_sites: BTreeSet<Uuid> = model
        .sites
        .iter()
        .filter(|site| !site.is_deleted())
        .map(|site| site.id)
        .collect();
    let listeners = validate_listeners(model, &tls_profiles, &live_sites, &mut report);

    let mut names = BTreeMap::new();
    let mut hosts = BTreeMap::new();
    let mut route_ids = HashSet::new();
    let mut site_ids = HashSet::new();
    for site in &model.sites {
        let resource = format!("sites/{}", site.id);
        if !site_ids.insert(site.id) {
            report.error(
                &resource,
                format!("site id {} is used more than once", site.id),
            );
        }
        if site.is_deleted() {
            continue;
        }
        check_text(
            &mut report,
            &resource,
            "name",
            Some(&site.name),
            MAX_NAME_BYTES,
            true,
        );
        if let Some(other) = names.insert(site.name.to_lowercase(), &site.name) {
            report.error(
                &resource,
                format!("site name {:?} is already used by {other:?}", site.name),
            );
        }
        check_text(
            &mut report,
            &resource,
            "group",
            site.group.as_deref(),
            MAX_LABEL_BYTES,
            false,
        );
        check_text(
            &mut report,
            &resource,
            "note",
            site.note.as_deref(),
            MAX_NOTE_BYTES,
            false,
        );
        for tag in &site.tags {
            check_text(
                &mut report,
                &resource,
                "tag",
                Some(tag),
                MAX_LABEL_BYTES,
                true,
            );
        }
        for listener in site
            .listener_ids
            .iter()
            .filter(|id| !listeners.contains(id.as_str()))
        {
            report.error(&resource, format!("listener {listener} does not exist"));
        }
        if let Some(profile) = site
            .tls_profile_id
            .as_deref()
            .filter(|profile| !tls_profiles.contains(profile))
        {
            report.error(&resource, format!("TLS profile {profile} does not exist"));
        }
        if let Some(policy) = site
            .security_policy_id
            .as_deref()
            .filter(|policy| !policies.contains(policy))
        {
            report.error(
                &resource,
                format!("security policy {policy} does not exist"),
            );
        }
        validate_domains(site, &resource, &tls_profiles, &mut hosts, &mut report);
        if site.hsts.is_some_and(|hsts| {
            hsts.preload && (!hsts.include_subdomains || hsts.max_age_seconds < PRELOAD_MAX_AGE)
        }) {
            report.error(
                &resource,
                "HSTS preloading needs includeSubDomains and a max-age of at least a year (31536000 seconds)",
            );
        }
        check_action(&site.action, &resource, &upstreams, &mut report);
        for route in &site.routes {
            if !route_ids.insert(route.id) {
                report.error(
                    format!("{resource}/routes/{}", route.id),
                    format!("route id {} is used more than once", route.id),
                );
            }
            validate_route(site, route, &resource, &upstreams, &mut report);
            if let Some(policy) = route
                .security_policy_id
                .as_deref()
                .filter(|policy| !policies.contains(policy))
            {
                report.error(
                    format!("{resource}/routes/{}", route.id),
                    format!("security policy {policy} does not exist"),
                );
            }
        }
    }
    report.0
}

fn validate_tls_profiles<'a>(model: &'a ConfigModel, report: &mut Report) -> BTreeSet<&'a str> {
    let mut ids = BTreeSet::new();
    for profile in &model.tls_profiles {
        let resource = format!("tls-profiles/{}", profile.id);
        if !is_token(&profile.id) || !ids.insert(profile.id.as_str()) {
            report.error(
                &resource,
                format!("TLS profile id {:?} is invalid or duplicated", profile.id),
            );
        }
        let named_files =
            !profile.certificate_secret_id.is_empty() || !profile.private_key_secret_id.is_empty();
        if profile.certificate_id.is_some() {
            if named_files {
                report.error(
                    &resource,
                    "name either a certificate of the inventory or files in the secret directory, not both",
                );
            }
        } else {
            for (field, secret) in [
                ("certificate", &profile.certificate_secret_id),
                ("private key", &profile.private_key_secret_id),
            ] {
                if !is_token(secret) || secret.starts_with('.') {
                    report.error(
                        &resource,
                        format!("{field} secret {secret:?} must be a plain file name"),
                    );
                } else if secret.starts_with(DELIVERED_PREFIX) {
                    report.error(
                        &resource,
                        format!(
                            "{field} secret {secret:?} is a file delivered for a certificate of the inventory; name that certificate with certificate_id instead"
                        ),
                    );
                }
            }
        }
        if !matches!(profile.min_protocol.as_str(), "TLSv1.2" | "TLSv1.3") {
            report.error(&resource, "minimum protocol must be TLSv1.2 or TLSv1.3");
        }
        validate_tls_settings(profile, &resource, report);
    }
    ids
}

fn validate_security_policies<'a>(
    model: &'a ConfigModel,
    report: &mut Report,
) -> BTreeSet<&'a str> {
    let mut ids = BTreeSet::new();
    for policy in &model.security_policies {
        let resource = format!("security-policies/{}", policy.id);
        if !is_token(&policy.id) || !ids.insert(policy.id.as_str()) {
            report.error(
                &resource,
                format!(
                    "security policy id {:?} is invalid or duplicated",
                    policy.id
                ),
            );
        }
        for problem in policy.problems() {
            report.error(&resource, problem);
        }
    }
    ids
}

fn validate_upstreams(model: &ConfigModel, report: &mut Report) -> BTreeSet<Uuid> {
    let mut ids = BTreeSet::new();
    let mut names = BTreeMap::new();
    for upstream in &model.upstreams {
        let resource = format!("upstreams/{}", upstream.id);
        if !ids.insert(upstream.id) {
            report.error(
                &resource,
                format!("upstream id {} is used more than once", upstream.id),
            );
        }
        check_text(
            report,
            &resource,
            "name",
            Some(&upstream.name),
            MAX_NAME_BYTES,
            true,
        );
        if let Some(other) = names.insert(upstream.name.to_lowercase(), &upstream.name) {
            report.error(
                &resource,
                format!(
                    "upstream name {:?} is already used by {other:?}",
                    upstream.name
                ),
            );
        }
        check_text(
            report,
            &resource,
            "note",
            upstream.note.as_deref(),
            MAX_NOTE_BYTES,
            false,
        );
        let mut nodes = HashSet::new();
        for node in &upstream.nodes {
            let node_resource = format!("{resource}/nodes/{}", node.id);
            if !nodes.insert(node.id) {
                report.error(
                    &node_resource,
                    format!("node id {} is used more than once", node.id),
                );
            }
            if let Err(error) = EndpointAddress::new(&node.host, node.port, node.tls) {
                report.error(&node_resource, format!("node address is invalid: {error}"));
            }
            if node.weight == 0 {
                report.error(&node_resource, "node weight must be positive");
            }
            check_text(
                report,
                &node_resource,
                "note",
                node.note.as_deref(),
                MAX_NOTE_BYTES,
                false,
            );
        }
    }
    ids
}

fn validate_listeners<'a>(
    model: &'a ConfigModel,
    tls_profiles: &BTreeSet<&str>,
    live_sites: &BTreeSet<Uuid>,
    report: &mut Report,
) -> BTreeSet<&'a str> {
    let mut ids = BTreeSet::new();
    for listener in &model.listeners {
        let resource = format!("listeners/{}", listener.id);
        if !is_token(&listener.id) || !ids.insert(listener.id.as_str()) {
            report.error(
                &resource,
                format!("listener id {:?} is invalid or duplicated", listener.id),
            );
        }
        match listener.address.parse::<SocketAddr>() {
            Ok(address) if address.port() == 0 => {
                report.error(&resource, "listener port must not be zero")
            }
            Ok(_) => {}
            Err(_) => report.error(
                &resource,
                format!("{:?} is not an IP address and port", listener.address),
            ),
        }
        if !listener.protocols.http1 && !listener.protocols.http2 {
            report.error(&resource, "a listener must accept HTTP/1.1 or HTTP/2");
        }
        if let Some(profile) = listener
            .tls_profile_id
            .as_deref()
            .filter(|profile| !tls_profiles.contains(profile))
        {
            report.error(&resource, format!("TLS profile {profile} does not exist"));
        }
        if let Some(site) = listener
            .default_site_id
            .filter(|site| !live_sites.contains(site))
        {
            report.error(&resource, format!("default site {site} does not exist"));
        }
        for proxy in listener
            .trusted_proxies
            .iter()
            .filter(|proxy| !crate::security::is_cidr(proxy))
        {
            report.error(
                &resource,
                format!("trusted proxy {proxy:?} is not a CIDR network such as 10.0.0.0/8"),
            );
        }
    }
    ids
}

fn validate_domains<'a>(
    site: &'a Site,
    resource: &str,
    tls_profiles: &BTreeSet<&str>,
    hosts: &mut BTreeMap<&'a str, &'a str>,
    report: &mut Report,
) {
    let mut primaries = 0;
    for domain in &site.domains {
        let domain_resource = format!("{resource}/domains/{}", domain.host);
        if let Some(owner) = hosts.insert(domain.host.as_str(), &site.name) {
            report.error(
                &domain_resource,
                format!("domain {} is already bound to site {owner:?}", domain.host),
            );
        }
        if domain.primary {
            primaries += 1;
            if domain.host.is_wildcard() || domain.redirect || !domain.enabled {
                report.error(
                    &domain_resource,
                    "the primary domain must be an enabled, concrete name",
                );
            }
        }
        if let Some(profile) = domain
            .tls_profile_id
            .as_deref()
            .filter(|profile| !tls_profiles.contains(profile))
        {
            report.error(
                &domain_resource,
                format!("TLS profile {profile} does not exist"),
            );
        }
    }
    if primaries > 1 {
        report.error(resource, "a site has at most one primary domain");
    }
    if primaries == 0 && site.domains.iter().any(|domain| domain.redirect) {
        report.error(resource, "alias redirects need a primary domain");
    }
}

fn validate_route(
    site: &Site,
    route: &Route,
    resource: &str,
    upstreams: &BTreeSet<Uuid>,
    report: &mut Report,
) {
    let resource = format!("{resource}/routes/{}", route.id);
    check_text(
        report,
        &resource,
        "name",
        route.name.as_deref(),
        MAX_NAME_BYTES,
        false,
    );
    let path = &route.matcher.path;
    let valid = match route.matcher.kind {
        MatchKind::Exact | MatchKind::Prefix => {
            path.starts_with('/') && !path.contains(char::is_whitespace) && !path.contains("..")
        }
        MatchKind::Glob => path.starts_with('/') && !path.contains(char::is_whitespace),
        MatchKind::Regex => !path.is_empty() && path.len() <= 1024,
    };
    if !valid {
        report.error(
            &resource,
            format!("{path:?} is not a valid {:?} match", route.matcher.kind),
        );
    } else if route.matcher.kind == MatchKind::Regex {
        if let Some(error) = panel_engine::route_regex_error(path) {
            report.error(
                &resource,
                format!("the regular expression {path:?} does not compile: {error}"),
            );
        }
    }
    if let Some(host) = &route.matcher.host {
        if route.matcher.kind != MatchKind::Prefix {
            report.error(&resource, "a host restriction needs a prefix match");
        }
        if !site
            .domains
            .iter()
            .any(|domain| domain.host == *host || domain.host.matches(host))
        {
            report.error(
                &resource,
                format!("host {host} is not one of the site's domains"),
            );
        }
    }
    check_action(&route.action, &resource, upstreams, report);
}

fn check_action(action: &Action, resource: &str, upstreams: &BTreeSet<Uuid>, report: &mut Report) {
    match action {
        Action::Proxy { upstream_id } if !upstreams.contains(upstream_id) => {
            report.error(resource, format!("upstream {upstream_id} does not exist"));
        }
        Action::Static {
            root, index_files, ..
        } => {
            let relative = Path::new(root);
            if root.is_empty()
                || !relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
            {
                report.error(
                    resource,
                    "the static root must be a relative path without '.' or '..'",
                );
            }
            if index_files.iter().any(|name| {
                name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".."
            }) {
                report.error(resource, "index files must be plain file names");
            }
        }
        Action::Redirect {
            location, status, ..
        } => {
            if !REDIRECT_STATUSES.contains(status) {
                report.error(resource, format!("{status} is not a redirect status"));
            }
            let absolute = ["https://", "http://", "$scheme://", "${scheme}://"]
                .iter()
                .any(|prefix| location.starts_with(prefix));
            if !(absolute || location.starts_with('/')) || location.contains(char::is_whitespace) {
                report.error(
                    resource,
                    "the redirect target must be an absolute URL or path",
                );
            }
            if let Err(error) = parse_template(location) {
                report.error(resource, format!("the redirect target is invalid: {error}"));
            }
        }
        Action::Respond {
            status,
            content_type,
            body,
            ..
        } => {
            if !(200..=599).contains(status) {
                report.error(resource, format!("{status} is not a final response status"));
            }
            if content_type
                .as_deref()
                .is_some_and(|value| value.is_empty() || value.contains(char::is_control))
            {
                report.error(resource, "the content type is invalid");
            }
            if body.as_ref().is_some_and(|body| body.len() > 64 * 1024) {
                report.error(resource, "the response body exceeds 64 KiB");
            }
            if let Some(Err(error)) = body.as_deref().map(parse_template) {
                report.error(resource, format!("the response body is invalid: {error}"));
            }
        }
        Action::Proxy { .. } => {}
    }
}

fn check_text(
    report: &mut Report,
    resource: &str,
    field: &str,
    value: Option<&str>,
    limit: usize,
    required: bool,
) {
    match value {
        Some(value) if value.trim().is_empty() || value.len() > limit => report.error(
            resource,
            format!("{field} must contain 1..={limit} bytes of visible text"),
        ),
        Some(value) if value.contains(char::is_control) && field != "note" => report.error(
            resource,
            format!("{field} must not contain control characters"),
        ),
        None if required => report.error(resource, format!("{field} is required")),
        _ => {}
    }
}

/// Files of certificates of the inventory in the gateway's secret directory.
const DELIVERED_PREFIX: &str = "cert-";
/// The shortest max-age browsers' preload lists accept.
const PRELOAD_MAX_AGE: u64 = 31_536_000;

/// The protocol range, cipher suites and their fit for that range.
fn validate_tls_settings(profile: &crate::TlsProfile, resource: &str, report: &mut Report) {
    let index = |name: &str| PROTOCOLS.iter().position(|known| *known == name);
    let newest = PROTOCOLS.len() - 1;
    let max = match &profile.max_protocol {
        None => Some(newest),
        Some(name) => {
            let found = index(name);
            if found.is_none() {
                report.error(resource, "maximum protocol must be TLSv1.2 or TLSv1.3");
            }
            found
        }
    };
    let range = match (index(&profile.min_protocol), max) {
        (Some(min), Some(max)) if max < min => {
            report.error(
                resource,
                "the maximum TLS version is older than the minimum",
            );
            None
        }
        (Some(min), Some(max)) => Some(min..=max),
        _ => None,
    };
    let mut seen = BTreeSet::new();
    for suite in &profile.cipher_suites {
        if suite_version(suite).is_none() {
            report.error(resource, format!("unknown cipher suite {suite:?}"));
        } else if !seen.insert(suite.as_str()) {
            report.error(resource, format!("cipher suite {suite} is listed twice"));
        }
    }
    if profile.cipher_suites.is_empty() {
        return;
    }
    for version in range.into_iter().flatten() {
        let needed = if version == 0 {
            SuiteVersion::Tls12
        } else {
            SuiteVersion::Tls13
        };
        if !profile
            .cipher_suites
            .iter()
            .any(|suite| suite_version(suite) == Some(needed))
        {
            report.error(
                resource,
                format!("no listed cipher suite works with {}", PROTOCOLS[version]),
            );
        }
    }
}

pub(crate) fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Domain, Listener, Upstream, UpstreamNode};
    use chrono::Utc;
    use panel_domain::NormalizedHost;
    use panel_ir::{ListenerProtocols, LoadBalancingPolicy, StrictTransportSecurity, WwwRedirect};

    pub(crate) fn site(name: &str, hosts: &[&str], action: Action) -> Site {
        Site {
            id: Uuid::now_v7(),
            name: name.into(),
            action,
            enabled: true,
            domains: hosts
                .iter()
                .map(|host| Domain {
                    host: NormalizedHost::new(host).unwrap(),
                    enabled: true,
                    primary: false,
                    redirect: false,
                    tls_profile_id: None,
                })
                .collect(),
            routes: Vec::new(),
            listener_ids: BTreeSet::new(),
            https_redirect: false,
            www_redirect: WwwRedirect::None,
            tls_profile_id: None,
            hsts: None,
            group: None,
            tags: BTreeSet::new(),
            note: None,
            favorite: false,
            deleted_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_policy_id: Default::default(),
        }
    }

    fn upstream() -> Upstream {
        Upstream {
            id: Uuid::now_v7(),
            name: "app".into(),
            nodes: vec![UpstreamNode {
                id: Uuid::now_v7(),
                host: "127.0.0.1".into(),
                port: 8080,
                tls: false,
                weight: 1,
                enabled: true,
                backup: false,
                sni: None,
                unix_socket: None,
                note: None,
            }],
            balancing: LoadBalancingPolicy::RoundRobin,
            host_header: None,
            tls: Default::default(),
            connection: Default::default(),
            health_check: None,
            passive_health: None,
            note: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn messages(model: &ConfigModel) -> Vec<String> {
        validate(model)
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    #[test]
    fn a_consistent_model_has_no_diagnostics() {
        let upstream = upstream();
        let mut model = ConfigModel::default();
        model.sites.push(site(
            "shop",
            &["shop.example.com"],
            Action::Proxy {
                upstream_id: upstream.id,
            },
        ));
        model.upstreams.push(upstream);
        assert!(validate(&model).is_empty(), "{:?}", messages(&model));
    }

    fn profile(id: &str, certificate_id: Option<&str>, files: (&str, &str)) -> crate::TlsProfile {
        crate::TlsProfile {
            id: id.into(),
            certificate_id: certificate_id.map(|id| panel_domain::CertificateId::new(id).unwrap()),
            certificate_secret_id: files.0.into(),
            private_key_secret_id: files.1.into(),
            min_protocol: "TLSv1.2".into(),
            max_protocol: None,
            cipher_suites: Vec::new(),
            session_resumption: true,
            ocsp_stapling: false,
            alpn: BTreeSet::new(),
        }
    }

    #[test]
    fn tls_profiles_name_an_inventory_certificate_or_files() {
        let mut model = ConfigModel {
            tls_profiles: vec![
                profile("managed", Some("example.com"), ("", "")),
                profile("files", None, ("site.pem", "site.key")),
            ],
            ..ConfigModel::default()
        };
        assert!(validate(&model).is_empty(), "{:?}", messages(&model));
        let compiled: Vec<_> = model
            .tls_profiles
            .iter()
            .map(crate::TlsProfile::runtime)
            .map(|profile| (profile.certificate_secret_id, profile.private_key_secret_id))
            .collect();
        assert_eq!(
            compiled,
            [
                (
                    "cert-example.com.pem".to_owned(),
                    "cert-example.com.key".to_owned()
                ),
                ("site.pem".into(), "site.key".into()),
            ]
        );

        model.tls_profiles = vec![
            profile("both", Some("example.com"), ("site.pem", "site.key")),
            profile("taken", None, ("cert-example.com.pem", "site.key")),
            profile("empty", None, ("", "site.key")),
        ];
        let found = messages(&model);
        for expected in [
            "not both",
            "is a file delivered for a certificate of the inventory",
            "certificate secret \"\" must be a plain file name",
        ] {
            assert!(
                found.iter().any(|message| message.contains(expected)),
                "{expected}: {found:?}"
            );
        }
    }

    #[test]
    fn tls_settings_fit_their_protocol_range() {
        let mut capped = profile("capped", None, ("site.pem", "site.key"));
        capped.max_protocol = Some("TLSv1.2".into());
        capped.cipher_suites = vec!["TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256".into()];
        let model = ConfigModel {
            tls_profiles: vec![capped.clone()],
            ..ConfigModel::default()
        };
        assert!(validate(&model).is_empty(), "{:?}", messages(&model));

        let mut inverted = capped.clone();
        inverted.id = "inverted".into();
        inverted.min_protocol = "TLSv1.3".into();
        let mut unknown = capped;
        unknown.id = "unknown".into();
        unknown.max_protocol = None;
        unknown.cipher_suites = vec![
            "RC4_MD5".into(),
            "TLS13_AES_128_GCM_SHA256".into(),
            "TLS13_AES_128_GCM_SHA256".into(),
        ];
        let model = ConfigModel {
            tls_profiles: vec![inverted, unknown],
            ..ConfigModel::default()
        };
        let found = messages(&model);
        for expected in [
            "older than the minimum",
            "unknown cipher suite \"RC4_MD5\"",
            "is listed twice",
            "no listed cipher suite works with TLSv1.2",
        ] {
            assert!(
                found.iter().any(|message| message.contains(expected)),
                "{expected}: {found:?}"
            );
        }
    }

    #[test]
    fn hsts_preloading_needs_subdomains_and_a_year() {
        let upstream = upstream();
        let mut shop = site(
            "shop",
            &["shop.example.com"],
            Action::Proxy {
                upstream_id: upstream.id,
            },
        );
        shop.hsts = Some(StrictTransportSecurity {
            max_age_seconds: 300,
            include_subdomains: false,
            preload: true,
        });
        let mut model = ConfigModel {
            sites: vec![shop],
            upstreams: vec![upstream],
            ..ConfigModel::default()
        };
        assert!(messages(&model)
            .iter()
            .any(|message| message.contains("HSTS preloading")));
        model.sites[0].hsts = Some(StrictTransportSecurity {
            max_age_seconds: 31_536_000,
            include_subdomains: true,
            preload: true,
        });
        assert!(validate(&model).is_empty(), "{:?}", messages(&model));
    }

    #[test]
    fn duplicate_names_domains_and_dangling_references_are_reported() {
        let missing = Uuid::now_v7();
        let mut model = ConfigModel::default();
        model.sites.push(site(
            "Shop",
            &["example.com"],
            Action::Proxy {
                upstream_id: missing,
            },
        ));
        model.sites.push(site(
            "shop",
            &["EXAMPLE.com"],
            Action::Redirect {
                location: "example.org".into(),
                status: 200,
                preserve_path: false,
            },
        ));
        model.listeners.push(Listener {
            id: "bad id".into(),
            address: "localhost:80".into(),
            tls_profile_id: Some("missing".into()),
            protocols: ListenerProtocols {
                http1: false,
                http2: false,
                http3: false,
            },
            reuse_port: false,
            ipv6_only: None,
            default_site_id: Some(Uuid::now_v7()),
            real_ip_header: Default::default(),
            trusted_proxies: Default::default(),
        });
        let found = messages(&model);
        for expected in [
            "site name \"shop\" is already used",
            "domain example.com is already bound to site \"Shop\"",
            "does not exist",
            "200 is not a redirect status",
            "absolute URL or path",
            "listener id \"bad id\" is invalid",
            "is not an IP address and port",
            "must accept HTTP/1.1 or HTTP/2",
            "TLS profile missing does not exist",
        ] {
            assert!(
                found.iter().any(|message| message.contains(expected)),
                "{expected}: {found:?}"
            );
        }
    }

    #[test]
    fn deleted_sites_release_their_names_and_domains() {
        let mut model = ConfigModel::default();
        let mut old = site(
            "shop",
            &["example.com"],
            Action::Respond {
                status: 503,
                body: None,
                content_type: None,
                retry_after_seconds: None,
            },
        );
        old.deleted_at = Some(Utc::now());
        model.sites.push(old);
        model.sites.push(site(
            "shop",
            &["example.com"],
            Action::Respond {
                status: 503,
                body: None,
                content_type: None,
                retry_after_seconds: None,
            },
        ));
        assert!(validate(&model).is_empty(), "{:?}", messages(&model));
    }

    #[test]
    fn domain_roles_and_route_matchers_are_checked() {
        let mut model = ConfigModel::default();
        let mut shop = site(
            "shop",
            &["*.example.com", "alias.example.net"],
            Action::Respond {
                status: 503,
                body: None,
                content_type: None,
                retry_after_seconds: None,
            },
        );
        shop.domains[0].primary = true;
        shop.domains[1].redirect = true;
        shop.routes.push(Route {
            id: Uuid::now_v7(),
            name: None,
            enabled: true,
            priority: 1,
            matcher: crate::model::RouteMatch {
                kind: MatchKind::Exact,
                path: "relative".into(),
                host: Some(NormalizedHost::new("other.example").unwrap()),
            },
            action: Action::Static {
                root: "../escape".into(),
                index_files: vec!["index.html".into()],
                spa_fallback: false,
            },
            security_policy_id: Default::default(),
        });
        model.sites.push(shop);
        let found = messages(&model);
        for expected in [
            "primary domain must be an enabled, concrete name",
            "is not a valid Exact match",
            "host restriction needs a prefix match",
            "host other.example is not one of the site's domains",
            "static root must be a relative path",
        ] {
            assert!(
                found.iter().any(|message| message.contains(expected)),
                "{expected}: {found:?}"
            );
        }
    }
}
