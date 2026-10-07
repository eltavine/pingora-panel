//! `cache_policy` blocks and `cache_store` (ADR 0043): what responses are
//! cached by and for how long, and how much the cache keeps.

use super::Lowerer;
use crate::{
    codes,
    schema::Context,
    values::{self, Params},
};
use panel_config_model::{CachePolicy, RouteCondition};
use panel_dsl::{Argument, Directive};
use std::collections::BTreeSet;

/// What a server or route writes in place of a policy to cache nothing.
pub(crate) const OFF: &str = "off";

impl Lowerer<'_> {
    pub(super) fn cache_policy(&mut self, file: &str, directive: &Directive, depth: usize) {
        let name = &directive.args[0];
        let id = Self::literal(name);
        if id == OFF {
            self.error_with_help(
                file,
                name.span,
                codes::TYPE,
                "a cache policy is not named off",
                "off keeps servers and routes out of the cache; name the policy otherwise",
            );
            return;
        }
        if self
            .cache_policies
            .iter()
            .any(|(policy, _)| policy.id == id)
        {
            self.error(
                file,
                name.span,
                codes::DUPLICATE,
                format!("cache policy {id:?} is defined twice"),
            );
            return;
        }
        let mut policy = CachePolicy::new(id);
        if let Some(block) = directive.block() {
            self.with_scope(format!("cache-policies/{}", policy.id), |lowerer| {
                let mut seen = BTreeSet::new();
                lowerer.each(
                    file,
                    &block.directives,
                    Context::CachePolicy,
                    depth + 1,
                    &mut seen,
                    &mut |lowerer, file, directive, spec, depth| {
                        let arg = &directive.args;
                        match spec.name {
                            "enabled" => {
                                policy.enabled = lowerer.bool_arg(file, &arg[0]).unwrap_or(true);
                            }
                            "key" => policy.key = lowerer.template(file, &arg[0]),
                            "valid" => lowerer.valid(file, directive, &mut policy),
                            "vary" => policy.vary_headers.extend(
                                arg.iter()
                                    .map(|name| Self::literal(name).to_ascii_lowercase()),
                            ),
                            "honor_origin" => {
                                policy.honor_origin =
                                    lowerer.bool_arg(file, &arg[0]).unwrap_or(true);
                            }
                            "bypass" => lowerer.bypass(file, directive, depth, &mut policy.bypass),
                            "stale_while_revalidate" => {
                                policy.stale_while_revalidate_seconds =
                                    lowerer.short_seconds(file, &arg[0]).unwrap_or_default();
                            }
                            "stale_if_error" => {
                                policy.stale_if_error_seconds =
                                    lowerer.short_seconds(file, &arg[0]).unwrap_or_default();
                            }
                            "max_object_size" => {
                                policy.max_object_bytes = lowerer.size(file, &arg[0])
                            }
                            "status_header" => {
                                policy.status_header =
                                    lowerer.bool_arg(file, &arg[0]).unwrap_or(true);
                            }
                            _ => unreachable!("the schema allows nothing else in cache_policy"),
                        }
                    },
                );
            });
        }
        let origin = Self::origin(file, directive, depth);
        self.origins
            .insert(format!("cache-policies/{}", policy.id), origin.clone());
        self.cache_policies.push((policy, origin));
    }

    /// `valid <duration>;` or `valid <status> ... <duration>;`.
    fn valid(&mut self, file: &str, directive: &Directive, policy: &mut CachePolicy) {
        let (duration, statuses) = directive
            .args
            .split_last()
            .expect("the schema requires an argument");
        let Some(seconds) = self.seconds(file, duration) else {
            return;
        };
        if statuses.is_empty() {
            policy.ttl_seconds = seconds;
            return;
        }
        for arg in statuses {
            let Some(value) = self.value(file, arg) else {
                continue;
            };
            match value.parse::<u16>() {
                Ok(status) if (100..=599).contains(&status) => {
                    policy.status_ttls.insert(status, seconds);
                }
                _ => self.error_with_help(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{value:?} is not a status"),
                    "write it as `valid 404 410 1m;`, the duration last",
                ),
            }
        }
    }

    fn bypass(
        &mut self,
        file: &str,
        directive: &Directive,
        depth: usize,
        bypass: &mut Vec<RouteCondition>,
    ) {
        let Some(block) = directive.block() else {
            return;
        };
        let mut seen = BTreeSet::new();
        self.each(
            file,
            &block.directives,
            Context::Conditions,
            depth + 1,
            &mut seen,
            &mut |lowerer, file, directive, _, _| {
                if let Some(condition) = lowerer.condition(file, directive, depth + 1) {
                    bypass.push(condition);
                }
            },
        );
        if bypass.is_empty() {
            self.error(
                file,
                directive.span,
                codes::ARGUMENTS,
                "'bypass' holds no condition",
            );
        }
    }

    /// A duration in whole seconds, `0` included.
    fn seconds(&mut self, file: &str, arg: &Argument) -> Option<u64> {
        let value = self.value(file, arg)?;
        let ms = self.duration(file, arg, &value)?;
        if ms % 1000 != 0 {
            self.error(
                file,
                arg.span,
                codes::TYPE,
                format!("{value:?} is not a whole number of seconds"),
            );
            return None;
        }
        Some(ms / 1000)
    }

    fn short_seconds(&mut self, file: &str, arg: &Argument) -> Option<u32> {
        let seconds = self.whole_seconds(file, arg)?;
        let short = u32::try_from(seconds).ok();
        if short.is_none() {
            self.error(
                file,
                arg.span,
                codes::TYPE,
                format!("{:?} is too long", arg.value),
            );
        }
        short
    }

    /// `cache_store max_size=<size>;`
    pub(super) fn cache_store(&mut self, file: &str, directive: &Directive, depth: usize) {
        let params = Params::split(&directive.args);
        self.only_params(file, &params, &["max_size"]);
        for arg in &params.positional {
            self.error_with_help(
                file,
                arg.span,
                codes::ARGUMENTS,
                format!("{:?} is not a setting", arg.value),
                "write it as `cache_store max_size=512m;`",
            );
        }
        if let Some((value, arg)) = params.named.get("max_size") {
            match values::parse_size(value) {
                Some(size) => self.cache.max_bytes = Some(size),
                None => self.error_with_help(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{value:?} is not a size"),
                    "write sizes such as 256m or 2g",
                ),
            }
        }
        self.origins
            .insert("cache".into(), Self::origin(file, directive, depth));
    }
}
