//! Error page and maintenance wire conversion (ADR 0041).

use crate::{optional_string, status_code};
use panel_contracts::gateway::v1 as wire;
use panel_errors::{PanelError, Result};
use panel_ir::{ErrorPage, ErrorPages, ErrorResponse, Maintenance};

pub(super) fn decode_error_pages(value: wire::ErrorPages) -> Result<ErrorPages> {
    Ok(ErrorPages {
        pages: value
            .pages
            .into_iter()
            .map(decode_page)
            .collect::<Result<_>>()?,
        intercept: value.intercept,
    })
}

pub(super) fn encode_error_pages(value: &ErrorPages) -> wire::ErrorPages {
    wire::ErrorPages {
        pages: value.pages.iter().map(encode_page).collect(),
        intercept: value.intercept,
    }
}

fn decode_page(value: wire::ErrorPage) -> Result<ErrorPage> {
    use wire::error_page::Response;
    let response = match value.response.ok_or_else(|| {
        PanelError::invalid_argument("an error page answers in a way this gateway does not know")
    })? {
        Response::Body(body) => ErrorResponse::Body {
            body: body.body,
            content_type: optional_string(body.content_type),
        },
        Response::File(path) => ErrorResponse::File { path },
        Response::Redirect(redirect) => ErrorResponse::Redirect {
            location: redirect.location,
            status: status_code(redirect.status)?,
        },
    };
    Ok(ErrorPage {
        statuses: value
            .statuses
            .into_iter()
            .map(status_code)
            .collect::<Result<_>>()?,
        response,
        status: value.status.map(status_code).transpose()?,
    })
}

fn encode_page(value: &ErrorPage) -> wire::ErrorPage {
    use wire::error_page::Response;
    wire::ErrorPage {
        statuses: value.statuses.iter().copied().map(u32::from).collect(),
        response: Some(match &value.response {
            ErrorResponse::Body { body, content_type } => Response::Body(wire::ErrorBody {
                body: body.clone(),
                content_type: content_type.clone().unwrap_or_default(),
            }),
            ErrorResponse::File { path } => Response::File(path.clone()),
            ErrorResponse::Redirect { location, status } => {
                Response::Redirect(wire::ErrorRedirect {
                    location: location.clone(),
                    status: u32::from(*status),
                })
            }
        }),
        status: value.status.map(u32::from),
    }
}

pub(super) fn decode_maintenance(value: wire::Maintenance) -> Result<Maintenance> {
    Ok(Maintenance {
        status: status_code(value.status)?,
        body: value.has_body.then_some(value.body),
        content_type: optional_string(value.content_type),
        retry_after_seconds: value.retry_after_seconds,
        allow: value.allow,
    })
}

pub(super) fn encode_maintenance(value: &Maintenance) -> wire::Maintenance {
    wire::Maintenance {
        status: u32::from(value.status),
        body: value.body.clone().unwrap_or_default(),
        has_body: value.body.is_some(),
        content_type: value.content_type.clone().unwrap_or_default(),
        retry_after_seconds: value.retry_after_seconds,
        allow: value.allow.clone(),
    }
}
