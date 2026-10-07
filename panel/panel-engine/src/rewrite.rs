//! Checks of rewrites and internal redirects (ADR 0040): prefixes are
//! normalized absolute paths, templates and replacements parse into paths,
//! patterns compile within the route limit with the groups their
//! replacements name, named targets exist in the site, rule lists stay
//! bounded, and snapshots using any of it require the capability.

use crate::{route_regex_error, ROUTE_REGEX_SIZE_LIMIT};
use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::{
    rewrite::{parse_replacement, redirects, ReplacementPart},
    template::parse_template,
    RewriteRule, RouteAction, RouteMatcher, RuntimeSnapshot, MOST_REWRITE_RULES,
    REWRITE_CAPABILITY,
};
use std::collections::{BTreeMap, BTreeSet};

const MOST_PATTERN_BYTES: usize = 1024;
const MOST_TEMPLATE_BYTES: usize = 4096;
const MOST_PREFIX_BYTES: usize = 1024;

pub(crate) fn validate_rewrites(snapshot: &RuntimeSnapshot, diagnostics: &mut Vec<Diagnostic>) {
    let mut report = |resource: &str, message: String| {
        diagnostics
            .push(Diagnostic::error(ErrorCode::VALIDATION_FAILED, message).with_resource(resource));
    };
    let mut used = false;
    for site in &snapshot.sites {
        used |= !site.rewrites.is_empty();
        for problem in problems(&site.rewrites) {
            report(site.id.as_str(), format!("site {} {problem}", site.id));
        }
    }
    let mut named: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for route in &snapshot.routes {
        if let RouteMatcher::Named { name } = &route.matcher {
            named
                .entry(route.site_id.as_str())
                .or_default()
                .insert(name.as_str());
        }
    }
    for route in &snapshot.routes {
        used |= !route.rewrites.is_empty() || route.internal;
        for problem in problems(&route.rewrites) {
            report(route.id.as_str(), format!("route {} {problem}", route.id));
        }
        if let RouteAction::InternalRedirect { target } = &route.action {
            used = true;
            let names = named.get(route.site_id.as_str());
            if let Some(problem) = target_problem(target, |name| {
                names.is_some_and(|names| names.contains(name))
            }) {
                report(route.id.as_str(), format!("route {} {problem}", route.id));
            }
        }
    }
    let declared = snapshot
        .required_capabilities()
        .iter()
        .any(|capability| capability.name == REWRITE_CAPABILITY);
    if used && !declared {
        report(
            "routes",
            format!("rewrites are used without requiring {REWRITE_CAPABILITY}"),
        );
    }
}

/// What is wrong with `rules`, one line each.
pub fn problems(rules: &[RewriteRule]) -> Vec<String> {
    let mut found = Vec::new();
    if rules.len() > MOST_REWRITE_RULES {
        found.push(format!(
            "has {} rewrite rules, more than {MOST_REWRITE_RULES}",
            rules.len()
        ));
    }
    for rule in rules {
        match rule {
            RewriteRule::StripPrefix { prefix } | RewriteRule::AddPrefix { prefix } => {
                if let Some(problem) = prefix_problem(prefix) {
                    found.push(problem);
                }
            }
            RewriteRule::SetUri { template } => {
                if let Some(problem) = path_template_problem(template) {
                    found.push(format!("sets a URI that {problem}"));
                }
            }
            RewriteRule::Rewrite {
                pattern,
                replacement,
                flag,
            } => {
                if let Some(problem) = rewrite_problem(pattern, replacement, flag.redirect_status())
                {
                    found.push(problem);
                }
            }
        }
    }
    found
}

/// What is wrong with an internal redirect's `target`, a path template or
/// `@name`, whose route `named` tells exists.
pub fn target_problem(target: &str, named: impl Fn(&str) -> bool) -> Option<String> {
    match target.strip_prefix('@') {
        Some("") => Some("redirects internally to an unnamed route".into()),
        Some(name) if !named(name) => Some(format!(
            "redirects internally to @{name}, which no route of its site is named"
        )),
        Some(_) => None,
        None => path_template_problem(target)
            .map(|problem| format!("redirects internally to a target that {problem}")),
    }
}

fn prefix_problem(prefix: &str) -> Option<String> {
    let problem = if prefix.len() > MOST_PREFIX_BYTES {
        format!("is longer than {MOST_PREFIX_BYTES} bytes")
    } else if !prefix.starts_with('/') || prefix == "/" || prefix.ends_with('/') {
        "is not an absolute path of one or more segments without a trailing /".into()
    } else if prefix.bytes().any(|byte| {
        byte.is_ascii_control() || byte.is_ascii_whitespace() || matches!(byte, b'?' | b'#')
    }) {
        "holds a character a path segment may not".into()
    } else if normalized_prefix(prefix) != prefix {
        "is not normalized: write it without dot segments, empty segments or needless percent-encodings".into()
    } else {
        return None;
    };
    Some(format!("has a rewrite prefix {prefix:?} that {problem}"))
}

/// `prefix` as route matchers see paths, or empty when it has empty or dot
/// segments or percent-encodings that normalization would change.
fn normalized_prefix(prefix: &str) -> String {
    let mut normal = String::with_capacity(prefix.len());
    for segment in prefix.split('/').skip(1) {
        if segment.is_empty() || segment == "." || segment == ".." {
            return String::new();
        }
        normal.push('/');
        normal.push_str(segment);
    }
    if normal.contains('%') {
        let bytes = normal.as_bytes();
        let mut index = 0;
        while let Some(offset) = normal[index..].find('%') {
            let at = index + offset;
            let byte = bytes
                .get(at + 1..at + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok());
            match byte {
                Some(byte)
                    if !(byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
                        && bytes[at + 1..at + 3]
                            .iter()
                            .all(|digit| !digit.is_ascii_lowercase()) => {}
                _ => return String::new(),
            }
            index = at + 3;
        }
    }
    normal
}

fn path_template_problem(template: &str) -> Option<String> {
    if template.is_empty() || template.len() > MOST_TEMPLATE_BYTES {
        return Some(format!("is not 1..={MOST_TEMPLATE_BYTES} bytes"));
    }
    if let Err(error) = parse_template(template) {
        return Some(format!("is not a template: {error}"));
    }
    if !(template.starts_with('/') || template.starts_with('$')) {
        return Some("is not an absolute path".into());
    }
    None
}

fn rewrite_problem(pattern: &str, replacement: &str, status: Option<u16>) -> Option<String> {
    if pattern.is_empty() || pattern.len() > MOST_PATTERN_BYTES {
        return Some(format!(
            "has a rewrite pattern of {} bytes; it needs 1..={MOST_PATTERN_BYTES}",
            pattern.len()
        ));
    }
    if let Some(error) = route_regex_error(pattern) {
        return Some(format!(
            "has a rewrite pattern that does not compile: {error}"
        ));
    }
    let compiled = regex::RegexBuilder::new(pattern)
        .size_limit(ROUTE_REGEX_SIZE_LIMIT)
        .dfa_size_limit(ROUTE_REGEX_SIZE_LIMIT)
        .build()
        .ok()?;
    if replacement.is_empty() || replacement.len() > MOST_TEMPLATE_BYTES {
        return Some(format!(
            "has a rewrite replacement of {} bytes; it needs 1..={MOST_TEMPLATE_BYTES}",
            replacement.len()
        ));
    }
    let groups: Vec<&str> = compiled.capture_names().flatten().collect();
    let parts = match parse_replacement(replacement, &groups) {
        Ok(parts) => parts,
        Err(error) => {
            return Some(format!(
                "has a rewrite replacement that does not parse: {error}"
            ))
        }
    };
    if let Some(missing) = parts.iter().find_map(|part| match part {
        ReplacementPart::Capture(group) if *group >= compiled.captures_len() => Some(*group),
        _ => None,
    }) {
        return Some(format!(
            "has a rewrite replacement that takes ${missing}, which its pattern does not capture"
        ));
    }
    if !(redirects(replacement) || replacement.starts_with('/') || replacement.starts_with('$')) {
        let what = if status.is_some() {
            "redirect"
        } else {
            "rewrite"
        };
        return Some(format!(
            "has a {what} replacement that is neither an absolute path nor an http or https URL"
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_ir::RewriteFlag;

    fn rewrite(pattern: &str, replacement: &str) -> RewriteRule {
        RewriteRule::Rewrite {
            pattern: pattern.into(),
            replacement: replacement.into(),
            flag: RewriteFlag::None,
        }
    }

    #[test]
    fn sound_rules_pass() {
        let rules = [
            RewriteRule::StripPrefix {
                prefix: "/api".into(),
            },
            RewriteRule::AddPrefix {
                prefix: "/v2/%E2%82%AC".into(),
            },
            RewriteRule::SetUri {
                template: "/index.php?q=$uri&$args".into(),
            },
            rewrite("^/u/(?<id>\\d+)/(.*)$", "/users/$id/$2?from=$host"),
            rewrite("^/old$", "https://shop.example/new"),
            rewrite("^/legacy(.*)$", "$scheme://$host/new$1"),
        ];
        assert_eq!(problems(&rules), Vec::<String>::new());
    }

    #[test]
    fn broken_rules_are_told() {
        let found = problems(&[
            RewriteRule::StripPrefix {
                prefix: "api".into(),
            },
            RewriteRule::StripPrefix {
                prefix: "/api/".into(),
            },
            RewriteRule::AddPrefix { prefix: "/".into() },
            RewriteRule::AddPrefix {
                prefix: "/a/../b".into(),
            },
            RewriteRule::AddPrefix {
                prefix: "/%7e".into(),
            },
            RewriteRule::AddPrefix {
                prefix: "/a?b".into(),
            },
            RewriteRule::SetUri {
                template: "index.php".into(),
            },
            RewriteRule::SetUri {
                template: "/$nope".into(),
            },
            rewrite("(a", "/b"),
            rewrite("^(?<=a)b$", "/b"),
            rewrite("^/(a)$", "/$2"),
            rewrite("^/a$", "relative"),
            rewrite("^/a$", ""),
        ]);
        assert_eq!(found.len(), 13, "{found:#?}");
        assert!(found[0].contains("not an absolute path"), "{}", found[0]);
        assert!(found[3].contains("not normalized"), "{}", found[3]);
        assert!(found[4].contains("not normalized"), "{}", found[4]);
        assert!(found[5].contains("may not"), "{}", found[5]);
        assert!(found[7].contains("$nope"), "{}", found[7]);
        assert!(found[8].contains("does not compile"), "{}", found[8]);
        assert!(found[9].contains("does not compile"), "{}", found[9]);
        assert!(found[10].contains("takes $2"), "{}", found[10]);
        assert!(
            found[11].contains("neither an absolute path"),
            "{}",
            found[11]
        );
        let many = vec![
            RewriteRule::AddPrefix {
                prefix: "/a".into()
            };
            MOST_REWRITE_RULES + 1
        ];
        assert!(problems(&many)[0].contains("more than 64"));
    }

    #[test]
    fn snapshots_with_rewrites_require_the_capability() {
        use panel_domain::{NormalizedHost, PathPrefix, RevisionId, RouteId, SiteId};
        use panel_ir::{CapabilityRequirement, DomainSpec, RouteSpec, SiteSpec};

        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites.push(SiteSpec::new(
            SiteId::new("site").unwrap(),
            "site",
            vec![DomainSpec::new(NormalizedHost::new("example.com").unwrap())],
        ));
        let mut route = RouteSpec::new(
            RouteId::new("route").unwrap(),
            SiteId::new("site").unwrap(),
            1,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new("/").unwrap(),
            },
            RouteAction::InternalRedirect {
                target: "@missing".into(),
            },
        );
        route.internal = true;
        snapshot.routes.push(route);
        let mut diagnostics = Vec::new();
        validate_rewrites(&snapshot, &mut diagnostics);
        let messages: Vec<_> = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect();
        assert!(
            messages.iter().any(|message| message.contains("@missing")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("without requiring route.rewrite")),
            "{messages:?}"
        );

        snapshot
            .required_capabilities
            .push(CapabilityRequirement::new(REWRITE_CAPABILITY, "1"));
        snapshot.routes[0].action = RouteAction::respond(204, None);
        let mut diagnostics = Vec::new();
        validate_rewrites(&snapshot, &mut diagnostics);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn internal_targets_are_paths_or_named_routes_of_the_site() {
        let named = |name: &str| name == "fallback";
        assert_eq!(target_problem("@fallback", named), None);
        assert_eq!(target_problem("/errors$uri", named), None);
        assert!(target_problem("@missing", named)
            .unwrap()
            .contains("@missing"));
        assert!(target_problem("@", named).unwrap().contains("unnamed"));
        assert!(target_problem("errors", named)
            .unwrap()
            .contains("not an absolute path"));
    }
}
