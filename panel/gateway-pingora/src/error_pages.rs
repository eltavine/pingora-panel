//! Error pages and maintenance of sites and routes (ADR 0041), compiled when
//! a snapshot is prepared and answered per request.

use crate::{
    responses,
    template::{Facts, Template},
};
use bytes::Bytes;
use http::header;
use panel_domain::IpNetwork;
use panel_errors::{PanelError, Result};
use panel_ir::{ErrorPages, ErrorResponse, Maintenance};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::{
    collections::HashMap,
    net::IpAddr,
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tokio::io::AsyncReadExt;

/// The most of a file a page sends.
const MOST_FILE_BYTES: u64 = 1 << 20;
const HTML: &str = "text/html; charset=utf-8";

/// The pages of a site or route, by status.
#[derive(Default)]
pub(crate) struct PageSet {
    pages: HashMap<u16, Arc<Page>>,
    intercept: bool,
}

pub(crate) struct Page {
    response: PageResponse,
    status: Option<u16>,
}

enum PageResponse {
    Body {
        body: Template,
        content_type: String,
    },
    File {
        base: PathBuf,
        path: PathBuf,
        content_type: String,
    },
    Redirect {
        location: Template,
        status: u16,
    },
}

/// What a page answers with, made for a request.
pub(crate) enum Answer {
    Full {
        status: u16,
        content_type: String,
        body: Bytes,
    },
    Redirect {
        status: u16,
        location: String,
    },
}

/// A page made for a request, its file still to read.
pub(crate) enum Prepared {
    Ready(Answer),
    File {
        base: PathBuf,
        path: PathBuf,
        status: u16,
        content_type: String,
    },
}

impl PageSet {
    pub(crate) fn compile(
        pages: &ErrorPages,
        static_root: Option<&Path>,
        owner: &str,
    ) -> Result<Self> {
        let invalid = |detail: String| PanelError::validation_failed(format!("{owner} {detail}"));
        let mut compiled = HashMap::new();
        for page in &pages.pages {
            let response = match &page.response {
                ErrorResponse::Body { body, content_type } => PageResponse::Body {
                    body: Template::parse(body).map_err(|error| {
                        invalid(format!(
                            "has an error page body that is not a template: {error}"
                        ))
                    })?,
                    content_type: content_type.clone().unwrap_or_else(|| HTML.to_owned()),
                },
                ErrorResponse::File { path } => {
                    let base = static_root
                        .ok_or_else(|| {
                            invalid("has an error page file, but the gateway has no static content root".into())
                        })?
                        .canonicalize()
                        .map_err(|_| invalid("has an error page file, but the static content root does not exist".into()))?;
                    let relative = Path::new(path);
                    if !relative
                        .components()
                        .all(|component| matches!(component, Component::Normal(_)))
                    {
                        return Err(invalid(format!(
                            "has an error page file {path:?} outside the static content root"
                        )));
                    }
                    PageResponse::File {
                        path: base.join(relative),
                        content_type: crate::static_files::content_type(relative),
                        base,
                    }
                }
                ErrorResponse::Redirect { location, status } => PageResponse::Redirect {
                    location: Template::parse(location).map_err(|error| {
                        invalid(format!(
                            "has an error page location that is not a template: {error}"
                        ))
                    })?,
                    status: *status,
                },
            };
            let answering = Arc::new(Page {
                response,
                status: page.status,
            });
            for status in &page.statuses {
                compiled.insert(*status, Arc::clone(&answering));
            }
        }
        Ok(Self {
            pages: compiled,
            intercept: pages.intercept,
        })
    }

    pub(crate) fn page(&self, status: u16) -> Option<&Arc<Page>> {
        self.pages.get(&status)
    }

    /// Whether an upstream's response with `status` gets its page.
    pub(crate) fn intercepts(&self, status: u16) -> bool {
        self.intercept && self.pages.contains_key(&status)
    }
}

impl Page {
    /// The page for an error with `status`, filled in from `facts`.
    pub(crate) fn prepare(&self, status: u16, facts: &Facts<'_>) -> Prepared {
        let status = self.status.unwrap_or(status);
        match &self.response {
            PageResponse::Body { body, content_type } => Prepared::Ready(Answer::Full {
                status,
                content_type: content_type.clone(),
                body: body.render(facts),
            }),
            PageResponse::File {
                base,
                path,
                content_type,
            } => Prepared::File {
                base: base.clone(),
                path: path.clone(),
                status,
                content_type: content_type.clone(),
            },
            PageResponse::Redirect { location, status } => Prepared::Ready(Answer::Redirect {
                status: *status,
                location: String::from_utf8_lossy(&location.render(facts)).into_owned(),
            }),
        }
    }
}

impl Prepared {
    /// The answer, its file read; `None` when the file cannot be, so the
    /// error is answered without its page.
    pub(crate) async fn load(self) -> Option<Answer> {
        let (base, path, status, content_type) = match self {
            Self::Ready(answer) => return Some(answer),
            Self::File {
                base,
                path,
                status,
                content_type,
            } => (base, path, status, content_type),
        };
        let resolved = tokio::fs::canonicalize(&path).await.ok()?;
        if !resolved.starts_with(&base) {
            tracing::warn!(event = "error_page_outside_root", path = %path.display());
            return None;
        }
        let file = tokio::fs::File::open(&resolved).await.ok()?;
        let mut body = Vec::new();
        if let Err(error) = file.take(MOST_FILE_BYTES).read_to_end(&mut body).await {
            tracing::warn!(event = "error_page_unreadable", path = %path.display(), %error);
            return None;
        }
        Some(Answer::Full {
            status,
            content_type,
            body: Bytes::from(body),
        })
    }
}

impl Answer {
    /// Sends the answer, with `headers` of the error, such as `Retry-After`.
    pub(crate) async fn send(
        self,
        session: &mut Session,
        headers: &[(header::HeaderName, &str)],
    ) -> pingora_core::Result<()> {
        match self {
            Self::Full {
                status,
                content_type,
                body,
            } => {
                let mut all = Vec::with_capacity(headers.len() + 1);
                all.push((header::CONTENT_TYPE, content_type.as_str()));
                all.extend(
                    headers
                        .iter()
                        .filter(|(name, _)| name != header::CONTENT_TYPE)
                        .cloned(),
                );
                responses::send(session, status, &all, body).await
            }
            Self::Redirect { status, location } => {
                responses::redirect(session, status, &location).await
            }
        }
    }

    /// Turns an upstream's error response into this answer; the body to
    /// send in place of the upstream's, empty for redirects.
    ///
    /// As nginx's intercepted errors, the answer keeps none of the
    /// upstream's fields but those its status needs (RFC 9110 §10.2.3,
    /// §11.6, §15.5.6).
    pub(crate) fn replace(self, response: &mut ResponseHeader) -> pingora_core::Result<Bytes> {
        let (status, body) = match &self {
            Self::Full { status, body, .. } => (*status, body.clone()),
            Self::Redirect { status, .. } => (*status, Bytes::new()),
        };
        let mut replaced = ResponseHeader::build(status, None)?;
        replaced.set_version(response.version);
        for name in [
            header::DATE,
            header::WWW_AUTHENTICATE,
            header::PROXY_AUTHENTICATE,
            header::RETRY_AFTER,
            header::ALLOW,
        ] {
            for value in response.headers.get_all(&name) {
                replaced.append_header(name.clone(), value.clone())?;
            }
        }
        match self {
            Self::Full { content_type, .. } => {
                replaced.insert_header(header::CONTENT_TYPE, content_type)?;
            }
            Self::Redirect { location, .. } => {
                replaced.insert_header(header::LOCATION, location)?;
            }
        }
        replaced.insert_header(header::CONTENT_LENGTH, body.len().to_string())?;
        *response = replaced;
        Ok(body)
    }
}

/// A site's maintenance, its allowlist parsed.
pub(crate) struct MaintenancePlan {
    pub status: u16,
    pub body: Option<Template>,
    pub content_type: Option<String>,
    pub retry_after: Option<String>,
    allow: Vec<IpNetwork>,
}

impl MaintenancePlan {
    pub(crate) fn compile(maintenance: &Maintenance, owner: &str) -> Result<Self> {
        let invalid = |detail: String| PanelError::validation_failed(format!("{owner} {detail}"));
        Ok(Self {
            status: maintenance.status,
            body: maintenance
                .body
                .as_deref()
                .map(Template::parse)
                .transpose()
                .map_err(|error| {
                    invalid(format!(
                        "has a maintenance body that is not a template: {error}"
                    ))
                })?,
            content_type: maintenance.content_type.clone(),
            retry_after: maintenance
                .retry_after_seconds
                .map(|seconds| seconds.to_string()),
            allow: maintenance
                .allow
                .iter()
                .map(|network| {
                    IpNetwork::new(network).map_err(|_| {
                        invalid(format!(
                            "allows {network:?}, which is not a network or address"
                        ))
                    })
                })
                .collect::<Result<_>>()?,
        })
    }

    /// Whether `client`, after trusted proxies, reaches the site.
    pub(crate) fn admits(&self, client: Option<IpAddr>) -> bool {
        client.is_some_and(|client| self.allow.iter().any(|network| network.contains(client)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_ir::ErrorPage;

    fn pages(response: ErrorResponse) -> ErrorPages {
        ErrorPages {
            pages: vec![ErrorPage {
                statuses: [404, 410].into(),
                response,
                status: None,
            }],
            intercept: true,
        }
    }

    #[test]
    fn pages_answer_each_of_their_statuses() {
        let set = PageSet::compile(
            &pages(ErrorResponse::Body {
                body: "gone".into(),
                content_type: None,
            }),
            None,
            "site shop",
        )
        .unwrap();
        assert!(set.page(404).is_some() && set.page(410).is_some());
        assert!(set.page(500).is_none());
        assert!(set.intercepts(410) && !set.intercepts(500));
    }

    #[test]
    fn file_pages_stay_below_the_static_root() {
        let root = tempfile::tempdir().unwrap();
        let file = |path: &str| pages(ErrorResponse::File { path: path.into() });
        assert!(PageSet::compile(&file("errors/404.html"), Some(root.path()), "site shop").is_ok());
        for outside in ["../404.html", "/etc/passwd", "errors/../../404.html"] {
            let error = PageSet::compile(&file(outside), Some(root.path()), "site shop")
                .err()
                .unwrap();
            assert!(
                error
                    .to_string()
                    .contains("outside the static content root"),
                "{error}"
            );
        }
        let error = PageSet::compile(&file("404.html"), None, "site shop")
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("no static content root"),
            "{error}"
        );
    }

    #[test]
    fn an_intercepted_response_keeps_only_what_its_status_needs() {
        let mut response = ResponseHeader::build(401, None).unwrap();
        for (name, value) in [
            ("www-authenticate", "Basic realm=\"shop\""),
            ("set-cookie", "session=1"),
            ("content-type", "application/json"),
            ("content-encoding", "gzip"),
            ("content-length", "999"),
        ] {
            response.append_header(name, value).unwrap();
        }
        let body = Answer::Full {
            status: 401,
            content_type: HTML.into(),
            body: Bytes::from_static(b"<p>sign in</p>"),
        }
        .replace(&mut response)
        .unwrap();
        assert_eq!(body.as_ref(), b"<p>sign in</p>");
        assert_eq!(response.status.as_u16(), 401);
        assert_eq!(response.headers["www-authenticate"], "Basic realm=\"shop\"");
        assert_eq!(response.headers["content-type"], HTML);
        assert_eq!(response.headers["content-length"], "14");
        assert!(response.headers.get("set-cookie").is_none());
        assert!(response.headers.get("content-encoding").is_none());

        let mut response = ResponseHeader::build(404, None).unwrap();
        let body = Answer::Redirect {
            status: 302,
            location: "/elsewhere".into(),
        }
        .replace(&mut response)
        .unwrap();
        assert!(body.is_empty());
        assert_eq!(response.status.as_u16(), 302);
        assert_eq!(response.headers["location"], "/elsewhere");
    }

    #[test]
    fn maintenance_admits_only_its_allowlist() {
        let plan = MaintenancePlan::compile(
            &Maintenance {
                allow: vec!["10.0.0.0/8".into(), "2001:db8::1".into()],
                ..Maintenance::default()
            },
            "site shop",
        )
        .unwrap();
        assert!(plan.admits(Some("10.1.2.3".parse().unwrap())));
        assert!(plan.admits(Some("2001:db8::1".parse().unwrap())));
        assert!(!plan.admits(Some("192.0.2.1".parse().unwrap())));
        assert!(!plan.admits(None));
        assert_eq!(plan.status, 503);
        let error = MaintenancePlan::compile(
            &Maintenance {
                allow: vec!["office".into()],
                ..Maintenance::default()
            },
            "site shop",
        )
        .err()
        .unwrap();
        assert!(
            error.to_string().contains("not a network or address"),
            "{error}"
        );
    }
}
