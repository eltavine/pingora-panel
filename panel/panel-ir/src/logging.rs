//! What the gateway writes to its access logs and how it keeps its log
//! files (ADR 0025).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Required by snapshots that configure logging.
pub const LOGGING_CAPABILITY: &str = "log.access";

/// Query parameters whose values are logged as [`REDACTED`] unless a policy
/// names its own: OpenTelemetry's defaults for `url.query`, matched by case.
pub const DEFAULT_REDACTED_QUERY: &[&str] = &[
    "X-Amz-Signature",
    "X-Amz-Credential",
    "X-Amz-Security-Token",
    "sig",
    "X-Goog-Signature",
];

/// Headers always logged as [`REDACTED`]: credentials and cookies.
pub const DEFAULT_REDACTED_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "proxy-authorization",
    "set-cookie",
];

/// What a redacted query value or header is logged as.
pub const REDACTED: &str = "REDACTED";

/// Prefixes of the names records already use; extra fields may not take them.
pub const RESERVED_FIELD_PREFIXES: &[&str] = &[
    "client.",
    "error.",
    "event.",
    "http.",
    "network.",
    "pingora_panel.",
    "server.",
    "url.",
    "user_agent.",
];

/// Names records already use outside the reserved prefixes.
pub const RESERVED_FIELD_NAMES: &[&str] = &[
    "message",
    "severity_text",
    "span_id",
    "timestamp",
    "trace_id",
];

/// The longest name of an extra field.
pub const MAX_FIELD_NAME: usize = 64;

/// Whether `name` may name an extra field: lowercase words of letters,
/// digits and `_` joined by dots, not taken by the record itself.
pub fn is_field_name(name: &str) -> bool {
    name.len() <= MAX_FIELD_NAME
        && name.split('.').all(|word| {
            word.bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase())
                && word
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
        && !RESERVED_FIELD_NAMES.contains(&name)
        && !RESERVED_FIELD_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessLogFormat {
    /// One JSON object per line, keyed by OpenTelemetry attribute names.
    #[default]
    Json,
    /// The Combined Log Format that log analysers read; extra fields are
    /// left out.
    Combined,
}

/// Access log settings of every site, of one site or of one route. Unset
/// values come from the enclosing scope.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessLog {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<AccessLogFormat>,
    /// Extra fields by name, as templates of request variables; they add to
    /// the enclosing scope's and replace those of the same name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

impl AccessLog {
    pub fn is_unset(&self) -> bool {
        *self == Self::default()
    }
}

/// How the gateway rotates and keeps each log file.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogFiles {
    /// A file rotates before it grows past this many bytes.
    pub max_size_bytes: u64,
    /// Whether files also rotate at midnight UTC.
    pub rotate_daily: bool,
    /// Rotated files older than this many days are deleted; zero keeps them.
    pub keep_days: u32,
    /// The most rotated files kept of each log; zero keeps them all.
    pub max_files: u32,
}

impl Default for LogFiles {
    fn default() -> Self {
        Self {
            max_size_bytes: 100 * 1024 * 1024,
            rotate_daily: true,
            keep_days: 7,
            max_files: 30,
        }
    }
}

impl LogFiles {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Logging for every site, and what is never logged.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingPolicy {
    #[serde(default, skip_serializing_if = "AccessLog::is_unset")]
    pub access: AccessLog,
    #[serde(default, skip_serializing_if = "LogFiles::is_default")]
    pub files: LogFiles,
    /// Query parameters logged as [`REDACTED`], replacing
    /// [`DEFAULT_REDACTED_QUERY`] when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redact_query: Option<BTreeSet<String>>,
    /// Lowercase header names logged as [`REDACTED`] besides
    /// [`DEFAULT_REDACTED_HEADERS`].
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub redact_headers: BTreeSet<String>,
}

impl LoggingPolicy {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The query parameters whose values are redacted.
    pub fn redacted_query(&self) -> BTreeSet<String> {
        self.redact_query.clone().unwrap_or_else(|| {
            DEFAULT_REDACTED_QUERY
                .iter()
                .map(|key| (*key).to_owned())
                .collect()
        })
    }

    /// The headers whose values are redacted, lowercase.
    pub fn redacted_headers(&self) -> BTreeSet<String> {
        DEFAULT_REDACTED_HEADERS
            .iter()
            .map(|name| (*name).to_owned())
            .chain(self.redact_headers.iter().cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_fields_keep_clear_of_the_record_s_own_names() {
        assert!(is_field_name("tenant"));
        assert!(is_field_name("app.tenant_id"));
        assert!(!is_field_name(""));
        assert!(!is_field_name("Tenant"));
        assert!(!is_field_name("1tenant"));
        assert!(!is_field_name("app..tenant"));
        assert!(!is_field_name("http.request.method"));
        assert!(!is_field_name("pingora_panel.site.id"));
        assert!(!is_field_name("trace_id"));
        assert!(!is_field_name(&"a".repeat(MAX_FIELD_NAME + 1)));
    }

    #[test]
    fn a_policy_replaces_the_query_list_and_adds_to_the_header_list() {
        let default = LoggingPolicy::default();
        assert!(default.redacted_query().contains("sig"));
        assert!(default.redacted_headers().contains("authorization"));

        let policy = LoggingPolicy {
            redact_query: Some(BTreeSet::from(["token".to_owned()])),
            redact_headers: BTreeSet::from(["x-api-key".to_owned()]),
            ..LoggingPolicy::default()
        };
        assert_eq!(
            policy.redacted_query(),
            BTreeSet::from(["token".to_owned()])
        );
        assert!(policy.redacted_headers().contains("x-api-key"));
        assert!(policy.redacted_headers().contains("cookie"));
    }

    #[test]
    fn snapshots_without_logging_keep_their_canonical_bytes() {
        use crate::{RuntimeSnapshot, SiteSpec};
        use panel_domain::{RevisionId, SiteId};

        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites.push(SiteSpec::new(
            SiteId::new("site").unwrap(),
            "site",
            Vec::new(),
        ));
        let plain = snapshot.content_hash();
        let canonical = String::from_utf8(snapshot.canonical_bytes()).unwrap();
        assert!(!canonical.contains("logging"), "{canonical}");
        assert!(!canonical.contains("access_log"), "{canonical}");

        snapshot.sites[0].access_log.enabled = Some(false);
        let site_off = snapshot.content_hash();
        assert_ne!(site_off, plain);
        snapshot.logging.files.keep_days = 30;
        assert_ne!(snapshot.content_hash(), site_off);
    }

    #[test]
    fn defaults_are_left_out_of_the_serialized_policy() {
        assert_eq!(
            serde_json::to_string(&LoggingPolicy::default()).unwrap(),
            "{}"
        );
        assert_eq!(serde_json::to_string(&AccessLog::default()).unwrap(), "{}");
    }
}
