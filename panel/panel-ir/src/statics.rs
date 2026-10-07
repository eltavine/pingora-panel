//! Directory listings, media types and cache headers of static content
//! (ADR 0042).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Required by snapshots whose static content lists directories.
pub const LISTING_CAPABILITY: &str = "static.listing";
/// Required by snapshots whose static content maps media types.
pub const MEDIA_TYPES_CAPABILITY: &str = "static.media-types";
/// Required by snapshots whose static content sets `Cache-Control`.
pub const STATIC_CACHE_CAPABILITY: &str = "static.cache-control";

/// The most entries a listing shows.
pub const MOST_LISTED: usize = 10_000;
/// The most extensions static content maps.
pub const MOST_MEDIA_TYPES: usize = 256;
/// The most cache rules static content lists.
pub const MOST_CACHE_RULES: usize = 32;
/// The longest `max-age` caches are bound to read (RFC 9111 §1.2.2).
pub const MOST_MAX_AGE_SECONDS: u64 = 1 << 31;

fn is_false(value: &bool) -> bool {
    !*value
}

/// How a directory without an index file is answered.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryListing {
    /// 404, as without listings.
    #[default]
    Off,
    Html,
    /// Entries as nginx's `autoindex_format json` writes them.
    Json,
}

impl DirectoryListing {
    pub fn is_off(&self) -> bool {
        *self == Self::Off
    }
}

/// A rule setting `Cache-Control` on the files with its extensions.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticCacheRule {
    /// Lowercase extensions without their dot; none for every file.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub extensions: BTreeSet<String>,
    /// How long caches may reuse a file (`max-age`); without it they
    /// revalidate it first (`no-cache`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_seconds: Option<u64>,
    /// The file never changes at its URL (RFC 8246).
    #[serde(default, skip_serializing_if = "is_false")]
    pub immutable: bool,
}

impl StaticCacheRule {
    /// The `Cache-Control` value the rule sends.
    pub fn header_value(&self) -> String {
        match (self.max_age_seconds, self.immutable) {
            (Some(seconds), true) => format!("max-age={seconds}, immutable"),
            (Some(seconds), false) => format!("max-age={seconds}"),
            (None, _) => "no-cache".to_owned(),
        }
    }

    pub fn applies_to_every_file(&self) -> bool {
        self.extensions.is_empty()
    }
}

/// The rule for a file with `extension`, lowercase: the first naming it,
/// then the first for every file.
pub fn cache_rule<'a>(
    rules: &'a [StaticCacheRule],
    extension: Option<&str>,
) -> Option<&'a StaticCacheRule> {
    extension
        .and_then(|extension| {
            rules
                .iter()
                .find(|rule| rule.extensions.contains(extension))
        })
        .or_else(|| rules.iter().find(|rule| rule.applies_to_every_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_rule_naming_an_extension_comes_before_the_rule_for_every_file() {
        let rules = [
            rule(&[], Some(60), false),
            rule(&["css", "js"], Some(31_536_000), true),
            rule(&["html"], None, false),
        ];
        assert_eq!(
            cache_rule(&rules, Some("js")).map(StaticCacheRule::header_value),
            Some("max-age=31536000, immutable".into())
        );
        assert_eq!(
            cache_rule(&rules, Some("html")).map(StaticCacheRule::header_value),
            Some("no-cache".into())
        );
        assert_eq!(
            cache_rule(&rules, Some("png")).map(StaticCacheRule::header_value),
            Some("max-age=60".into())
        );
        assert_eq!(
            cache_rule(&rules, None).map(StaticCacheRule::header_value),
            Some("max-age=60".into())
        );
        assert!(cache_rule(&rules[1..], Some("png")).is_none());
    }

    #[test]
    fn settings_left_out_read_as_none() {
        let rule: StaticCacheRule = serde_json::from_str("{}").unwrap();
        assert_eq!(rule.header_value(), "no-cache");
        assert_eq!(serde_json::to_string(&rule).unwrap(), "{}");
        let listing: DirectoryListing = serde_json::from_str("\"json\"").unwrap();
        assert_eq!(listing, DirectoryListing::Json);
        assert!(DirectoryListing::default().is_off());
    }
}
