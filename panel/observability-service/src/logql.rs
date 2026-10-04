//! LogQL for the filters clients send (ADR 0026). Values go into quoted
//! strings and text that joins a regular expression is escaped first, so no
//! filter can change a query.

use ipnet::IpNet;
use panel_contracts::observability::v1 as wire;
use panel_domain::{RouteId, SiteId};
use panel_errors::{PanelError, Result};
use std::{fmt::Write as _, net::IpAddr};

/// The gateway's records among everything the store keeps.
pub const SELECTOR: &str = r#"{service_name="pingora-panel-gateway"}"#;
/// The event name of error records; access records have another or none.
pub const ERROR_EVENT: &str = "pingora_panel.error";
const MAX_TEXT: usize = 256;
const MAX_PATH: usize = 1024;
const MAX_REQUEST_ID: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Access,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Status {
    Code(u16),
    /// The first digit of a class, such as 5 for `5xx`.
    Class(u16),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Client {
    Address(IpAddr),
    Network(IpNet),
}

/// Which records to read, checked.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Filter {
    kind: Option<Kind>,
    site: Option<SiteId>,
    route: Option<RouteId>,
    status: Option<Status>,
    client: Option<Client>,
    path_prefix: Option<String>,
    request_id: Option<String>,
    text: Option<String>,
}

fn invalid(message: impl Into<String>) -> PanelError {
    PanelError::invalid_argument(message)
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn status(value: &str) -> Result<Status> {
    let not_status = || {
        invalid(format!(
            "{value:?} is not a status code or a class such as 5xx"
        ))
    };
    match value.as_bytes() {
        [digit @ b'1'..=b'5', b'x' | b'X', b'x' | b'X'] => {
            Ok(Status::Class(u16::from(digit - b'0')))
        }
        _ => value
            .parse::<u16>()
            .ok()
            .filter(|code| (100..=599).contains(code))
            .map(Status::Code)
            .ok_or_else(not_status),
    }
}

fn client(value: &str) -> Result<Client> {
    if let Ok(address) = value.parse::<IpAddr>() {
        return Ok(Client::Address(address));
    }
    value
        .parse::<IpNet>()
        .map(|network| Client::Network(network.trunc()))
        .map_err(|_| invalid(format!("{value:?} is not an address or a CIDR block")))
}

impl Filter {
    pub fn from_wire(filter: Option<wire::LogFilter>) -> Result<Self> {
        let filter = filter.unwrap_or_default();
        let kind = match wire::LogKind::try_from(filter.kind) {
            Ok(wire::LogKind::Unspecified) => None,
            Ok(wire::LogKind::Access) => Some(Kind::Access),
            Ok(wire::LogKind::Error) => Some(Kind::Error),
            Err(_) => return Err(invalid(format!("unknown log kind {}", filter.kind))),
        };
        let site = nonempty(filter.site)
            .map(|site| SiteId::new(site).map_err(|error| invalid(error.to_string())))
            .transpose()?;
        let route = nonempty(filter.route)
            .map(|route| RouteId::new(route).map_err(|error| invalid(error.to_string())))
            .transpose()?;
        let status = nonempty(filter.status).as_deref().map(status).transpose()?;
        let client = nonempty(filter.client).as_deref().map(client).transpose()?;
        let path_prefix = nonempty(filter.path_prefix);
        if let Some(path) = &path_prefix {
            if !path.starts_with('/') || path.len() > MAX_PATH || path.contains(char::is_control) {
                return Err(invalid(format!(
                    "a path prefix starts with / and has at most {MAX_PATH} bytes"
                )));
            }
        }
        let request_id = nonempty(filter.request_id);
        if let Some(id) = &request_id {
            if id.len() > MAX_REQUEST_ID || !id.bytes().all(|byte| byte.is_ascii_graphic()) {
                return Err(invalid(format!(
                    "a request ID has at most {MAX_REQUEST_ID} visible characters"
                )));
            }
        }
        let text = nonempty(filter.text);
        if let Some(text) = &text {
            if text.chars().count() > MAX_TEXT || text.contains(char::is_control) {
                return Err(invalid(format!(
                    "search text has at most {MAX_TEXT} characters on one line"
                )));
            }
        }
        Ok(Self {
            kind,
            site,
            route,
            status,
            client,
            path_prefix,
            request_id,
            text,
        })
    }

    /// Every record of `site`, or every record.
    pub fn of_site(site: Option<SiteId>) -> Self {
        Self {
            site,
            ..Self::default()
        }
    }

    pub fn query(&self) -> String {
        let mut query = SELECTOR.to_owned();
        if let Some(text) = &self.text {
            let _ = write!(
                query,
                " |~ {}",
                quoted(&format!("(?i){}", regex::escape(text)))
            );
        }
        match self.kind {
            Some(Kind::Error) => {
                let _ = write!(query, " | event_name = {}", quoted(ERROR_EVENT));
            }
            Some(Kind::Access) => {
                let _ = write!(query, " | event_name != {}", quoted(ERROR_EVENT));
            }
            None => {}
        }
        if let Some(site) = &self.site {
            let _ = write!(
                query,
                " | pingora_panel_site_id = {}",
                quoted(site.as_str())
            );
        }
        if let Some(route) = &self.route {
            let _ = write!(
                query,
                " | pingora_panel_route_id = {}",
                quoted(route.as_str())
            );
        }
        match &self.client {
            Some(Client::Address(address)) => {
                let _ = write!(
                    query,
                    " | client_address = {}",
                    quoted(&address.to_string())
                );
            }
            Some(Client::Network(network)) => {
                let _ = write!(
                    query,
                    " | client_address = ip({})",
                    quoted(&network.to_string())
                );
            }
            None => {}
        }
        if let Some(prefix) = &self.path_prefix {
            let _ = write!(
                query,
                " | url_path =~ {}",
                quoted(&format!("{}.*", regex::escape(prefix)))
            );
        }
        if let Some(id) = &self.request_id {
            let _ = write!(query, " | pingora_panel_request_id = {}", quoted(id));
        }
        match self.status {
            Some(Status::Code(code)) => {
                let _ = write!(
                    query,
                    r#" | http_response_status_code = {code} | __error__ = """#
                );
            }
            Some(Status::Class(class)) => {
                let _ = write!(
                    query,
                    r#" | http_response_status_code >= {} | http_response_status_code < {} | __error__ = """#,
                    class * 100,
                    (class + 1) * 100
                );
            }
            None => {}
        }
        query
    }
}

/// `value` as a LogQL string, which follows Go's syntax: backslashes and
/// quotes escaped, control characters as `\u` escapes.
pub fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The site a deletion query this module wrote names, if it names one.
pub fn deletion_site(query: &str) -> Option<Option<String>> {
    let rest = query.strip_prefix(SELECTOR)?.trim();
    if rest.is_empty() {
        return Some(None);
    }
    let value = rest
        .strip_prefix('|')?
        .trim()
        .strip_prefix("pingora_panel_site_id")?
        .trim()
        .strip_prefix('=')?
        .trim();
    let site = value.strip_prefix('"')?.strip_suffix('"')?;
    (!site.contains(['"', '\\'])).then(|| Some(site.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(edit: impl FnOnce(&mut wire::LogFilter)) -> Result<Filter> {
        let mut filter = wire::LogFilter::default();
        edit(&mut filter);
        Filter::from_wire(Some(filter))
    }

    #[test]
    fn every_filter_becomes_its_own_stage() {
        let query = filter(|filter| {
            filter.kind = wire::LogKind::Error.into();
            filter.site = "shop".into();
            filter.route = "checkout".into();
            filter.status = "5xx".into();
            filter.client = "10.1.2.3/8".into();
            filter.path_prefix = "/api/v1.2".into();
            filter.request_id = "req-7".into();
            filter.text = "Timed out".into();
        })
        .unwrap()
        .query();
        assert_eq!(
            query,
            concat!(
                r#"{service_name="pingora-panel-gateway"} |~ "(?i)Timed out""#,
                r#" | event_name = "pingora_panel.error""#,
                r#" | pingora_panel_site_id = "shop""#,
                r#" | pingora_panel_route_id = "checkout""#,
                r#" | client_address = ip("10.0.0.0/8")"#,
                r#" | url_path =~ "/api/v1\\.2.*""#,
                r#" | pingora_panel_request_id = "req-7""#,
                r#" | http_response_status_code >= 500 | http_response_status_code < 600"#,
                r#" | __error__ = """#,
            )
        );
        assert_eq!(Filter::default().query(), SELECTOR);
        assert_eq!(
            filter(|filter| filter.kind = wire::LogKind::Access.into())
                .unwrap()
                .query(),
            r#"{service_name="pingora-panel-gateway"} | event_name != "pingora_panel.error""#
        );
        assert!(filter(|filter| filter.status = "404".into())
            .unwrap()
            .query()
            .ends_with(r#" | http_response_status_code = 404 | __error__ = """#));
    }

    #[test]
    fn text_cannot_leave_its_string_or_its_pattern() {
        let query = filter(|filter| filter.text = r#"a" | line_format "x\ (.*)"#.into())
            .unwrap()
            .query();
        assert_eq!(
            query,
            r#"{service_name="pingora-panel-gateway"} |~ "(?i)a\" \\| line_format \"x\\\\ \\(\\.\\*\\)""#
        );
        assert_eq!(quoted("a\u{1}b"), r#""a\u0001b""#);
    }

    #[test]
    fn values_are_checked() {
        for edit in [
            (|filter: &mut wire::LogFilter| filter.site = "bad site".into())
                as fn(&mut wire::LogFilter),
            |filter| filter.status = "6xx".into(),
            |filter| filter.status = "99".into(),
            |filter| filter.client = "example.com".into(),
            |filter| filter.path_prefix = "api".into(),
            |filter| filter.request_id = "has space".into(),
            |filter| filter.text = "line\nbreak".into(),
            |filter| filter.text = "x".repeat(MAX_TEXT + 1),
            |filter| filter.kind = 9,
        ] {
            let error = filter(edit).unwrap_err();
            assert_eq!(
                error.code.as_str(),
                panel_errors::ErrorCode::INVALID_ARGUMENT
            );
        }
        assert!(filter(|filter| filter.client = "2001:db8::1".into()).is_ok());
    }

    #[test]
    fn deletion_queries_name_their_site() {
        let shop = Filter::of_site(Some(SiteId::new("shop").unwrap())).query();
        assert_eq!(deletion_site(&shop), Some(Some("shop".to_owned())));
        assert_eq!(deletion_site(SELECTOR), Some(None));
        assert_eq!(
            deletion_site(
                r#"{service_name="pingora-panel-gateway"} | pingora_panel_site_id="blog""#
            ),
            Some(Some("blog".to_owned()))
        );
        assert_eq!(deletion_site(r#"{app="other"}"#), None);
    }
}
