//! Checks of error pages and maintenance (ADR 0041): statuses are errors and
//! each has one page, bodies and locations are templates, files stay below
//! the static root, media types parse, allowlists parse, and snapshots using
//! them require their capabilities.

use panel_domain::IpNetwork;
use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::{
    template::parse_template, ErrorPages, ErrorResponse, Maintenance, RuntimeSnapshot,
    ERROR_PAGES_CAPABILITY, MAINTENANCE_CAPABILITY, MOST_ERROR_PAGES,
};
use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

const MOST_BODY_BYTES: usize = 64 << 10;
const MOST_TEMPLATE_BYTES: usize = 4096;
const MOST_PATH_BYTES: usize = 1024;
const MOST_ALLOWED: usize = 256;
const REDIRECT_STATUSES: [u16; 5] = [301, 302, 303, 307, 308];

pub(crate) fn validate_pages(snapshot: &RuntimeSnapshot, diagnostics: &mut Vec<Diagnostic>) {
    let mut report = |resource: &str, message: String| {
        diagnostics
            .push(Diagnostic::error(ErrorCode::VALIDATION_FAILED, message).with_resource(resource));
    };
    let (mut pages, mut maintained) = (false, false);
    for site in &snapshot.sites {
        pages |= !site.error_pages.is_empty();
        for problem in error_page_problems(&site.error_pages) {
            report(site.id.as_str(), format!("site {} {problem}", site.id));
        }
        if let Some(maintenance) = &site.maintenance {
            maintained = true;
            for problem in maintenance_problems(maintenance) {
                report(site.id.as_str(), format!("site {} {problem}", site.id));
            }
        }
    }
    for route in &snapshot.routes {
        if let Some(own) = &route.error_pages {
            pages |= !own.is_empty();
            for problem in error_page_problems(own) {
                report(route.id.as_str(), format!("route {} {problem}", route.id));
            }
        }
    }
    let declared = |name: &str| {
        snapshot
            .required_capabilities()
            .iter()
            .any(|capability| capability.name == name)
    };
    if pages && !declared(ERROR_PAGES_CAPABILITY) {
        report(
            "sites",
            format!("error pages are used without requiring {ERROR_PAGES_CAPABILITY}"),
        );
    }
    if maintained && !declared(MAINTENANCE_CAPABILITY) {
        report(
            "sites",
            format!("maintenance is used without requiring {MAINTENANCE_CAPABILITY}"),
        );
    }
}

/// What is wrong with `pages`, one line each.
pub fn error_page_problems(pages: &ErrorPages) -> Vec<String> {
    let mut found = Vec::new();
    if pages.pages.len() > MOST_ERROR_PAGES {
        found.push(format!(
            "has {} error pages, more than {MOST_ERROR_PAGES}",
            pages.pages.len()
        ));
    }
    let mut answered = BTreeSet::new();
    for page in &pages.pages {
        if page.statuses.is_empty() {
            found.push("has an error page for no status".into());
        }
        for status in &page.statuses {
            if !(400..=599).contains(status) {
                found.push(format!(
                    "has an error page for {status}, which is not an error status (400 to 599)"
                ));
            } else if !answered.insert(*status) {
                found.push(format!("answers {status} with two error pages"));
            }
        }
        match &page.response {
            ErrorResponse::Body { body, content_type } => {
                if body.len() > MOST_BODY_BYTES {
                    found.push(format!(
                        "has an error page body of {} bytes, more than {MOST_BODY_BYTES}",
                        body.len()
                    ));
                } else if let Err(error) = parse_template(body) {
                    found.push(format!(
                        "has an error page body that is not a template: {error}"
                    ));
                }
                if let Some(problem) = content_type.as_deref().and_then(media_type_problem) {
                    found.push(format!("has an error page {problem}"));
                }
            }
            ErrorResponse::File { path } => {
                if let Some(problem) = file_problem(path) {
                    found.push(format!("has an error page file {path:?} that {problem}"));
                }
            }
            ErrorResponse::Redirect { location, status } => {
                if location.is_empty() || location.len() > MOST_TEMPLATE_BYTES {
                    found.push(format!(
                        "has an error page redirect location of {} bytes; it needs 1..={MOST_TEMPLATE_BYTES}",
                        location.len()
                    ));
                } else if let Err(error) = parse_template(location) {
                    found.push(format!(
                        "has an error page redirect location that is not a template: {error}"
                    ));
                }
                if !REDIRECT_STATUSES.contains(status) {
                    found.push(format!(
                        "has an error page redirect with {status}, which is not 301, 302, 303, 307 or 308"
                    ));
                }
                if page.status.is_some() {
                    found.push(
                        "has an error page redirect with another status; a redirect answers with its own"
                            .into(),
                    );
                }
            }
        }
        if let Some(status) = page.status.filter(|status| !(200..=599).contains(status)) {
            found.push(format!(
                "has an error page answering with {status}, which is not 200 to 599"
            ));
        }
    }
    found
}

/// What is wrong with `maintenance`, one line each.
pub fn maintenance_problems(maintenance: &Maintenance) -> Vec<String> {
    let mut found = Vec::new();
    if !(200..=599).contains(&maintenance.status) {
        found.push(format!(
            "answers maintenance with {}, which is not 200 to 599",
            maintenance.status
        ));
    }
    if let Some(body) = &maintenance.body {
        if body.len() > MOST_BODY_BYTES {
            found.push(format!(
                "has a maintenance body of {} bytes, more than {MOST_BODY_BYTES}",
                body.len()
            ));
        } else if let Err(error) = parse_template(body) {
            found.push(format!(
                "has a maintenance body that is not a template: {error}"
            ));
        }
    }
    if let Some(problem) = maintenance
        .content_type
        .as_deref()
        .and_then(media_type_problem)
    {
        found.push(format!("has a maintenance {problem}"));
    }
    if maintenance.allow.len() > MOST_ALLOWED {
        found.push(format!(
            "allows {} networks during maintenance, more than {MOST_ALLOWED}",
            maintenance.allow.len()
        ));
    }
    for network in &maintenance.allow {
        if IpNetwork::new(network).is_err() {
            found.push(format!(
                "allows {network:?} during maintenance, which is not a network or address"
            ));
        }
    }
    found
}

fn media_type_problem(value: &str) -> Option<String> {
    let essence = value.split(';').next().unwrap_or_default().trim();
    let valid = value.len() <= 256
        && !value.bytes().any(|byte| byte.is_ascii_control())
        && essence.split_once('/').is_some_and(|(kind, subtype)| {
            crate::conditions::token(kind) && crate::conditions::token(subtype)
        });
    (!valid).then(|| format!("media type {value:?} that is not type/subtype"))
}

fn file_problem(path: &str) -> Option<&'static str> {
    if path.is_empty() || path.len() > MOST_PATH_BYTES {
        return Some("is empty or too long");
    }
    let relative = Path::new(path);
    (path.contains('\\')
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_))))
    .then_some("is not a relative path below the static root without '.' or '..'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_ir::ErrorPage;

    fn page(statuses: &[u16], response: ErrorResponse) -> ErrorPage {
        ErrorPage {
            statuses: statuses.iter().copied().collect(),
            response,
            status: None,
        }
    }

    fn body(text: &str) -> ErrorResponse {
        ErrorResponse::Body {
            body: text.into(),
            content_type: None,
        }
    }

    #[test]
    fn sound_pages_and_maintenance_pass() {
        let pages = ErrorPages {
            pages: vec![
                page(&[502, 503], body("<h1>$host is down</h1>")),
                page(
                    &[404],
                    ErrorResponse::File {
                        path: "errors/404.html".into(),
                    },
                ),
                page(
                    &[410],
                    ErrorResponse::Redirect {
                        location: "https://$host/".into(),
                        status: 301,
                    },
                ),
            ],
            intercept: true,
        };
        assert_eq!(error_page_problems(&pages), Vec::<String>::new());
        let maintenance = Maintenance {
            body: Some("Back at $http_x_when".into()),
            content_type: Some("text/plain; charset=utf-8".into()),
            allow: vec!["10.0.0.0/8".into(), "2001:db8::1".into()],
            ..Maintenance::default()
        };
        assert_eq!(maintenance_problems(&maintenance), Vec::<String>::new());
    }

    #[test]
    fn broken_pages_and_maintenance_are_told() {
        let mut redirect = page(
            &[404],
            ErrorResponse::Redirect {
                location: "/".into(),
                status: 200,
            },
        );
        redirect.status = Some(200);
        let mut status = page(&[403], body("no"));
        status.status = Some(700);
        let found = error_page_problems(&ErrorPages {
            pages: vec![
                page(&[], body("none")),
                page(&[302, 500], body("$nope")),
                page(
                    &[500],
                    ErrorResponse::File {
                        path: "../x".into(),
                    },
                ),
                redirect,
                page(
                    &[502],
                    ErrorResponse::Body {
                        body: "x".into(),
                        content_type: Some("html".into()),
                    },
                ),
                status,
            ],
            intercept: false,
        });
        for (expected, at) in [
            ("for no status", 0),
            ("302, which is not an error status", 1),
            ("not a template", 2),
            ("answers 500 with two error pages", 3),
            ("not a relative path", 4),
            ("200, which is not 301", 5),
            ("a redirect answers with its own", 6),
            ("not type/subtype", 7),
            ("700, which is not 200 to 599", 8),
        ] {
            assert!(
                found.get(at).is_some_and(|line| line.contains(expected)),
                "{expected:?} at {at} in {found:#?}"
            );
        }
        let found = maintenance_problems(&Maintenance {
            status: 99,
            allow: vec!["10.0.0.0/33".into()],
            ..Maintenance::default()
        });
        assert_eq!(found.len(), 2, "{found:#?}");
    }
}
