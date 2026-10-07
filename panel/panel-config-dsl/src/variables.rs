//! Variable references in argument values.
//!
//! `$name` and `${name}` name a constant set with `set` or a request
//! variable; `${env:NAME}` reads the environment and `${lua:name}` what a
//! script last gave a variable of the request. `$$` is a literal `$`, and a
//! `$` not followed by a name is literal too, so regular expressions keep
//! their anchors.

/// Request variables, evaluated by the gateway for each request.
pub const REQUEST_VARIABLES: &[&str] = &[
    "host",
    "uri",
    "method",
    "scheme",
    "client_ip",
    "request_id",
    "upstream_addr",
    "request_uri",
    "args",
    "query_string",
    "is_args",
];

/// Whether `name` is evaluated per request: one of [`REQUEST_VARIABLES`], or a
/// header (`http_<name>`), cookie (`cookie_<name>`) or query parameter
/// (`arg_<name>`).
pub fn is_request_variable(name: &str) -> bool {
    REQUEST_VARIABLES.contains(&name)
        || ["http_", "cookie_", "arg_"].iter().any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|rest| !rest.is_empty())
        })
}

/// Environment variables visible to `${env:NAME}` carry this prefix, so a
/// configuration cannot read the service's other settings.
pub const ENVIRONMENT_PREFIX: &str = "PINGORA_PANEL_DSL_";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Piece<'a> {
    Text(&'a str),
    /// `$name` or `${name}`.
    Variable(&'a str),
    /// `${env:NAME}`.
    Environment(&'a str),
    /// `${lua:name}`.
    Lua(&'a str),
}

fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Splits a value into literal text and references.
pub fn pieces(value: &str) -> Result<Vec<Piece<'_>>, String> {
    let bytes = value.as_bytes();
    let mut pieces = Vec::new();
    let mut text_start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        let (piece, end) = match bytes.get(index + 1) {
            Some(b'$') => (Piece::Text("$"), index + 2),
            Some(b'{') => {
                let close = value[index + 2..]
                    .find('}')
                    .map(|offset| index + 2 + offset)
                    .ok_or_else(|| {
                        format!(
                            "the variable at {:?} is not closed with '}}'",
                            &value[index..]
                        )
                    })?;
                let inner = &value[index + 2..close];
                let named = |name: &str| {
                    !name.is_empty()
                        && is_name_start(name.as_bytes()[0])
                        && name.bytes().all(is_name)
                };
                let piece = match inner.strip_prefix("env:") {
                    Some(name) if !name.is_empty() && name.bytes().all(is_name) => {
                        Piece::Environment(name)
                    }
                    Some(_) => {
                        return Err(format!(
                            "{inner:?} is not a valid environment variable name"
                        ))
                    }
                    None if inner.starts_with("lua:") => match &inner[4..] {
                        name if named(name) => Piece::Lua(name),
                        _ => return Err(format!("{inner:?} is not a valid variable name")),
                    },
                    None if !inner.is_empty()
                        && is_name_start(inner.as_bytes()[0])
                        && inner.bytes().all(is_name) =>
                    {
                        Piece::Variable(inner)
                    }
                    None => return Err(format!("{inner:?} is not a valid variable name")),
                };
                (piece, close + 1)
            }
            Some(&next) if is_name_start(next) => {
                let end = index
                    + 1
                    + bytes[index + 1..]
                        .iter()
                        .take_while(|byte| is_name(**byte))
                        .count();
                (Piece::Variable(&value[index + 1..end]), end)
            }
            _ => {
                index += 1;
                continue;
            }
        };
        if text_start < index {
            pieces.push(Piece::Text(&value[text_start..index]));
        }
        pieces.push(piece);
        index = end;
        text_start = end;
    }
    if text_start < value.len() {
        pieces.push(Piece::Text(&value[text_start..]));
    }
    Ok(pieces)
}

/// `value` written so that reading it back yields the same text: every `$`
/// that would start a reference is doubled.
pub fn escape(value: &str) -> std::borrow::Cow<'_, str> {
    let bytes = value.as_bytes();
    let starts_reference = |index: usize| {
        bytes[index] == b'$'
            && bytes
                .get(index + 1)
                .is_some_and(|next| *next == b'$' || *next == b'{' || is_name_start(*next))
    };
    if !(0..bytes.len()).any(starts_reference) {
        return std::borrow::Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len() + 2);
    for (index, c) in value.char_indices() {
        if starts_reference(index) {
            out.push('$');
        }
        out.push(c);
    }
    std::borrow::Cow::Owned(out)
}

/// The consistent-hash key a request variable selects, in the model's form:
/// `client_ip`, `uri`, `header:<name>` or `cookie:<name>`. The model's own
/// forms are accepted as well.
pub fn hash_key(value: &str) -> Option<String> {
    let key = match value {
        "$client_ip" | "client_ip" => "client_ip".to_owned(),
        "$uri" | "uri" => "uri".to_owned(),
        other => {
            if let Some(name) = other.strip_prefix("$http_") {
                format!("header:{}", name.replace('_', "-"))
            } else if let Some(name) = other.strip_prefix("$cookie_") {
                format!("cookie:{name}")
            } else if other.starts_with("header:") || other.starts_with("cookie:") {
                other.to_owned()
            } else {
                return None;
            }
        }
    };
    let token = key.split_once(':').map_or("", |(_, token)| token);
    (key.contains(':') != token.is_empty()).then_some(key)
}

/// A hash key written as a request variable when that reads back the same.
pub fn print_hash_key(key: &str) -> String {
    let variable = match key.split_once(':') {
        None => format!("${key}"),
        Some(("header", name))
            if !name.contains('_') && name.bytes().all(|byte| is_name(byte) || byte == b'-') =>
        {
            format!("$http_{}", name.to_ascii_lowercase().replace('-', "_"))
        }
        Some(("cookie", name)) if name.bytes().all(is_name) => format!("$cookie_{name}"),
        Some(_) => return key.to_owned(),
    };
    if hash_key(&variable).as_deref() == Some(key) {
        variable
    } else {
        key.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_are_split_from_text() {
        assert_eq!(
            pieces("https://${host}$uri?from=$client_ip").unwrap(),
            vec![
                Piece::Text("https://"),
                Piece::Variable("host"),
                Piece::Variable("uri"),
                Piece::Text("?from="),
                Piece::Variable("client_ip"),
            ]
        );
        assert_eq!(
            pieces("${env:PINGORA_PANEL_DSL_HOST}:80").unwrap()[0],
            Piece::Environment("PINGORA_PANEL_DSL_HOST")
        );
        assert_eq!(
            pieces("^/api/v[0-9]+$").unwrap(),
            vec![Piece::Text("^/api/v[0-9]+$")]
        );
        assert_eq!(pieces("cost $5").unwrap(), vec![Piece::Text("cost $5")]);
        assert_eq!(
            pieces("a$$b").unwrap(),
            vec![Piece::Text("a"), Piece::Text("$"), Piece::Text("b")]
        );
        for value in ["$HOME", "a$$b", "${x}", "cost $5", "end$", "$"] {
            let escaped = escape(value);
            let read: String = pieces(&escaped)
                .unwrap()
                .into_iter()
                .map(|piece| match piece {
                    Piece::Text(text) => text,
                    other => panic!("{value:?} read back as {other:?}"),
                })
                .collect();
            assert_eq!(read, value);
        }
        assert!(pieces("${host").is_err());
        assert!(pieces("${1x}").is_err());
        assert!(pieces("${env:}").is_err());
    }

    #[test]
    fn hash_keys_map_onto_the_model() {
        assert_eq!(hash_key("$client_ip").as_deref(), Some("client_ip"));
        assert_eq!(hash_key("$http_x_user").as_deref(), Some("header:x-user"));
        assert_eq!(
            hash_key("$cookie_session").as_deref(),
            Some("cookie:session")
        );
        assert_eq!(hash_key("header:X_Odd").as_deref(), Some("header:X_Odd"));
        assert_eq!(hash_key("$host"), None);
        assert_eq!(hash_key("header:"), None);
        for key in [
            "client_ip",
            "uri",
            "header:x-user",
            "header:X_Odd",
            "cookie:session",
            "cookie:a-b",
        ] {
            assert_eq!(
                hash_key(&print_hash_key(key)).as_deref(),
                Some(key),
                "{key}"
            );
        }
        assert_eq!(print_hash_key("header:x-user"), "$http_x_user");
        assert_eq!(print_hash_key("cookie:a-b"), "cookie:a-b");
    }
}
