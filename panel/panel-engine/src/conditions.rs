//! Checks of route conditions (ADR 0036): names are tokens, networks and
//! media types parse, regular expressions compile within the route limit,
//! and how many there are and how deeply they nest stays bounded.

use crate::route_regex_error;
use panel_domain::IpNetwork;
use panel_ir::{RouteCondition, ValueTest};

/// How deeply one route's conditions nest.
pub const MOST_CONDITION_DEPTH: usize = 8;
/// How many conditions one route has, groups included.
pub const MOST_CONDITIONS: usize = 64;
/// How long a value a condition compares with is.
const MOST_VALUE_BYTES: usize = 4096;
const MOST_PATTERN_BYTES: usize = 1024;
const MOST_NAME_BYTES: usize = 256;

/// What is wrong with `conditions`, one line each.
pub fn problems(conditions: &[RouteCondition]) -> Vec<String> {
    let mut found = Vec::new();
    let mut count = 0;
    for condition in conditions {
        check(condition, 1, &mut count, &mut found);
    }
    if count > MOST_CONDITIONS {
        found.push(format!(
            "has {count} conditions, more than {MOST_CONDITIONS}"
        ));
    }
    found
}

fn check(condition: &RouteCondition, depth: usize, count: &mut usize, found: &mut Vec<String>) {
    *count += 1;
    if depth > MOST_CONDITION_DEPTH {
        found.push(format!(
            "nests conditions deeper than {MOST_CONDITION_DEPTH}"
        ));
        return;
    }
    match condition {
        RouteCondition::Method { methods } => {
            if methods.is_empty() {
                found.push("has a method condition naming no method".into());
            }
            for method in methods.iter().filter(|method| !token(method)) {
                found.push(format!("names {method:?}, which is not a method"));
            }
        }
        RouteCondition::Host { hosts } => {
            if hosts.is_empty() {
                found.push("has a host condition naming no host".into());
            }
        }
        RouteCondition::Header { name, test } => {
            if !token(name) {
                found.push(format!("names {name:?}, which is not a header field name"));
            }
            check_test(test, found);
        }
        RouteCondition::Query { name, test } => {
            if name.is_empty() || name.len() > MOST_NAME_BYTES {
                found.push(format!(
                    "names a query parameter of {} bytes; it needs 1..={MOST_NAME_BYTES}",
                    name.len()
                ));
            }
            check_test(test, found);
        }
        RouteCondition::Cookie { name, test } => {
            if !token(name) {
                found.push(format!("names {name:?}, which is not a cookie name"));
            }
            check_test(test, found);
        }
        RouteCondition::Client { networks } => {
            if networks.is_empty() {
                found.push("has a client condition naming no network".into());
            }
            for network in networks {
                if let Err(error) = IpNetwork::new(network) {
                    found.push(error.to_string());
                }
            }
        }
        RouteCondition::UserAgent { test } | RouteCondition::Referer { test } => {
            check_test(test, found);
        }
        RouteCondition::ContentType { types } => {
            if types.is_empty() {
                found.push("has a content type condition naming no type".into());
            }
            for media in types.iter().filter(|media| !media_range(media)) {
                found.push(format!(
                    "names {media:?}, which is not a media type such as application/json or text/*"
                ));
            }
        }
        RouteCondition::All { conditions } | RouteCondition::Any { conditions } => {
            if conditions.is_empty() {
                found.push("has an empty group of conditions".into());
            }
            for condition in conditions {
                check(condition, depth + 1, count, found);
            }
        }
        RouteCondition::Not { condition } => check(condition, depth + 1, count, found),
    }
}

fn check_test(test: &ValueTest, found: &mut Vec<String>) {
    match test {
        ValueTest::Present | ValueTest::Absent => {}
        ValueTest::Equals { value, .. }
        | ValueTest::Prefix { value, .. }
        | ValueTest::Suffix { value, .. }
        | ValueTest::Contains { value, .. } => {
            if value.len() > MOST_VALUE_BYTES {
                found.push(format!(
                    "compares with a value of {} bytes, more than {MOST_VALUE_BYTES}",
                    value.len()
                ));
            }
        }
        ValueTest::Regex { pattern, .. } => {
            if pattern.is_empty() || pattern.len() > MOST_PATTERN_BYTES {
                found.push(format!(
                    "has a condition regex of {} bytes; it needs 1..={MOST_PATTERN_BYTES}",
                    pattern.len()
                ));
            } else if let Some(error) = route_regex_error(pattern) {
                found.push(format!(
                    "has a condition regex that does not compile: {error}"
                ));
            }
        }
    }
}

/// A token of RFC 9110 §5.6.2, as methods, field names and cookie names are.
pub fn token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// `type/subtype`, `type/*` or `*/*` (RFC 9110 §8.3.1, §12.5.1).
fn media_range(value: &str) -> bool {
    match value.split_once('/') {
        Some(("*", "*")) => true,
        Some((kind, "*")) => token(kind),
        Some((kind, subtype)) => token(kind) && token(subtype),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, test: ValueTest) -> RouteCondition {
        RouteCondition::Header {
            name: name.into(),
            test,
        }
    }

    #[test]
    fn well_formed_conditions_pass() {
        let conditions = vec![
            RouteCondition::Method {
                methods: vec!["GET".into(), "PURGE".into()],
            },
            header(
                "x-env",
                ValueTest::Regex {
                    pattern: "^(staging|qa)$".into(),
                    ignore_case: true,
                },
            ),
            RouteCondition::Client {
                networks: vec!["10.0.0.0/8".into(), "2001:db8::1".into()],
            },
            RouteCondition::ContentType {
                types: vec!["application/json".into(), "text/*".into(), "*/*".into()],
            },
            RouteCondition::Not {
                condition: Box::new(RouteCondition::Cookie {
                    name: "session".into(),
                    test: ValueTest::Absent,
                }),
            },
        ];
        assert_eq!(problems(&conditions), Vec::<String>::new());
    }

    #[test]
    fn malformed_conditions_are_named() {
        let conditions = vec![
            RouteCondition::Method {
                methods: vec!["GET POST".into()],
            },
            header("x env", ValueTest::Present),
            RouteCondition::Client {
                networks: vec!["10.0.0.0/33".into()],
            },
            RouteCondition::ContentType {
                types: vec!["json".into()],
            },
            RouteCondition::Any {
                conditions: Vec::new(),
            },
            header(
                "x-id",
                ValueTest::Regex {
                    pattern: "(".into(),
                    ignore_case: false,
                },
            ),
        ];
        let found = problems(&conditions);
        assert_eq!(found.len(), 6, "{found:#?}");
        assert!(found[0].contains("not a method"));
        assert!(found[5].contains("does not compile"));
    }

    #[test]
    fn conditions_stay_bounded() {
        let mut deep = RouteCondition::Method {
            methods: vec!["GET".into()],
        };
        for _ in 0..MOST_CONDITION_DEPTH {
            deep = RouteCondition::Not {
                condition: Box::new(deep),
            };
        }
        assert!(problems(&[deep])[0].contains("deeper than"));
        let many = vec![header("x-a", ValueTest::Present); MOST_CONDITIONS + 1];
        assert!(problems(&many)[0].contains("more than"));
    }
}
