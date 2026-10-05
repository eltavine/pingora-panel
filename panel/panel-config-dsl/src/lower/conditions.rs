//! Route conditions (ADR 0036): `method`, `host`, `header`, `query`,
//! `cookie`, `client`, `user_agent`, `referer` and `content_type`, grouped
//! with `any`, `all` and `not`.

use super::Lowerer;
use crate::{codes, schema::Context};
use panel_config_model::{RouteCondition, ValueTest};
use panel_domain::IpNetwork;
use panel_dsl::{Argument, Directive};
use std::collections::BTreeSet;

/// The directives that are conditions.
pub(super) const CONDITIONS: &[&str] = &[
    "method",
    "host",
    "header",
    "query",
    "cookie",
    "client",
    "user_agent",
    "referer",
    "content_type",
    "any",
    "all",
    "not",
];

const TEST_SYNTAX: &str = "present, absent, or =, ^=, $=, *=, ~ or ~* and a value";

impl<'a> Lowerer<'a> {
    /// The condition a directive of a route, `any`, `all` or `not` writes.
    pub(super) fn condition(
        &mut self,
        file: &str,
        directive: &Directive,
        depth: usize,
    ) -> Option<RouteCondition> {
        let args = directive.args.as_slice();
        Some(match directive.name.value.as_str() {
            "method" => RouteCondition::Method {
                methods: args
                    .iter()
                    .map(|arg| {
                        let method = self.value(file, arg)?;
                        if panel_config_model::token(&method) {
                            Some(method)
                        } else {
                            self.error(
                                file,
                                arg.span,
                                codes::TYPE,
                                format!("{method:?} is not a method such as GET or POST"),
                            );
                            None
                        }
                    })
                    .collect::<Option<_>>()?,
            },
            "host" => RouteCondition::Host {
                hosts: args
                    .iter()
                    .map(|arg| {
                        let value = self.value(file, arg)?;
                        self.host(file, arg, &value)
                    })
                    .collect::<Option<_>>()?,
            },
            kind @ ("header" | "query" | "cookie") => {
                let [name, rest @ ..] = args else {
                    return None;
                };
                let name = self.value(file, name).and_then(|value| {
                    let fine = if kind == "query" {
                        !value.is_empty()
                    } else {
                        panel_config_model::token(&value)
                    };
                    if fine {
                        Some(value)
                    } else {
                        self.error(
                            file,
                            name.span,
                            codes::TYPE,
                            format!("{value:?} is not a {kind} name"),
                        );
                        None
                    }
                })?;
                let test = self.value_test(file, directive, rest)?;
                match kind {
                    "header" => RouteCondition::Header { name, test },
                    "query" => RouteCondition::Query { name, test },
                    _ => RouteCondition::Cookie { name, test },
                }
            }
            "user_agent" => RouteCondition::UserAgent {
                test: self.value_test(file, directive, args)?,
            },
            "referer" => RouteCondition::Referer {
                test: self.value_test(file, directive, args)?,
            },
            "client" => RouteCondition::Client {
                networks: args
                    .iter()
                    .map(|arg| {
                        let network = self.value(file, arg)?;
                        match IpNetwork::new(&network) {
                            Ok(_) => Some(network),
                            Err(error) => {
                                self.error(file, arg.span, codes::TYPE, error.to_string());
                                None
                            }
                        }
                    })
                    .collect::<Option<_>>()?,
            },
            "content_type" => RouteCondition::ContentType {
                types: args
                    .iter()
                    .map(|arg| {
                        let media = self.value(file, arg)?;
                        let fine = media.split_once('/').is_some_and(|(kind, subtype)| {
                            (kind == "*" && subtype == "*")
                                || (panel_config_model::token(kind)
                                    && (subtype == "*" || panel_config_model::token(subtype)))
                        });
                        if fine {
                            Some(media.to_ascii_lowercase())
                        } else {
                            self.error(
                                file,
                                arg.span,
                                codes::TYPE,
                                format!(
                                    "{media:?} is not a media type such as application/json or text/*"
                                ),
                            );
                            None
                        }
                    })
                    .collect::<Option<_>>()?,
            },
            group @ ("any" | "all" | "not") => {
                let block = directive.block()?;
                let mut inner = Vec::new();
                let mut seen = BTreeSet::new();
                self.each(
                    file,
                    &block.directives,
                    Context::Conditions,
                    depth + 1,
                    &mut seen,
                    &mut |lowerer, file, directive, _, _| {
                        if let Some(condition) = lowerer.condition(file, directive, depth + 1) {
                            inner.push(condition);
                        }
                    },
                );
                if inner.is_empty() {
                    self.error(
                        file,
                        directive.span,
                        codes::ARGUMENTS,
                        format!("'{group}' holds no condition"),
                    );
                    return None;
                }
                match group {
                    "any" => RouteCondition::Any { conditions: inner },
                    "all" => RouteCondition::All { conditions: inner },
                    _ => RouteCondition::Not {
                        condition: Box::new(if inner.len() == 1 {
                            inner.remove(0)
                        } else {
                            RouteCondition::All { conditions: inner }
                        }),
                    },
                }
            }
            _ => return None,
        })
    }

    /// `present`, `absent`, or an operator and a value, then `ignore_case`.
    fn value_test(
        &mut self,
        file: &str,
        directive: &Directive,
        args: &[Argument],
    ) -> Option<ValueTest> {
        let problem = |lowerer: &mut Self, span| {
            lowerer.error_with_help(
                file,
                span,
                codes::ARGUMENTS,
                format!("expected {TEST_SYNTAX}"),
                format!(
                    "write it as `{} {}`",
                    directive.name.value, "present|absent|<op> <value> [ignore_case]"
                ),
            );
        };
        match args {
            [word] if word.value == "present" => Some(ValueTest::Present),
            [word] if word.value == "absent" => Some(ValueTest::Absent),
            [operator, value, flag @ ..] if flag.len() <= 1 => {
                let ignore_case = match flag {
                    [] => false,
                    [flag] if flag.value == "ignore_case" => true,
                    [flag] => {
                        problem(self, flag.span);
                        return None;
                    }
                    _ => unreachable!("at most one flag"),
                };
                let regex = |lowerer: &mut Self, ignore_case| {
                    let pattern = value.value.clone();
                    if let Some(error) = panel_config_model::route_regex_error(&pattern) {
                        lowerer.error(
                            file,
                            value.span,
                            codes::TYPE,
                            format!("the regular expression does not compile: {error}"),
                        );
                        return None;
                    }
                    Some(ValueTest::Regex {
                        pattern,
                        ignore_case,
                    })
                };
                Some(match operator.value.as_str() {
                    "~" => regex(self, ignore_case)?,
                    "~*" => regex(self, true)?,
                    op @ ("=" | "^=" | "$=" | "*=") => {
                        let value = self.value(file, value)?;
                        match op {
                            "=" => ValueTest::Equals { value, ignore_case },
                            "^=" => ValueTest::Prefix { value, ignore_case },
                            "$=" => ValueTest::Suffix { value, ignore_case },
                            _ => ValueTest::Contains { value, ignore_case },
                        }
                    }
                    _ => {
                        problem(self, operator.span);
                        return None;
                    }
                })
            }
            _ => {
                problem(self, directive.span);
                None
            }
        }
    }
}
