//! `route` blocks, `location` blocks as nginx writes them, and the actions
//! of servers and routes.

use super::{ActionDraft, Expansion, Lowerer, RouteDraft};
use crate::{codes, schema::Context, values::Params};
use panel_config_model::{Action, MatchKind, Route, RouteMatch, Site};
use panel_dsl::Directive;
use std::collections::BTreeSet;

impl<'a> Lowerer<'a> {
    pub(super) fn route(
        &mut self,
        file: &str,
        directive: &Directive,
        depth: usize,
        site: &Site,
    ) -> Option<RouteDraft> {
        let location = directive.name.value == "location";
        let id = self.identity(file, directive, depth);
        let mut draft = RouteDraft {
            route: Route {
                lua: Default::default(),
                named: None,
                id,
                name: None,
                enabled: true,
                priority: 0,
                matcher: RouteMatch {
                    kind: MatchKind::Prefix,
                    path: "/".into(),
                    host: None,
                    conditions: Vec::new(),
                },
                action: placeholder_action(),
                security_policy_id: None,
                http_policy_id: None,
                access_log: Default::default(),
                rewrites: Vec::new(),
                internal: false,
            },
            priority_set: false,
            action: None,
            origin: Self::origin(file, directive, depth),
        };
        let mut matched = false;
        let mut conditions = Vec::new();
        if location {
            let (modifier, path) = match directive.args.as_slice() {
                [path] => (None, path),
                [modifier, path] => (Some(modifier), path),
                _ => return None,
            };
            let named = modifier.is_none() && path.value.starts_with('@');
            let (kind, prefix) = match modifier.map(|modifier| modifier.value.as_str()) {
                None | Some("^~") => (MatchKind::Prefix, ""),
                Some("=") => (MatchKind::Exact, ""),
                Some("~") => (MatchKind::Regex, ""),
                Some("~*") => (MatchKind::Regex, "(?i)"),
                Some(other) => {
                    let span = modifier.map_or(directive.span, |modifier| modifier.span);
                    self.error(
                        file,
                        span,
                        codes::TYPE,
                        format!("{other:?} is not a location modifier"),
                    );
                    return None;
                }
            };
            if named {
                draft.route.named = Some(path.value[1..].to_owned());
            } else {
                draft.route.matcher = RouteMatch {
                    kind,
                    path: format!("{prefix}{}", path.value),
                    host: None,
                    conditions: Vec::new(),
                };
            }
            matched = true;
        } else if let Some(name) = directive.args.first() {
            draft.route.name = Some(Self::literal(name));
        }
        let block = directive.block()?;
        let variables = self.with_scope(
            format!("sites/{}/routes/{}", site.id, draft.route.id),
            |lowerer| {
                let mut seen = BTreeSet::new();
                lowerer.each(
                    file,
                    &block.directives,
                    Context::Route,
                    depth + 1,
                    &mut seen,
                    &mut |lowerer, file, directive, spec, _| {
                        let arg = &directive.args.first();
                        match spec.name {
                            "id" => {}
                            "match" => {
                                let params = Params::split(&directive.args);
                                lowerer.only_params(file, &params, &["host"]);
                                let [kind_arg, path_arg] = params.positional.as_slice() else {
                                    lowerer.error_with_help(
                                        file,
                                        directive.span,
                                        codes::ARGUMENTS,
                                        "a match is a kind and a path",
                                        format!("write it as `{}`", spec.syntax),
                                    );
                                    return;
                                };
                                if kind_arg.value == "named" {
                                    if let Some(name) = lowerer.value(file, path_arg) {
                                        draft.route.named = Some(name);
                                        matched = true;
                                    }
                                    return;
                                }
                                let kind = match kind_arg.value.as_str() {
                                    "exact" => MatchKind::Exact,
                                    "prefix" => MatchKind::Prefix,
                                    "glob" => MatchKind::Glob,
                                    "regex" => MatchKind::Regex,
                                    other => {
                                        lowerer.error(
                                            file,
                                            kind_arg.span,
                                            codes::TYPE,
                                            format!(
                                                "{other:?} is not exact, prefix, glob, regex or named"
                                            ),
                                        );
                                        return;
                                    }
                                };
                                let path = if kind == MatchKind::Regex {
                                    path_arg.value.clone()
                                } else {
                                    match lowerer.value(file, path_arg) {
                                        Some(path) => path,
                                        None => return,
                                    }
                                };
                                let host = match params.named.get("host") {
                                    Some((value, arg)) => {
                                        let Some(value) =
                                            lowerer.expand(file, arg, value, Expansion::Text)
                                        else {
                                            return;
                                        };
                                        lowerer.host(file, arg, &value)
                                    }
                                    None => None,
                                };
                                draft.route.matcher = RouteMatch {
                                    kind,
                                    path,
                                    host,
                                    conditions: Vec::new(),
                                };
                                matched = true;
                            }
                            "priority" => {
                                if let Some(arg) = arg {
                                    if let Some(value) = lowerer.value(file, arg) {
                                        draft.route.priority = lowerer
                                            .number(file, arg, &value, "a whole number")
                                            .unwrap_or_default();
                                        draft.priority_set = true;
                                    }
                                }
                            }
                            "enabled" => {
                                draft.route.enabled = arg
                                    .and_then(|arg| lowerer.bool_arg(file, arg))
                                    .unwrap_or(true)
                            }
                            "security_policy" => {
                                draft.route.security_policy_id = arg.map(Self::literal)
                            }
                            "http_policy" => draft.route.http_policy_id = arg.map(Self::literal),
                            name if super::conditions::CONDITIONS.contains(&name) => {
                                if let Some(condition) =
                                    lowerer.condition(file, directive, depth + 1)
                                {
                                    conditions.push(condition);
                                }
                            }
                            "access_log" => {
                                lowerer.access_log(file, directive, &mut draft.route.access_log)
                            }
                            name if super::lua::SCOPE.contains(&name) => lowerer.lua_scope(
                                file,
                                directive,
                                &mut draft.route.lua,
                                "the route",
                            ),
                            name if super::lua::inert(name).is_some() => {
                                lowerer.lua_inert(file, directive)
                            }
                            "log_field" => {
                                lowerer.log_field(file, directive, &mut draft.route.access_log)
                            }
                            name if super::rewrite::REWRITES.contains(&name) => {
                                if let Some(rule) = lowerer.rewrite_rule(file, directive, name) {
                                    draft.route.rewrites.push(rule);
                                }
                            }
                            "internal" => draft.route.internal = true,
                            action => {
                                let Some(found) = lowerer.action(file, directive, action) else {
                                    return;
                                };
                                if draft.action.is_some() {
                                    lowerer.error(
                                        file,
                                        directive.name.span,
                                        codes::DUPLICATE,
                                        "the route already has an action",
                                    );
                                } else {
                                    draft.action = Some(found);
                                }
                            }
                        }
                    },
                );
            },
        );
        draft.route.lua.variables = variables;
        draft.route.matcher.conditions = conditions;
        if !matched {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                "the route has no match",
                "add `match prefix /path;`",
            );
        }
        if draft.action.is_none() {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                "the route has no action",
                "add one of proxy, root, return, respond, internal_redirect or content_by_lua_block",
            );
        }
        self.origins.insert(
            format!("sites/{}/routes/{}", site.id, draft.route.id),
            draft.origin.clone(),
        );
        Some(draft)
    }

    pub(super) fn action(
        &mut self,
        file: &str,
        directive: &Directive,
        name: &str,
    ) -> Option<ActionDraft> {
        let params = Params::split(&directive.args);
        let first = params.positional.first().copied();
        match name {
            "proxy" | "proxy_pass" => {
                let arg = first?;
                let value = Self::literal(arg);
                let upstream = if name == "proxy_pass" {
                    value
                        .strip_prefix("http://")
                        .or_else(|| value.strip_prefix("https://"))
                        .unwrap_or(&value)
                        .trim_end_matches('/')
                        .to_owned()
                } else {
                    value
                };
                Some(ActionDraft::Proxy {
                    upstream,
                    file: file.to_owned(),
                    span: arg.span,
                })
            }
            "root" => {
                self.only_params(file, &params, &["index", "spa"]);
                let root = self.value(file, first?)?;
                let mut index_files = vec!["index.html".to_owned()];
                if let Some((value, arg)) = params.named.get("index") {
                    index_files = self
                        .expand(file, arg, value, Expansion::Text)?
                        .split(',')
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
                let spa_fallback = match params.named.get("spa") {
                    Some((value, arg)) => self.flag(file, arg, value).unwrap_or_default(),
                    None => false,
                };
                Some(ActionDraft::Ready(Action::Static {
                    root,
                    index_files,
                    spa_fallback,
                }))
            }
            "return" => {
                self.only_params(file, &params, &["preserve_path"]);
                let [code_arg, location_arg] = params.positional.as_slice() else {
                    self.error_with_help(
                        file,
                        directive.span,
                        codes::ARGUMENTS,
                        "a redirect is a status and a URL",
                        "write `return 308 https://example.com;`",
                    );
                    return None;
                };
                let status: u16 =
                    self.number(file, code_arg, &code_arg.value, "a redirect status")?;
                if !matches!(status, 301 | 302 | 303 | 307 | 308) {
                    self.error_with_help(
                        file,
                        code_arg.span,
                        codes::TYPE,
                        format!("{status} is not a redirect status"),
                        "use 301, 302, 303, 307 or 308, or respond for other statuses",
                    );
                    return None;
                }
                let location = self.expand(
                    file,
                    location_arg,
                    &location_arg.value.clone(),
                    Expansion::Template,
                )?;
                let preserve_path = match params.named.get("preserve_path") {
                    Some((value, arg)) => self.flag(file, arg, value).unwrap_or(true),
                    None => true,
                };
                Some(ActionDraft::Ready(Action::Redirect {
                    location,
                    status,
                    preserve_path,
                }))
            }
            "respond" => {
                self.only_params(file, &params, &["body", "type", "retry_after"]);
                let code_arg = first?;
                let status: u16 = self.number(file, code_arg, &code_arg.value, "an HTTP status")?;
                if !(100..=599).contains(&status) {
                    self.error(
                        file,
                        code_arg.span,
                        codes::TYPE,
                        format!("{status} is not an HTTP status"),
                    );
                    return None;
                }
                if let Some(extra) = params.positional.get(1) {
                    self.error_with_help(
                        file,
                        extra.span,
                        codes::ARGUMENTS,
                        format!("unexpected {:?}", extra.value),
                        "write the body as body=<text>",
                    );
                }
                let body = match params.named.get("body") {
                    Some((value, arg)) => {
                        Some(self.expand(file, arg, value, Expansion::Template)?)
                    }
                    None => None,
                };
                let content_type = match params.named.get("type") {
                    Some((value, arg)) => Some(self.expand(file, arg, value, Expansion::Text)?),
                    None => None,
                };
                let retry_after_seconds = match params.named.get("retry_after") {
                    Some((value, arg)) => {
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
                    None => None,
                };
                Some(ActionDraft::Ready(Action::Respond {
                    status,
                    body,
                    content_type,
                    retry_after_seconds,
                }))
            }
            "content_by_lua_block" | "content_by_lua_file" => {
                Some(ActionDraft::Ready(Action::Lua {
                    code: self.lua_code(file, directive)?,
                }))
            }
            "internal_redirect" => {
                let arg = first?;
                let target = if arg.value.starts_with('@') {
                    Self::literal(arg)
                } else {
                    self.expand(file, arg, &arg.value.clone(), Expansion::Template)?
                };
                Some(ActionDraft::Ready(Action::InternalRedirect { target }))
            }
            _ => unreachable!("the schema has no other actions"),
        }
    }
}

pub(super) fn placeholder_action() -> Action {
    Action::Respond {
        status: 503,
        body: None,
        content_type: None,
        retry_after_seconds: None,
    }
}
