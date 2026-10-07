//! Error pages, maintenance, `robots.txt` and favicons (ADR 0041).

use super::{Expansion, Lowerer};
use crate::{
    codes,
    values::{print_bool, print_duration_ms, Params},
    variables::escape,
};
use panel_config_model::{Favicon, Robots, SiteMaintenance};
use panel_dsl::{Argument, Directive};
use panel_ir::{ErrorPage, ErrorPages, ErrorResponse};
use std::collections::BTreeSet;

/// What one `error_page` writes.
pub(super) enum PageWrite {
    Page(ErrorPage),
    /// `error_page off;`: the route answers its errors without pages.
    Off,
}

impl Lowerer<'_> {
    /// `error_page <status> ... body=|file=|redirect= [type=] [status=];`,
    /// or as nginx writes it, `error_page <status> ... [=<status>] <target>;`
    /// with a URL to redirect to or a path below the static root.
    pub(super) fn error_page(
        &mut self,
        file: &str,
        directive: &Directive,
        route: bool,
    ) -> Option<PageWrite> {
        let params = Params::split(&directive.args);
        if let [only] = params.positional.as_slice() {
            if only.value == "off" && params.named.is_empty() {
                if !route {
                    self.error_with_help(
                        file,
                        only.span,
                        codes::ARGUMENTS,
                        "a server's error pages are left out rather than turned off",
                        "remove the server's error_page lines, or write `error_page off;` in a route",
                    );
                    return None;
                }
                return Some(PageWrite::Off);
            }
        }
        self.only_params(
            file,
            &params,
            &["body", "file", "redirect", "type", "status"],
        );
        let written: Vec<&str> = ["body", "file", "redirect"]
            .into_iter()
            .filter(|key| params.named.contains_key(key))
            .collect();
        let mut positional = params.positional.as_slice();
        let mut nginx_target = None;
        if written.is_empty() {
            let Some((target, rest)) = positional.split_last() else {
                self.page_help(file, directive);
                return None;
            };
            nginx_target = Some(*target);
            positional = rest;
        } else if written.len() > 1 {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                format!("the error page is written with {}", written.join(" and ")),
                "write one of body=, file= or redirect=",
            );
            return None;
        }
        let mut status = None;
        if let Some((last, rest)) = positional.split_last() {
            if let Some(code) = last.value.strip_prefix('=') {
                if code.is_empty() {
                    self.error_with_help(
                        file,
                        last.span,
                        codes::ARGUMENTS,
                        "`=` without a status takes the page's status, which an error page does not have",
                        "write the status, such as =200",
                    );
                    return None;
                }
                status = Some(self.number::<u16>(file, last, code, "an HTTP status")?);
                positional = rest;
            }
        }
        if let Some((value, arg)) = params.named.get("status") {
            if status.is_some() {
                self.error(
                    file,
                    arg.span,
                    codes::DUPLICATE,
                    "the error page's status is written twice",
                );
                return None;
            }
            status = Some(self.number::<u16>(file, arg, value, "an HTTP status")?);
        }
        let statuses = self.page_statuses(file, directive, positional)?;
        let content_type = match params.named.get("type") {
            Some((value, arg)) if params.named.contains_key("body") => {
                Some(self.expand(file, arg, value, Expansion::Text)?)
            }
            Some((_, arg)) => {
                self.error(
                    file,
                    arg.span,
                    codes::ARGUMENTS,
                    "type= goes with body=; a file's media type follows its extension",
                );
                return None;
            }
            None => None,
        };
        let redirect = |location: String, status: Option<u16>| ErrorPage {
            statuses: statuses.clone(),
            response: ErrorResponse::Redirect {
                location,
                status: status.unwrap_or(302),
            },
            status: None,
        };
        let page = if let Some(target) = nginx_target {
            let value = &target.value;
            if value.starts_with("http://")
                || value.starts_with("https://")
                || value.starts_with("$scheme")
            {
                redirect(
                    self.expand(file, target, value, Expansion::Template)?,
                    status,
                )
            } else if let Some(path) = value.strip_prefix('/') {
                ErrorPage {
                    statuses,
                    response: ErrorResponse::File {
                        path: self.expand(file, target, path, Expansion::Text)?,
                    },
                    status,
                }
            } else {
                self.error_with_help(
                    file,
                    target.span,
                    codes::ARGUMENTS,
                    format!("{value:?} is not an error page"),
                    if value.starts_with('@') {
                        "error pages do not go to named routes; write body=, file= or redirect="
                    } else {
                        "write a URL to redirect to, a /path to a file below the static root, or body=, file= or redirect="
                    },
                );
                return None;
            }
        } else if let Some((value, arg)) = params.named.get("body") {
            ErrorPage {
                statuses,
                response: ErrorResponse::Body {
                    body: self.expand(file, arg, value, Expansion::Template)?,
                    content_type,
                },
                status,
            }
        } else if let Some((value, arg)) = params.named.get("file") {
            ErrorPage {
                statuses,
                response: ErrorResponse::File {
                    path: self.expand(file, arg, value, Expansion::Text)?,
                },
                status,
            }
        } else {
            let (value, arg) = params.named["redirect"];
            redirect(self.expand(file, arg, value, Expansion::Template)?, status)
        };
        Some(PageWrite::Page(page))
    }

    fn page_statuses(
        &mut self,
        file: &str,
        directive: &Directive,
        args: &[&Argument],
    ) -> Option<BTreeSet<u16>> {
        if args.is_empty() {
            self.page_help(file, directive);
            return None;
        }
        let mut statuses = BTreeSet::new();
        for arg in args {
            let status: u16 = self.number(file, arg, &arg.value, "an HTTP status")?;
            if !(400..=599).contains(&status) {
                self.error(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{status} is not an error status (400 to 599)"),
                );
                return None;
            }
            if !statuses.insert(status) {
                self.error(
                    file,
                    arg.span,
                    codes::DUPLICATE,
                    format!("{status} is listed twice"),
                );
            }
        }
        Some(statuses)
    }

    fn page_help(&mut self, file: &str, directive: &Directive) {
        self.error_with_help(
            file,
            directive.span,
            codes::ARGUMENTS,
            "an error page is its statuses and how it answers",
            "write `error_page 404 file=errors/404.html;` or `error_page 502 503 body=\"<h1>Back soon</h1>\";`",
        );
    }

    /// `maintenance on|off [allow=...] [status=] [body=] [type=] [retry_after=];`
    pub(super) fn maintenance(
        &mut self,
        file: &str,
        directive: &Directive,
    ) -> Option<SiteMaintenance> {
        let params = Params::split(&directive.args);
        self.only_params(
            file,
            &params,
            &["allow", "status", "body", "type", "retry_after"],
        );
        let [switch] = params.positional.as_slice() else {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                "maintenance is on or off, and its settings",
                "write `maintenance on allow=10.0.0.0/8 retry_after=10m;`",
            );
            return None;
        };
        let mut maintenance = SiteMaintenance {
            enabled: self.flag(file, switch, &switch.value)?,
            status: 503,
            body: None,
            content_type: None,
            retry_after_seconds: None,
            allow: Vec::new(),
        };
        if let Some((value, arg)) = params.named.get("allow") {
            maintenance.allow = self
                .expand(file, arg, value, Expansion::Text)?
                .split(',')
                .filter(|network| !network.is_empty())
                .map(str::to_owned)
                .collect();
        }
        if let Some((value, arg)) = params.named.get("status") {
            maintenance.status = self.number(file, arg, value, "an HTTP status")?;
        }
        if let Some((value, arg)) = params.named.get("body") {
            maintenance.body = Some(self.expand(file, arg, value, Expansion::Template)?);
        }
        if let Some((value, arg)) = params.named.get("type") {
            maintenance.content_type = Some(self.expand(file, arg, value, Expansion::Text)?);
        }
        if let Some((value, arg)) = params.named.get("retry_after") {
            maintenance.retry_after_seconds = Some(self.retry_after(file, arg, value)?);
        }
        Some(maintenance)
    }

    /// `robots allow_all|disallow_all;` or `robots body=<text>;`
    pub(super) fn robots(&mut self, file: &str, directive: &Directive) -> Option<Robots> {
        let params = Params::split(&directive.args);
        self.only_params(file, &params, &["body"]);
        match (params.positional.as_slice(), params.named.get("body")) {
            ([choice], None) if choice.value == "allow_all" => Some(Robots::AllowAll),
            ([choice], None) if choice.value == "disallow_all" => Some(Robots::DisallowAll),
            ([], Some((value, arg))) => Some(Robots::Custom {
                body: self.expand(file, arg, value, Expansion::Text)?,
            }),
            _ => {
                self.error_with_help(
                    file,
                    directive.span,
                    codes::ARGUMENTS,
                    "robots allows every crawler, disallows every one, or has its own text",
                    "write `robots allow_all;`, `robots disallow_all;` or `robots body=\"User-agent: *\\nDisallow: /admin/\\n\";`",
                );
                None
            }
        }
    }

    /// `favicon no_content|file=<path>|redirect=<url>;`
    pub(super) fn favicon(&mut self, file: &str, directive: &Directive) -> Option<Favicon> {
        let params = Params::split(&directive.args);
        self.only_params(file, &params, &["file", "redirect"]);
        let named: Vec<_> = params.named.iter().collect();
        match (params.positional.as_slice(), named.as_slice()) {
            ([choice], []) if choice.value == "no_content" => Some(Favicon::NoContent),
            ([], [(&"file", (value, arg))]) => Some(Favicon::File {
                path: self.expand(file, arg, value, Expansion::Text)?,
            }),
            ([], [(&"redirect", (value, arg))]) => Some(Favicon::Redirect {
                location: self.expand(file, arg, value, Expansion::Text)?,
            }),
            _ => {
                self.error_with_help(
                    file,
                    directive.span,
                    codes::ARGUMENTS,
                    "a favicon is answered with 204, a file or a redirect",
                    "write `favicon no_content;`, `favicon file=shop/favicon.ico;` or `favicon redirect=https://cdn.example.com/icon.png;`",
                );
                None
            }
        }
    }

    /// A `Retry-After` duration, in whole seconds.
    pub(super) fn retry_after(&mut self, file: &str, arg: &Argument, value: &str) -> Option<u32> {
        let ms = self.duration(file, arg, value)?;
        if !ms.is_multiple_of(1_000) {
            self.error(
                file,
                arg.span,
                codes::TYPE,
                "Retry-After counts whole seconds",
            );
            return None;
        }
        Some(u32::try_from(ms / 1_000).unwrap_or(u32::MAX))
    }
}

/// A route's error pages from what it writes: its own pages, or the
/// server's when it turns interception on or off without pages of its own,
/// as an nginx location keeps its server's `error_page` and takes its
/// `proxy_intercept_errors`.
pub(super) fn route_pages(
    pages: Option<Vec<ErrorPage>>,
    intercept: Option<bool>,
    site: &ErrorPages,
) -> Option<ErrorPages> {
    match (pages, intercept) {
        (None, None) => None,
        (Some(pages), intercept) => Some(ErrorPages {
            pages,
            intercept: intercept.unwrap_or(site.intercept),
        }),
        (None, Some(intercept)) => Some(ErrorPages {
            pages: site.pages.clone(),
            intercept,
        }),
    }
}

/// The directives writing `pages` of a server, or of a route of a server
/// with `site` pages.
pub(crate) fn print_pages(pages: &ErrorPages, site: Option<&ErrorPages>) -> Vec<Directive> {
    let mut directives: Vec<Directive> = pages.pages.iter().map(print_page).collect();
    if site.is_some() && pages.pages.is_empty() {
        directives.push(Directive::simple("error_page", ["off"]));
    }
    if pages.intercept != site.is_some_and(|site| site.intercept) {
        directives.push(Directive::simple(
            "intercept_errors",
            [print_bool(pages.intercept)],
        ));
    }
    directives
}

fn print_page(page: &ErrorPage) -> Directive {
    let mut args: Vec<String> = page.statuses.iter().map(ToString::to_string).collect();
    match &page.response {
        ErrorResponse::Body { body, content_type } => {
            args.push(format!("body={body}"));
            if let Some(content_type) = content_type {
                args.push(format!("type={}", escape(content_type)));
            }
        }
        ErrorResponse::File { path } => args.push(format!("file={}", escape(path))),
        ErrorResponse::Redirect { location, status } => {
            args.push(format!("redirect={location}"));
            if *status != 302 {
                args.push(format!("status={status}"));
            }
        }
    }
    if let Some(status) = page.status {
        args.push(format!("status={status}"));
    }
    Directive::simple("error_page", args)
}

pub(crate) fn print_maintenance(maintenance: &SiteMaintenance) -> Directive {
    let mut args = vec![print_bool(maintenance.enabled).to_owned()];
    if !maintenance.allow.is_empty() {
        args.push(format!("allow={}", escape(&maintenance.allow.join(","))));
    }
    if maintenance.status != 503 {
        args.push(format!("status={}", maintenance.status));
    }
    if let Some(body) = &maintenance.body {
        args.push(format!("body={body}"));
    }
    if let Some(content_type) = &maintenance.content_type {
        args.push(format!("type={}", escape(content_type)));
    }
    if let Some(seconds) = maintenance.retry_after_seconds {
        args.push(format!(
            "retry_after={}",
            print_duration_ms(u64::from(seconds) * 1_000)
        ));
    }
    Directive::simple("maintenance", args)
}

pub(crate) fn print_robots(robots: &Robots) -> Directive {
    Directive::simple(
        "robots",
        [match robots {
            Robots::AllowAll => "allow_all".to_owned(),
            Robots::DisallowAll => "disallow_all".to_owned(),
            Robots::Custom { body } => format!("body={}", escape(body)),
            _ => "allow_all".to_owned(),
        }],
    )
}

pub(crate) fn print_favicon(favicon: &Favicon) -> Directive {
    Directive::simple(
        "favicon",
        [match favicon {
            Favicon::NoContent => "no_content".to_owned(),
            Favicon::File { path } => format!("file={}", escape(path)),
            Favicon::Redirect { location } => format!("redirect={}", escape(location)),
            _ => "no_content".to_owned(),
        }],
    )
}
