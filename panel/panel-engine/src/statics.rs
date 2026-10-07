//! Checks of directory listings, media types and cache rules of static
//! content (ADR 0042), and that snapshots using them require their
//! capabilities.

use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::{
    RuntimeSnapshot, StaticContentPolicy, LISTING_CAPABILITY, MEDIA_TYPES_CAPABILITY,
    MOST_CACHE_RULES, MOST_MAX_AGE_SECONDS, MOST_MEDIA_TYPES, STATIC_CACHE_CAPABILITY,
};
use std::collections::BTreeSet;

const MOST_EXTENSION_BYTES: usize = 32;

pub(crate) fn validate_statics(snapshot: &RuntimeSnapshot, diagnostics: &mut Vec<Diagnostic>) {
    let mut report = |resource: &str, message: String| {
        diagnostics
            .push(Diagnostic::error(ErrorCode::VALIDATION_FAILED, message).with_resource(resource));
    };
    let declared = |name: &str| {
        snapshot
            .required_capabilities()
            .iter()
            .any(|capability| capability.name == name)
    };
    for policy in &snapshot.static_content {
        for problem in static_problems(policy) {
            report(
                &policy.id,
                format!("static content {} {problem}", policy.id),
            );
        }
        for (used, capability, what) in [
            (
                !policy.listing.is_off(),
                LISTING_CAPABILITY,
                "lists directories",
            ),
            (
                !policy.media_types.is_empty() || policy.default_type.is_some(),
                MEDIA_TYPES_CAPABILITY,
                "maps media types",
            ),
            (
                !policy.cache.is_empty(),
                STATIC_CACHE_CAPABILITY,
                "sets Cache-Control",
            ),
        ] {
            if used && !declared(capability) {
                report(
                    &policy.id,
                    format!(
                        "static content {} {what} without requiring {capability}",
                        policy.id
                    ),
                );
            }
        }
    }
}

/// What is wrong with the listings, media types and cache rules of
/// `policy`, one line each.
pub fn static_problems(policy: &StaticContentPolicy) -> Vec<String> {
    let mut found = Vec::new();
    if policy.media_types.len() > MOST_MEDIA_TYPES {
        found.push(format!(
            "maps {} extensions, more than {MOST_MEDIA_TYPES}",
            policy.media_types.len()
        ));
    }
    for (extension, media_type) in &policy.media_types {
        if !is_extension(extension) {
            found.push(format!(
                "maps {extension:?}, which is not {EXTENSION_SHAPE}"
            ));
        }
        if let Some(problem) = crate::pages::media_type_problem(media_type) {
            found.push(format!("maps {extension} to a {problem}"));
        }
    }
    if let Some(problem) = policy
        .default_type
        .as_deref()
        .and_then(crate::pages::media_type_problem)
    {
        found.push(format!("has a default {problem}"));
    }
    if policy.cache.len() > MOST_CACHE_RULES {
        found.push(format!(
            "has {} cache rules, more than {MOST_CACHE_RULES}",
            policy.cache.len()
        ));
    }
    let (mut named, mut every_file) = (BTreeSet::new(), false);
    for (position, rule) in policy.cache.iter().enumerate() {
        let position = position + 1;
        for extension in &rule.extensions {
            if !is_extension(extension) {
                found.push(format!(
                    "has cache rule {position} for {extension:?}, which is not {EXTENSION_SHAPE}"
                ));
            } else if !named.insert(extension.as_str()) {
                found.push(format!(
                    "has cache rule {position} for {extension}, which an earlier rule takes"
                ));
            }
        }
        if rule.extensions.is_empty() {
            if every_file {
                found.push(format!(
                    "has cache rule {position} for every file after another, so it never applies"
                ));
            }
            every_file = true;
        }
        match rule.max_age_seconds {
            Some(seconds) if seconds > MOST_MAX_AGE_SECONDS => found.push(format!(
                "has cache rule {position} with a max-age of {seconds} seconds, more than {MOST_MAX_AGE_SECONDS}"
            )),
            None if rule.immutable => found.push(format!(
                "has cache rule {position} marking files immutable without a max-age"
            )),
            _ => {}
        }
    }
    found
}

const EXTENSION_SHAPE: &str =
    "an extension of lowercase letters, digits, '+', '-' and '_' without its dot";

fn is_extension(extension: &str) -> bool {
    !extension.is_empty()
        && extension.len() <= MOST_EXTENSION_BYTES
        && extension.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_ir::{DirectoryListing, StaticCacheRule};

    fn rule(extensions: &[&str], max_age_seconds: Option<u64>, immutable: bool) -> StaticCacheRule {
        StaticCacheRule {
            extensions: extensions
                .iter()
                .map(|extension| (*extension).to_owned())
                .collect(),
            max_age_seconds,
            immutable,
        }
    }

    #[test]
    fn sound_settings_pass() {
        let policy = StaticContentPolicy {
            id: "shop".into(),
            listing: DirectoryListing::Html,
            media_types: [
                ("wasm".to_owned(), "application/wasm".to_owned()),
                (
                    "webmanifest".to_owned(),
                    "application/manifest+json".to_owned(),
                ),
            ]
            .into(),
            default_type: Some("text/plain; charset=utf-8".into()),
            cache: vec![
                rule(&["css", "js", "woff2"], Some(31_536_000), true),
                rule(&["html"], None, false),
                rule(&[], Some(3600), false),
            ],
            ..StaticContentPolicy::default()
        };
        assert_eq!(static_problems(&policy), Vec::<String>::new());
    }

    #[test]
    fn broken_settings_are_told() {
        let policy = StaticContentPolicy {
            id: "shop".into(),
            media_types: [
                (".css".to_owned(), "text/css".to_owned()),
                ("svg".to_owned(), "image".to_owned()),
            ]
            .into(),
            default_type: Some("binary".into()),
            cache: vec![
                rule(&["CSS"], Some(60), false),
                rule(&["js"], Some(60), false),
                rule(&["js"], Some(60), false),
                rule(&[], Some(MOST_MAX_AGE_SECONDS + 1), false),
                rule(&[], None, true),
            ],
            ..StaticContentPolicy::default()
        };
        let found = static_problems(&policy);
        for expected in [
            "maps \".css\", which is not an extension",
            "maps svg to a media type \"image\"",
            "has a default media type \"binary\"",
            "cache rule 1 for \"CSS\"",
            "cache rule 3 for js, which an earlier rule takes",
            "cache rule 4 with a max-age of 2147483649 seconds",
            "cache rule 5 for every file after another",
            "cache rule 5 marking files immutable without a max-age",
        ] {
            assert!(
                found.iter().any(|line| line.contains(expected)),
                "{expected:?} in {found:#?}"
            );
        }
    }

    #[test]
    fn snapshots_using_the_settings_require_their_capabilities() {
        let mut snapshot = RuntimeSnapshot::empty(panel_domain::RevisionId::new(1));
        snapshot.static_content.push(StaticContentPolicy {
            id: "shop".into(),
            listing: DirectoryListing::Json,
            default_type: Some("text/plain".into()),
            cache: vec![rule(&[], None, false)],
            ..StaticContentPolicy::default()
        });
        let mut diagnostics = Vec::new();
        validate_statics(&snapshot, &mut diagnostics);
        let messages: Vec<_> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        for capability in [
            LISTING_CAPABILITY,
            MEDIA_TYPES_CAPABILITY,
            STATIC_CACHE_CAPABILITY,
        ] {
            assert!(
                messages.iter().any(|message| message.ends_with(capability)),
                "{capability} in {messages:?}"
            );
        }
    }
}
