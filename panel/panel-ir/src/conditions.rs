//! Conditions a route's requests meet beyond its path (ADR 0036). A route
//! takes a request only when every one of its conditions holds.

use panel_domain::NormalizedHost;
use serde::{Deserialize, Serialize};

/// What a snapshot whose routes have conditions requires of a gateway.
pub const ROUTE_CONDITIONS_CAPABILITY: &str = "route.conditions";

/// A condition on a request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteCondition {
    /// One of the methods, compared case-sensitively (RFC 9110 §9.1).
    Method {
        methods: Vec<String>,
    },
    /// One of the hosts, or of the `*.parent` wildcards of one label.
    Host {
        hosts: Vec<NormalizedHost>,
    },
    /// A header field, by its case-insensitive name; repeated field lines
    /// are tested as their combined value (RFC 9110 §5.3).
    Header {
        name: String,
        test: ValueTest,
    },
    /// A query parameter decoded as `application/x-www-form-urlencoded`;
    /// a repeated parameter holds when any of its values does.
    Query {
        name: String,
        test: ValueTest,
    },
    /// A cookie among the pairs of every `Cookie` field (RFC 6265 §5.4).
    Cookie {
        name: String,
        test: ValueTest,
    },
    /// The client's address, after trusted proxies, in one of the networks,
    /// such as `10.0.0.0/8` or `2001:db8::1`.
    Client {
        networks: Vec<String>,
    },
    UserAgent {
        test: ValueTest,
    },
    Referer {
        test: ValueTest,
    },
    /// The request's media type, without parameters and ignoring case,
    /// against types such as `application/json` or ranges such as `text/*`.
    ContentType {
        types: Vec<String>,
    },
    /// Every one of the conditions.
    All {
        conditions: Vec<RouteCondition>,
    },
    /// At least one of the conditions.
    Any {
        conditions: Vec<RouteCondition>,
    },
    /// Not the condition.
    Not {
        condition: Box<RouteCondition>,
    },
}

/// A test of a field, parameter or cookie.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueTest {
    Present,
    Absent,
    Equals {
        value: String,
        #[serde(default, skip_serializing_if = "is_false")]
        ignore_case: bool,
    },
    Prefix {
        value: String,
        #[serde(default, skip_serializing_if = "is_false")]
        ignore_case: bool,
    },
    Suffix {
        value: String,
        #[serde(default, skip_serializing_if = "is_false")]
        ignore_case: bool,
    },
    Contains {
        value: String,
        #[serde(default, skip_serializing_if = "is_false")]
        ignore_case: bool,
    },
    Regex {
        pattern: String,
        #[serde(default, skip_serializing_if = "is_false")]
        ignore_case: bool,
    },
}

fn is_false(value: &bool) -> bool {
    !value
}

impl RouteCondition {
    /// The conditions this one is made of, for groups and negations.
    pub fn children(&self) -> &[RouteCondition] {
        match self {
            Self::All { conditions } | Self::Any { conditions } => conditions,
            Self::Not { condition } => std::slice::from_ref(condition),
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_travel_as_tagged_trees() {
        let condition = RouteCondition::Any {
            conditions: vec![
                RouteCondition::Header {
                    name: "x-canary".into(),
                    test: ValueTest::Equals {
                        value: "1".into(),
                        ignore_case: false,
                    },
                },
                RouteCondition::Not {
                    condition: Box::new(RouteCondition::Client {
                        networks: vec!["192.0.2.0/24".into()],
                    }),
                },
            ],
        };
        let encoded = serde_json::to_value(&condition).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({
                "kind": "any",
                "conditions": [
                    { "kind": "header", "name": "x-canary", "test": { "op": "equals", "value": "1" } },
                    { "kind": "not", "condition": { "kind": "client", "networks": ["192.0.2.0/24"] } },
                ],
            })
        );
        assert_eq!(
            serde_json::from_value::<RouteCondition>(encoded).unwrap(),
            condition
        );
        assert_eq!(condition.children().len(), 2);
    }
}
