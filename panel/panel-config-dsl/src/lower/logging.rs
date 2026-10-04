//! Logging directives (ADR 0025).

use super::{Expansion, Lowerer};
use crate::{
    codes,
    values::{self, Params},
};
use panel_dsl::Directive;
use panel_ir::{
    logging::is_field_name, template::parse_template, AccessLog, AccessLogFormat, LogFiles,
    LoggingPolicy,
};
use std::collections::BTreeSet;

const DAY_MS: u64 = 86_400_000;

impl Lowerer<'_> {
    /// `access_log [on|off] [format=json|combined];`
    pub(super) fn access_log(&mut self, file: &str, directive: &Directive, access: &mut AccessLog) {
        let params = Params::split(&directive.args);
        self.only_params(file, &params, &["format"]);
        match params.positional.as_slice() {
            [] => {}
            [arg] => {
                if let Some(on) = self.bool_arg(file, arg) {
                    access.enabled = Some(on);
                }
            }
            [_, extra, ..] => self.error_with_help(
                file,
                extra.span,
                codes::ARGUMENTS,
                "access_log takes on or off once",
                "write `access_log off;` or `access_log on format=combined;`",
            ),
        }
        if let Some((value, arg)) = params.named.get("format") {
            match *value {
                "json" => access.format = Some(AccessLogFormat::Json),
                "combined" => access.format = Some(AccessLogFormat::Combined),
                other => self.error(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{other:?} is not json or combined"),
                ),
            }
        }
    }

    /// `log_field <name> <template>;`
    pub(super) fn log_field(&mut self, file: &str, directive: &Directive, access: &mut AccessLog) {
        let [name, template] = directive.args.as_slice() else {
            return;
        };
        if !is_field_name(&name.value) {
            self.error_with_help(
                file,
                name.span,
                codes::TYPE,
                format!("{:?} cannot name a log field", name.value),
                "use lowercase words joined by dots, other than names records use such as url.path",
            );
            return;
        }
        let Some(expanded) =
            self.expand(file, template, &template.value.clone(), Expansion::Template)
        else {
            return;
        };
        if let Err(error) = parse_template(&expanded) {
            self.error(file, template.span, codes::TYPE, error);
            return;
        }
        if access.fields.insert(name.value.clone(), expanded).is_some() {
            self.error(
                file,
                name.span,
                codes::DUPLICATE,
                format!("log field {} is written twice here", name.value),
            );
        }
    }

    /// The logging directives of `http`.
    pub(super) fn logging(&mut self, file: &str, directive: &Directive) {
        let mut policy = std::mem::take(&mut self.logging);
        match directive.name.value.as_str() {
            "access_log" => self.access_log(file, directive, &mut policy.access),
            "log_field" => self.log_field(file, directive, &mut policy.access),
            "log_redact_query" => {
                policy.redact_query = Some(match directive.args.as_slice() {
                    [arg] if arg.value == "off" => BTreeSet::new(),
                    args => args.iter().map(Self::literal).collect(),
                });
            }
            "log_redact_headers" => {
                for arg in &directive.args {
                    let name = arg.value.to_ascii_lowercase();
                    if name.is_empty()
                        || !name.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
                        })
                    {
                        self.error(
                            file,
                            arg.span,
                            codes::TYPE,
                            format!("{:?} is not a header name", arg.value),
                        );
                    } else {
                        policy.redact_headers.insert(name);
                    }
                }
            }
            "log_files" => self.log_files(file, directive, &mut policy.files),
            _ => unreachable!("the schema allows no other logging directive in http"),
        }
        self.logging = policy;
    }

    /// `log_files [max_size=<size>] [rotate=daily|size] [keep=<days>] [max_files=<n>];`
    fn log_files(&mut self, file: &str, directive: &Directive, files: &mut LogFiles) {
        let params = Params::split(&directive.args);
        self.only_params(file, &params, &["max_size", "rotate", "keep", "max_files"]);
        for arg in &params.positional {
            self.error_with_help(
                file,
                arg.span,
                codes::ARGUMENTS,
                format!("{:?} is not a setting", arg.value),
                "write settings such as max_size=100m rotate=daily keep=7d max_files=30",
            );
        }
        if let Some((value, arg)) = params.named.get("max_size") {
            match values::parse_size(value) {
                Some(size) => files.max_size_bytes = size,
                None => self.error_with_help(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{value:?} is not a size"),
                    "write sizes such as 512k, 100m or 1g",
                ),
            }
        }
        if let Some((value, arg)) = params.named.get("rotate") {
            match *value {
                "daily" => files.rotate_daily = true,
                "size" => files.rotate_daily = false,
                other => self.error(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{other:?} is not daily or size"),
                ),
            }
        }
        if let Some((value, arg)) = params.named.get("keep") {
            let days = match *value {
                "0" => Some(0),
                value => values::parse_duration_ms(value)
                    .filter(|ms| ms % DAY_MS == 0)
                    .and_then(|ms| u32::try_from(ms / DAY_MS).ok()),
            };
            match days {
                Some(days) => files.keep_days = days,
                None => self.error_with_help(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{value:?} is not a number of days"),
                    "write whole days such as 7d, or 0 to keep every rotated file",
                ),
            }
        }
        if let Some((value, arg)) = params.named.get("max_files") {
            if let Some(count) = self.number(file, arg, value, "a whole number") {
                files.max_files = count;
            }
        }
    }
}

/// The `http` directives that print `policy`, defaults left out.
pub(crate) fn print_policy(policy: &LoggingPolicy) -> Vec<Directive> {
    let mut directives = print_access(&policy.access);
    if let Some(keys) = &policy.redact_query {
        directives.push(if keys.is_empty() {
            Directive::simple("log_redact_query", ["off"])
        } else {
            Directive::simple("log_redact_query", keys.iter().cloned())
        });
    }
    if !policy.redact_headers.is_empty() {
        directives.push(Directive::simple(
            "log_redact_headers",
            policy.redact_headers.iter().cloned(),
        ));
    }
    let files = policy.files;
    let defaults = LogFiles::default();
    let mut args = Vec::new();
    if files.max_size_bytes != defaults.max_size_bytes {
        args.push(format!(
            "max_size={}",
            values::print_size(files.max_size_bytes)
        ));
    }
    if files.rotate_daily != defaults.rotate_daily {
        args.push(format!(
            "rotate={}",
            if files.rotate_daily { "daily" } else { "size" }
        ));
    }
    if files.keep_days != defaults.keep_days {
        args.push(match files.keep_days {
            0 => "keep=0".to_owned(),
            days => format!("keep={days}d"),
        });
    }
    if files.max_files != defaults.max_files {
        args.push(format!("max_files={}", files.max_files));
    }
    if !args.is_empty() {
        directives.push(Directive::simple("log_files", args));
    }
    directives
}

/// The directives that print `access`, in `http`, a server or a route.
pub(crate) fn print_access(access: &AccessLog) -> Vec<Directive> {
    let mut directives = Vec::new();
    let mut args = Vec::new();
    if let Some(enabled) = access.enabled {
        args.push(values::print_bool(enabled).to_owned());
    }
    match access.format {
        Some(AccessLogFormat::Json) => args.push("format=json".to_owned()),
        Some(AccessLogFormat::Combined) => args.push("format=combined".to_owned()),
        None => {}
    }
    if !args.is_empty() {
        directives.push(Directive::simple("access_log", args));
    }
    for (name, template) in &access.fields {
        directives.push(Directive::simple(
            "log_field",
            [name.clone(), template.clone()],
        ));
    }
    directives
}
