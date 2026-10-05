//! HTTP policies of the editable configuration (ADR 0037): field changes,
//! the `Server` field, CORS and compression, named by sites and routes.

use panel_ir::{CompressionPolicy, CorsPolicy, HeaderField, HeaderPolicy, ServerHeader};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Methods the Fetch Standard normalizes to uppercase; others are compared
/// as written.
const NORMALIZED_METHODS: [&str; 6] = ["DELETE", "GET", "HEAD", "OPTIONS", "POST", "PUT"];

/// What requests and responses go through, written once and named by sites
/// and routes. A route's policy applies after its site's, and its CORS and
/// compression replace the site's.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct HttpPolicy {
    /// Lowercase letters, digits, dots, hyphens and underscores.
    pub id: String,
    /// Changes to request fields before requests go upstream.
    #[serde(default, skip_serializing_if = "FieldChanges::is_empty")]
    pub request: FieldChanges,
    /// Changes to response fields before responses go to clients.
    #[serde(default, skip_serializing_if = "FieldChanges::is_empty")]
    pub response: FieldChanges,
    /// What becomes of the `Server` field of responses.
    #[serde(default, skip_serializing_if = "ServerHeader::is_keep")]
    pub server: ServerHeader,
    /// Cross-origin requests allowed, with preflights the gateway answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cors: Option<CorsPolicy>,
    /// Responses compressed with a coding the client accepts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression: Option<CompressionPolicy>,
}

/// Field changes, applied as removals, then replacements, then additions.
/// Values are templates of request variables such as `$host`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct FieldChanges {
    /// Fields removed, by case-insensitive name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
    /// Fields given one line with this value, replacing any others.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<HeaderField>,
    /// Field lines appended, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<HeaderField>,
}

impl FieldChanges {
    pub fn is_empty(&self) -> bool {
        self.remove.is_empty() && self.set.is_empty() && self.add.is_empty()
    }

    fn removed(&self) -> BTreeSet<String> {
        self.remove
            .iter()
            .map(|name| name.trim().to_ascii_lowercase())
            .collect()
    }

    fn set(&self) -> std::collections::BTreeMap<String, String> {
        self.set
            .iter()
            .map(|field| (field.name.trim().to_ascii_lowercase(), field.value.clone()))
            .collect()
    }

    fn added(&self) -> Vec<HeaderField> {
        self.add
            .iter()
            .map(|field| HeaderField {
                name: field.name.trim().to_ascii_lowercase(),
                value: field.value.clone(),
            })
            .collect()
    }
}

impl HttpPolicy {
    /// The policy as the gateway receives it: field names in lowercase,
    /// origins serialized as browsers send them, and the methods the Fetch
    /// Standard normalizes in uppercase.
    pub fn compile(&self) -> HeaderPolicy {
        let lowercase = |names: &[String]| -> Vec<String> {
            names
                .iter()
                .map(|name| name.trim().to_ascii_lowercase())
                .collect()
        };
        HeaderPolicy {
            id: self.id.clone(),
            request_set: self.request.set(),
            request_remove: self.request.removed(),
            response_set: self.response.set(),
            response_remove: self.response.removed(),
            request_add: self.request.added(),
            response_add: self.response.added(),
            server: self.server.clone(),
            cors: self.cors.as_ref().map(|cors| CorsPolicy {
                allowed_origins: lowercase(&cors.allowed_origins),
                allowed_methods: cors
                    .allowed_methods
                    .iter()
                    .map(|method| {
                        let method = method.trim();
                        if NORMALIZED_METHODS
                            .iter()
                            .any(|normal| normal.eq_ignore_ascii_case(method))
                        {
                            method.to_ascii_uppercase()
                        } else {
                            method.to_owned()
                        }
                    })
                    .collect(),
                allowed_headers: lowercase(&cors.allowed_headers),
                exposed_headers: lowercase(&cors.exposed_headers),
                ..cors.clone()
            }),
            compression: self
                .compression
                .as_ref()
                .map(|compression| CompressionPolicy {
                    types: lowercase(&compression.types),
                    ..compression.clone()
                }),
        }
    }

    /// Every problem with the policy on its own, as messages.
    pub fn problems(&self) -> Vec<String> {
        let mut problems: Vec<String> = panel_engine::http_policy_problems(&self.compile())
            .into_iter()
            .map(|problem| format!("the policy {problem}"))
            .collect();
        for (side, changes) in [("request", &self.request), ("response", &self.response)] {
            let mut set = BTreeSet::new();
            for field in &changes.set {
                if !set.insert(field.name.trim().to_ascii_lowercase()) {
                    problems.push(format!(
                        "the policy sets the {side} field {} twice",
                        field.name
                    ));
                }
            }
            for name in &changes.remove {
                if set.contains(&name.trim().to_ascii_lowercase()) {
                    problems.push(format!(
                        "the policy both sets and removes the {side} field {name}"
                    ));
                }
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_ir::CompressionAlgorithm;

    fn field(name: &str, value: &str) -> HeaderField {
        HeaderField {
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn policies_compile_to_the_gateway_form() {
        let policy = HttpPolicy {
            id: "api".into(),
            request: FieldChanges {
                remove: vec!["X-Internal".into()],
                set: vec![field("X-Tenant", "$host")],
                add: vec![field("X-Hop", "panel")],
            },
            response: FieldChanges {
                set: vec![field("X-Frame-Options", "DENY")],
                ..FieldChanges::default()
            },
            server: ServerHeader::Remove,
            cors: Some(CorsPolicy {
                allowed_origins: vec!["https://*.Shop.Example".into()],
                allowed_methods: vec!["put".into(), "purge".into()],
                allowed_headers: vec!["X-Api-Key".into()],
                exposed_headers: vec!["X-Request-Id".into()],
                allow_credentials: true,
                max_age_seconds: Some(600),
            }),
            compression: Some(CompressionPolicy {
                algorithms: [CompressionAlgorithm::Gzip].into(),
                types: vec!["Text/*".into()],
                min_bytes: 256,
            }),
        };
        assert!(policy.problems().is_empty(), "{:?}", policy.problems());
        let compiled = policy.compile();
        assert!(compiled.request_remove.contains("x-internal"));
        assert_eq!(compiled.request_set["x-tenant"], "$host");
        assert_eq!(compiled.request_add[0].name, "x-hop");
        assert_eq!(compiled.response_set["x-frame-options"], "DENY");
        let cors = compiled.cors.expect("CORS");
        assert_eq!(cors.allowed_origins, ["https://*.shop.example"]);
        assert_eq!(cors.allowed_methods, ["PUT", "purge"]);
        assert_eq!(cors.allowed_headers, ["x-api-key"]);
        assert_eq!(compiled.compression.expect("compression").types, ["text/*"]);
    }

    #[test]
    fn policies_report_each_problem() {
        let policy = HttpPolicy {
            id: "bad".into(),
            request: FieldChanges {
                remove: vec!["x-tenant".into()],
                set: vec![
                    field("x-tenant", "a"),
                    field("X-Tenant", "b"),
                    field("Host", "x"),
                ],
                add: vec![field("bad name", "x")],
            },
            response: FieldChanges {
                add: vec![field("x-note", "line\nbreak")],
                ..FieldChanges::default()
            },
            server: ServerHeader::Replace { value: " ".into() },
            cors: Some(CorsPolicy {
                allowed_origins: vec!["*".into()],
                allow_credentials: true,
                ..CorsPolicy::default()
            }),
            compression: Some(CompressionPolicy {
                algorithms: BTreeSet::new(),
                types: vec!["html".into()],
                min_bytes: 0,
            }),
        };
        let problems = policy.problems();
        for expected in [
            "the policy changes the request field host, which the gateway keeps",
            "the policy names \"bad name\", which is not a request field name",
            "the policy sets a response value with a line break",
            "the policy replaces Server with a value that is empty or spans lines",
            "the policy allows credentials for every origin; list the origins instead of *",
            "the policy compresses with no coding",
            "the policy compresses \"html\", which is not a media type",
            "the policy sets the request field X-Tenant twice",
            "the policy both sets and removes the request field x-tenant",
        ] {
            assert!(
                problems.iter().any(|problem| problem == expected),
                "{expected}: {problems:#?}"
            );
        }
    }
}
