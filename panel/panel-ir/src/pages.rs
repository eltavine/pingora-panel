//! Error pages and maintenance (ADR 0041).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Required by snapshots whose sites or routes have error pages.
pub const ERROR_PAGES_CAPABILITY: &str = "response.error-pages";
/// Required by snapshots with a site in maintenance.
pub const MAINTENANCE_CAPABILITY: &str = "site.maintenance";

/// The most pages a site or route lists.
pub const MOST_ERROR_PAGES: usize = 64;

fn is_false(value: &bool) -> bool {
    !*value
}

/// The error pages of a site or route.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorPages {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<ErrorPage>,
    /// Upstreams' error responses with the pages' statuses get the pages
    /// too, as nginx's `proxy_intercept_errors`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub intercept: bool,
}

impl ErrorPages {
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty() && !self.intercept
    }

    /// The page answering `status`, if any.
    pub fn page(&self, status: u16) -> Option<&ErrorPage> {
        self.pages
            .iter()
            .find(|page| page.statuses.contains(&status))
    }
}

/// A page answering errors with the statuses it names.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorPage {
    /// Between 400 and 599.
    pub statuses: BTreeSet<u16>,
    pub response: ErrorResponse,
    /// The status a body or file answers with instead of the error's, as
    /// nginx's `=code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

/// How an error page answers.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ErrorResponse {
    /// A template of request variables; `text/html` unless a media type is
    /// written.
    Body {
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    /// A file below the gateway's static root, read when an error needs it;
    /// its media type follows its extension.
    File { path: String },
    /// A redirect to a URL template.
    Redirect {
        location: String,
        #[serde(default = "found")]
        status: u16,
    },
}

const fn found() -> u16 {
    302
}

const fn unavailable() -> u16 {
    503
}

/// A site in maintenance: clients outside `allow` get the maintenance
/// response, the others reach the site as it is.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Maintenance {
    #[serde(default = "unavailable")]
    pub status: u16,
    /// A template of request variables; the site's error page for the status
    /// answers when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u32>,
    /// Networks such as `10.0.0.0/8`, or addresses, whose clients after
    /// trusted proxies reach the site.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
}

impl Default for Maintenance {
    fn default() -> Self {
        Self {
            status: unavailable(),
            body: None,
            content_type: None,
            retry_after_seconds: None,
            allow: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_found_by_status_and_default_sensibly() {
        let pages: ErrorPages = serde_json::from_value(serde_json::json!({
            "pages": [
                {"statuses": [502, 503], "response": {"kind": "body", "body": "<h1>Soon</h1>"}},
                {"statuses": [404], "response": {"kind": "redirect", "location": "/"}}
            ]
        }))
        .unwrap();
        assert!(matches!(
            pages.page(503).map(|page| &page.response),
            Some(ErrorResponse::Body { .. })
        ));
        assert!(matches!(
            pages.page(404).map(|page| &page.response),
            Some(ErrorResponse::Redirect { status: 302, .. })
        ));
        assert!(pages.page(500).is_none());
        assert!(!pages.is_empty());
        assert!(ErrorPages::default().is_empty());
        let maintenance: Maintenance = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(maintenance, Maintenance::default());
        assert_eq!(maintenance.status, 503);
    }
}
