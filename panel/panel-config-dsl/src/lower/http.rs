//! `http_policy` blocks (ADR 0037): field changes, the `Server` field, CORS
//! and compression.

use super::{Expansion, Lowerer};
use crate::{
    codes,
    schema::Context,
    values::{self, Params},
};
use panel_config_model::{FieldChanges, HttpPolicy};
use panel_dsl::{Argument, Directive};
use panel_ir::{
    template::parse_template, CompressionAlgorithm, CompressionPolicy, CorsPolicy, HeaderField,
    ServerHeader,
};
use std::collections::BTreeSet;

/// The content codings `compress` takes, by their HTTP names.
pub const CODINGS: [(&str, CompressionAlgorithm); 3] = [
    ("gzip", CompressionAlgorithm::Gzip),
    ("br", CompressionAlgorithm::Brotli),
    ("zstd", CompressionAlgorithm::Zstd),
];

impl Lowerer<'_> {
    pub(super) fn http_policy(&mut self, file: &str, directive: &Directive, depth: usize) {
        let id = Self::literal(&directive.args[0]);
        if self.http_policies.iter().any(|(policy, _)| policy.id == id) {
            self.error(
                file,
                directive.args[0].span,
                codes::DUPLICATE,
                format!("HTTP policy {id:?} is defined twice"),
            );
            return;
        }
        let mut policy = HttpPolicy {
            id,
            ..HttpPolicy::default()
        };
        if let Some(block) = directive.block() {
            self.with_scope(format!("http-policies/{}", policy.id), |lowerer| {
                let mut seen = BTreeSet::new();
                lowerer.each(
                    file,
                    &block.directives,
                    Context::HttpPolicy,
                    depth + 1,
                    &mut seen,
                    &mut |lowerer, file, directive, spec, depth| match spec.name {
                        "request_header" => {
                            lowerer.field_change(file, directive, &mut policy.request)
                        }
                        "response_header" => {
                            lowerer.field_change(file, directive, &mut policy.response);
                        }
                        "server_header" => {
                            lowerer.server_header(file, directive, &mut policy.server)
                        }
                        "cors" => policy.cors = Some(lowerer.cors(file, directive, depth)),
                        "compress" => policy.compression = lowerer.compress(file, directive),
                        _ => unreachable!("the schema allows nothing else in http_policy"),
                    },
                );
            });
        }
        let origin = Self::origin(file, directive, depth);
        self.origins
            .insert(format!("http-policies/{}", policy.id), origin.clone());
        self.http_policies.push((policy, origin));
    }

    fn field_change(&mut self, file: &str, directive: &Directive, changes: &mut FieldChanges) {
        let (operation, rest) = directive
            .args
            .split_first()
            .expect("the schema requires an argument");
        match (operation.value.as_str(), rest) {
            ("remove", names) if !names.is_empty() => {
                changes.remove.extend(names.iter().map(Self::literal));
            }
            (operation @ ("set" | "add"), [name, value]) => {
                let Some(value) = self.template(file, value) else {
                    return;
                };
                let field = HeaderField {
                    name: Self::literal(name),
                    value,
                };
                if operation == "set" {
                    changes.set.push(field);
                } else {
                    changes.add.push(field);
                }
            }
            _ => self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                format!(
                    "{} takes remove <name> ..., set <name> <value> or add <name> <value>",
                    directive.name.value
                ),
                format!(
                    "write it as `{} set X-Frame-Options DENY;`",
                    directive.name.value
                ),
            ),
        }
    }

    /// A value the gateway fills in per request, checked where it is written.
    fn template(&mut self, file: &str, arg: &Argument) -> Option<String> {
        let value = self.expand(file, arg, &arg.value.clone(), Expansion::Template)?;
        if let Err(error) = parse_template(&value) {
            self.error(file, arg.span, codes::TYPE, error);
            return None;
        }
        Some(value)
    }

    fn server_header(&mut self, file: &str, directive: &Directive, server: &mut ServerHeader) {
        match directive.args.as_slice() {
            [mode] if mode.value == "keep" => *server = ServerHeader::Keep,
            [mode] if mode.value == "remove" => *server = ServerHeader::Remove,
            [mode, value] if mode.value == "replace" => {
                if let Some(value) = self.value(file, value) {
                    *server = ServerHeader::Replace { value };
                }
            }
            _ => self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                "server_header is keep, remove or replace <value>",
                "write it as `server_header replace shop;`",
            ),
        }
    }

    fn cors(&mut self, file: &str, directive: &Directive, depth: usize) -> CorsPolicy {
        let mut cors = CorsPolicy::default();
        let Some(block) = directive.block() else {
            return cors;
        };
        let mut seen = BTreeSet::new();
        self.each(
            file,
            &block.directives,
            Context::Cors,
            depth + 1,
            &mut seen,
            &mut |lowerer, file, directive, spec, _| {
                let names = || directive.args.iter().map(Self::literal);
                let arg = &directive.args[0];
                match spec.name {
                    "origins" => {
                        for arg in &directive.args {
                            if let Some(origin) = lowerer.value(file, arg) {
                                cors.allowed_origins.push(origin);
                            }
                        }
                    }
                    "methods" => cors.allowed_methods.extend(names()),
                    "headers" => cors.allowed_headers.extend(names()),
                    "expose" => cors.exposed_headers.extend(names()),
                    "credentials" => {
                        cors.allow_credentials = lowerer.bool_arg(file, arg).unwrap_or_default();
                    }
                    "max_age" => cors.max_age_seconds = lowerer.preflight_age(file, arg),
                    _ => unreachable!("the schema allows nothing else in cors"),
                }
            },
        );
        cors
    }

    fn preflight_age(&mut self, file: &str, arg: &Argument) -> Option<u32> {
        let value = self.value(file, arg)?;
        let ms = self.duration(file, arg, &value)?;
        match u32::try_from(ms / 1000) {
            Ok(seconds) if ms % 1000 == 0 => Some(seconds),
            _ => {
                self.error(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{value:?} is not a whole number of seconds"),
                );
                None
            }
        }
    }

    fn compress(&mut self, file: &str, directive: &Directive) -> Option<CompressionPolicy> {
        let params = Params::split(&directive.args);
        self.only_params(file, &params, &["types", "min_size"]);
        let mut algorithms = BTreeSet::new();
        for arg in &params.positional {
            match CODINGS.iter().find(|(name, _)| *name == arg.value) {
                Some((_, algorithm)) => {
                    algorithms.insert(*algorithm);
                }
                None => self.error_with_help(
                    file,
                    arg.span,
                    codes::TYPE,
                    format!("{:?} is not a content coding", arg.value),
                    "write gzip, br or zstd",
                ),
            }
        }
        let help = "write it as `compress gzip br types=text/*,application/json min_size=1k;`";
        let Some((types, types_arg)) = params.named.get("types") else {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                "compress names the media types it compresses",
                help,
            );
            return None;
        };
        let types = self
            .expand(file, types_arg, types, Expansion::Text)?
            .split(',')
            .map(str::trim)
            .filter(|media| !media.is_empty())
            .map(str::to_owned)
            .collect();
        let min_bytes = match params.named.get("min_size") {
            Some((value, arg)) => {
                let Some(bytes) = values::parse_size(value) else {
                    self.error_with_help(
                        file,
                        arg.span,
                        codes::TYPE,
                        format!("{value:?} is not a size"),
                        "write sizes such as 512, 1k or 1m",
                    );
                    return None;
                };
                bytes
            }
            None => 0,
        };
        if algorithms.is_empty() {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                "compress names at least one coding",
                help,
            );
            return None;
        }
        Some(CompressionPolicy {
            algorithms,
            types,
            min_bytes,
        })
    }
}
