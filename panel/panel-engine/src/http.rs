//! Checks of HTTP policies (ADR 0037): field names are tokens, values are
//! templates on one line, a policy leaves framing, hop-by-hop, `Host` and
//! forwarding fields alone, CORS origins parse, and every reference names a
//! policy of the snapshot.

use crate::conditions::token;
use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::{
    template::parse_template, CorsPolicy, HeaderPolicy, RuntimeSnapshot, ServerHeader,
    HTTP_POLICIES_CAPABILITY,
};
use std::collections::BTreeSet;

/// Request fields a policy may not change: hop-by-hop and framing fields,
/// `Host`, which an upstream names, and the forwarding fields the gateway
/// writes.
const RESERVED_REQUEST_FIELDS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "host",
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
    "via",
];

/// Response fields a policy may not change: hop-by-hop and framing fields,
/// and the content coding compression decides.
const RESERVED_RESPONSE_FIELDS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "content-encoding",
];

const MOST_FIELDS: usize = 64;
const MOST_VALUE_BYTES: usize = 4096;
/// The longest a preflight may be cached, a day.
pub const MOST_PREFLIGHT_SECONDS: u32 = 86_400;
const MOST_MIN_BYTES: u64 = 1 << 30;

pub(crate) fn validate_http(snapshot: &RuntimeSnapshot, diagnostics: &mut Vec<Diagnostic>) {
    let mut report = |resource: &str, message: String| {
        diagnostics
            .push(Diagnostic::error(ErrorCode::VALIDATION_FAILED, message).with_resource(resource));
    };
    let mut ids = BTreeSet::new();
    for policy in &snapshot.header_policies {
        if policy.id.trim().is_empty() {
            report("http_policies", "an HTTP policy has no ID".into());
        } else if !ids.insert(policy.id.as_str()) {
            report(
                &policy.id,
                format!("HTTP policy {} is defined twice", policy.id),
            );
        }
        for problem in problems(policy) {
            report(&policy.id, format!("HTTP policy {} {problem}", policy.id));
        }
    }
    let references = snapshot
        .sites
        .iter()
        .map(|site| (site.id.as_str(), site.header_policy_id.as_deref()))
        .chain(
            snapshot
                .routes
                .iter()
                .map(|route| (route.id.as_str(), route.header_policy_id.as_deref())),
        );
    let mut referenced = false;
    for (owner, policy) in references {
        let Some(policy) = policy else {
            continue;
        };
        referenced = true;
        if !ids.contains(policy) {
            report(
                owner,
                format!("{owner} names an unknown HTTP policy {policy}"),
            );
        }
    }
    let declared = snapshot
        .required_capabilities()
        .iter()
        .any(|capability| capability.name == HTTP_POLICIES_CAPABILITY);
    if referenced && !declared {
        report(
            "http_policies",
            format!("HTTP policies are used without requiring {HTTP_POLICIES_CAPABILITY}"),
        );
    }
}

/// What is wrong with `policy`, one line each.
pub fn problems(policy: &HeaderPolicy) -> Vec<String> {
    let mut found = Vec::new();
    let mut fields = |side: &str, reserved: &[&str], names: Vec<&str>, values: Vec<&str>| {
        if names.len() > MOST_FIELDS {
            found.push(format!(
                "changes {} {side} fields, more than {MOST_FIELDS}",
                names.len()
            ));
        }
        for name in names {
            if !token(name) {
                found.push(format!("names {name:?}, which is not a {side} field name"));
            } else if reserved.contains(&name.to_ascii_lowercase().as_str()) {
                found.push(format!(
                    "changes the {side} field {name}, which the gateway keeps"
                ));
            }
        }
        for value in values {
            if value.len() > MOST_VALUE_BYTES {
                found.push(format!(
                    "sets a {side} value of {} bytes, more than {MOST_VALUE_BYTES}",
                    value.len()
                ));
            } else if value.contains(['\r', '\n', '\0']) {
                found.push(format!("sets a {side} value with a line break"));
            } else if let Err(error) = parse_template(value) {
                found.push(format!(
                    "sets a {side} value that is not a template: {error}"
                ));
            }
        }
    };
    fields(
        "request",
        RESERVED_REQUEST_FIELDS,
        policy
            .request_set
            .keys()
            .chain(policy.request_remove.iter())
            .map(String::as_str)
            .chain(policy.request_add.iter().map(|field| field.name.as_str()))
            .collect(),
        policy
            .request_set
            .values()
            .map(String::as_str)
            .chain(policy.request_add.iter().map(|field| field.value.as_str()))
            .collect(),
    );
    fields(
        "response",
        RESERVED_RESPONSE_FIELDS,
        policy
            .response_set
            .keys()
            .chain(policy.response_remove.iter())
            .map(String::as_str)
            .chain(policy.response_add.iter().map(|field| field.name.as_str()))
            .collect(),
        policy
            .response_set
            .values()
            .map(String::as_str)
            .chain(policy.response_add.iter().map(|field| field.value.as_str()))
            .collect(),
    );
    if let ServerHeader::Replace { value } = &policy.server {
        if value.trim().is_empty() || value.contains(['\r', '\n', '\0']) {
            found.push("replaces Server with a value that is empty or spans lines".into());
        }
    }
    if let Some(cors) = &policy.cors {
        cors_problems(cors, &mut found);
    }
    if let Some(compression) = &policy.compression {
        if compression.algorithms.is_empty() {
            found.push("compresses with no coding".into());
        }
        if compression.types.is_empty() {
            found.push("compresses no media type".into());
        }
        for media in &compression.types {
            let fine = media.split_once('/').is_some_and(|(kind, subtype)| {
                (kind == "*" && subtype == "*")
                    || (token(kind) && (subtype == "*" || token(subtype)))
            });
            if !fine {
                found.push(format!("compresses {media:?}, which is not a media type"));
            }
        }
        if compression.min_bytes > MOST_MIN_BYTES {
            found.push(format!(
                "compresses only responses of {} bytes or more, beyond {MOST_MIN_BYTES}",
                compression.min_bytes
            ));
        }
    }
    found
}

fn cors_problems(cors: &CorsPolicy, found: &mut Vec<String>) {
    if cors.allowed_origins.is_empty() {
        found.push("allows CORS for no origin".into());
    }
    for origin in &cors.allowed_origins {
        if origin == "*" {
            if cors.allow_credentials {
                found.push(
                    "allows credentials for every origin; list the origins instead of *".into(),
                );
            }
        } else if !origin_pattern(origin) {
            found.push(format!(
                "allows {origin:?}, which is not an origin such as https://shop.example or https://*.shop.example"
            ));
        }
    }
    for (what, names) in [
        ("method", &cors.allowed_methods),
        ("request field", &cors.allowed_headers),
        ("response field", &cors.exposed_headers),
    ] {
        for name in names
            .iter()
            .filter(|name| name.as_str() != "*" && !token(name))
        {
            found.push(format!("allows {name:?}, which is not a {what}"));
        }
    }
    if cors
        .max_age_seconds
        .is_some_and(|seconds| seconds > MOST_PREFLIGHT_SECONDS)
    {
        found.push(format!(
            "caches preflights for more than {MOST_PREFLIGHT_SECONDS} seconds"
        ));
    }
}

/// `scheme://host[:port]`, the host optionally `*.parent` of one label or
/// a bracketed IPv6 address.
fn origin_pattern(origin: &str) -> bool {
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    let (host_fine, port) = match rest.strip_prefix('[') {
        Some(literal) => match literal.split_once(']') {
            Some((address, after)) => (
                address.parse::<std::net::Ipv6Addr>().is_ok(),
                match after {
                    "" => None,
                    after => match after.strip_prefix(':') {
                        Some(port) => Some(port),
                        None => return false,
                    },
                },
            ),
            None => return false,
        },
        None => {
            let (host, port) = match rest.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (rest, None),
            };
            let host = host.strip_prefix("*.").unwrap_or(host);
            let fine = !host.is_empty()
                && host.split('.').all(|label| {
                    !label.is_empty()
                        && label.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                        })
                });
            (fine, port)
        }
    };
    matches!(scheme, "http" | "https")
        && host_fine
        && port.is_none_or(|port| port.parse::<u16>().is_ok_and(|port| port > 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_ir::{CompressionAlgorithm, CompressionPolicy, HeaderField};

    fn policy() -> HeaderPolicy {
        HeaderPolicy {
            id: "api".into(),
            request_set: [("x-tenant".to_owned(), "$host".to_owned())].into(),
            response_add: vec![HeaderField {
                name: "link".into(),
                value: "</app.css>; rel=preload".into(),
            }],
            server: ServerHeader::Replace {
                value: "shop".into(),
            },
            cors: Some(CorsPolicy {
                allowed_origins: vec![
                    "https://*.shop.example".into(),
                    "http://localhost:5173".into(),
                ],
                allowed_methods: vec!["PUT".into()],
                allowed_headers: vec!["x-api-key".into()],
                allow_credentials: true,
                max_age_seconds: Some(600),
                ..CorsPolicy::default()
            }),
            compression: Some(CompressionPolicy {
                algorithms: [CompressionAlgorithm::Gzip].into(),
                types: vec!["text/*".into(), "application/json".into()],
                min_bytes: 1024,
            }),
            ..HeaderPolicy::default()
        }
    }

    #[test]
    fn well_formed_policies_pass() {
        assert_eq!(problems(&policy()), Vec::<String>::new());
    }

    #[test]
    fn reserved_fields_bad_values_and_loose_cors_are_named() {
        let mut bad = policy();
        bad.request_set
            .insert("Host".into(), "other.example".into());
        bad.request_remove.insert("x forwarded".into());
        bad.response_set.insert("content-length".into(), "0".into());
        bad.response_add.push(HeaderField {
            name: "x-split".into(),
            value: "a\r\nb".into(),
        });
        let cors = bad.cors.as_mut().unwrap();
        cors.allowed_origins = vec!["*".into(), "shop.example".into()];
        cors.max_age_seconds = Some(MOST_PREFLIGHT_SECONDS + 1);
        bad.compression.as_mut().unwrap().types = vec!["json".into()];
        let found = problems(&bad);
        assert_eq!(found.len(), 8, "{found:#?}");
        assert!(found
            .iter()
            .any(|problem| problem.contains("Host, which the gateway keeps")));
        assert!(found
            .iter()
            .any(|problem| problem.contains("credentials for every origin")));
    }
}
