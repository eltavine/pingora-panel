//! Conditional requests (RFC 9110 section 13) with strong entity tags.

use crate::error::ApiError;
use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use panel_application::ContentHash;
use panel_errors::PanelError;
use serde::Serialize;

/// An entity tag (RFC 9110 section 8.8.3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EntityTag {
    weak: bool,
    opaque: String,
}

impl EntityTag {
    pub(crate) fn strong(opaque: impl Into<String>) -> Self {
        Self {
            weak: false,
            opaque: opaque.into(),
        }
    }

    /// A strong validator of a representation's bytes.
    pub(crate) fn of(representation: &[u8]) -> Self {
        Self::strong(ContentHash::from_bytes(representation).as_str())
    }

    pub(crate) fn opaque(&self) -> &str {
        &self.opaque
    }

    pub(crate) fn header_value(&self) -> HeaderValue {
        let prefix = if self.weak { "W/" } else { "" };
        HeaderValue::from_str(&format!("{prefix}\"{}\"", self.opaque))
            .expect("entity tags contain only visible ASCII")
    }

    /// Strong comparison: neither tag is weak and the opaque tags match.
    pub(crate) fn strong_eq(&self, other: &Self) -> bool {
        !self.weak && !other.weak && self.opaque == other.opaque
    }

    /// Weak comparison: the opaque tags match.
    fn weak_eq(&self, other: &Self) -> bool {
        self.opaque == other.opaque
    }
}

/// An `If-Match` or `If-None-Match` field value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Condition {
    Any,
    Tags(Vec<EntityTag>),
}

/// The combined field lines of `name`, or `None` when absent. A field that
/// does not follow the grammar fails the request rather than being guessed.
pub(crate) fn condition(
    headers: &HeaderMap,
    name: header::HeaderName,
) -> Result<Option<Condition>, ApiError> {
    let mut lines = Vec::new();
    for value in headers.get_all(&name) {
        lines.push(value.to_str().map_err(|_| invalid(&name))?.to_owned());
    }
    if lines.is_empty() {
        return Ok(None);
    }
    let value = lines.join(",");
    if value.trim() == "*" {
        return Ok(Some(Condition::Any));
    }
    let mut tags = Vec::new();
    let mut rest = value.as_str();
    loop {
        rest = rest.trim_start_matches([' ', '\t', ',']);
        if rest.is_empty() {
            break;
        }
        let (weak, quoted) = match rest.strip_prefix("W/") {
            Some(quoted) => (true, quoted),
            None => (false, rest),
        };
        let quoted = quoted.strip_prefix('"').ok_or_else(|| invalid(&name))?;
        let end = quoted.find('"').ok_or_else(|| invalid(&name))?;
        let opaque = &quoted[..end];
        if !opaque
            .bytes()
            .all(|byte| byte == 0x21 || (0x23..=0x7e).contains(&byte))
        {
            return Err(invalid(&name));
        }
        tags.push(EntityTag {
            weak,
            opaque: opaque.to_owned(),
        });
        rest = &quoted[end + 1..];
        let separated = rest.trim_start_matches([' ', '\t']);
        if !(separated.is_empty() || separated.starts_with(',')) {
            return Err(invalid(&name));
        }
    }
    if tags.is_empty() {
        return Err(invalid(&name));
    }
    Ok(Some(Condition::Tags(tags)))
}

fn invalid(name: &header::HeaderName) -> ApiError {
    ApiError::new(PanelError::invalid_argument(format!(
        "{name} must be \"*\" or a list of entity tags"
    )))
}

/// A JSON response with a strong validator, answering `304 Not Modified`
/// when the request's `If-None-Match` already names the representation.
pub(crate) fn validated_json<T: Serialize>(
    headers: &HeaderMap,
    value: &T,
) -> Result<Response, ApiError> {
    let body = serde_json::to_vec(value).map_err(|error| {
        ApiError::new(PanelError::internal(format!(
            "response cannot be encoded: {error}"
        )))
    })?;
    let tag = EntityTag::of(&body);
    let not_modified = match condition(headers, header::IF_NONE_MATCH)? {
        Some(Condition::Any) => true,
        Some(Condition::Tags(tags)) => tags.iter().any(|candidate| candidate.weak_eq(&tag)),
        None => false,
    };
    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            Body::from(body),
        )
            .into_response()
    };
    let headers = response.headers_mut();
    headers.insert(header::ETAG, tag.header_value());
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(response)
}

/// Selects the current tag named by an `If-Match` condition, by strong
/// comparison; `None` means the precondition fails.
pub(crate) fn matching(condition: &Condition, current: Option<&EntityTag>) -> Option<EntityTag> {
    let current = current?;
    match condition {
        Condition::Any => Some(current.clone()),
        Condition::Tags(tags) => tags
            .iter()
            .any(|candidate| candidate.strong_eq(current))
            .then(|| current.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> Result<Option<Condition>, ()> {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(header::IF_MATCH, HeaderValue::from_str(value).unwrap());
        }
        condition(&headers, header::IF_MATCH).map_err(drop)
    }

    #[test]
    fn fields_follow_the_entity_tag_grammar() {
        assert_eq!(parse(&[]).unwrap(), None);
        assert_eq!(parse(&["*"]).unwrap(), Some(Condition::Any));
        assert_eq!(
            parse(&["\"a\", W/\"b\"", "\"c\""]).unwrap(),
            Some(Condition::Tags(vec![
                EntityTag::strong("a"),
                EntityTag {
                    weak: true,
                    opaque: "b".into()
                },
                EntityTag::strong("c"),
            ]))
        );
        assert_eq!(
            parse(&["\"\""]).unwrap(),
            Some(Condition::Tags(vec![EntityTag::strong("")]))
        );
        for invalid in [
            "a",
            "\"a",
            "\"a\" \"b\"",
            "w/\"a\"",
            "\"a b\"",
            ",",
            "*, \"a\"",
        ] {
            assert!(parse(&[invalid]).is_err(), "{invalid}");
        }
    }

    #[test]
    fn comparison_follows_rfc_9110() {
        let strong = EntityTag::strong("1");
        let weak = EntityTag {
            weak: true,
            opaque: "1".into(),
        };
        assert!(strong.strong_eq(&EntityTag::strong("1")));
        assert!(!weak.strong_eq(&strong) && !strong.strong_eq(&weak));
        assert!(weak.weak_eq(&strong));
        assert_eq!(strong.header_value(), "\"1\"");
        assert_eq!(weak.header_value(), "W/\"1\"");

        let current = EntityTag::strong("active");
        assert_eq!(
            matching(&Condition::Any, Some(&current)),
            Some(current.clone())
        );
        assert_eq!(matching(&Condition::Any, None), None);
        assert_eq!(
            matching(
                &Condition::Tags(vec![EntityTag::strong("old"), EntityTag::strong("active")]),
                Some(&current)
            ),
            Some(current.clone())
        );
        assert_eq!(
            matching(
                &Condition::Tags(vec![EntityTag {
                    weak: true,
                    opaque: "active".into()
                }]),
                Some(&current)
            ),
            None
        );
    }
}
