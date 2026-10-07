//! What a server or route writes about the files its `root` serves:
//! media types and cache rules (ADR 0042).

use super::{ActionDraft, Expansion, Lowerer};
use crate::{
    codes,
    values::{print_duration_ms, Params},
    variables::escape,
};
use panel_config_model::Action;
use panel_dsl::{Directive, Span};
use panel_errors::Diagnostic;
use panel_ir::StaticCacheRule;
use std::collections::BTreeMap;

pub(super) const STATICS: &[&str] = &["media_type", "default_type", "cache_control"];

#[derive(Default)]
pub(super) struct StaticSettings {
    media_types: BTreeMap<String, String>,
    default_type: Option<String>,
    cache: Vec<StaticCacheRule>,
    /// The first of them written, to point at when nothing serves files.
    first: Option<(String, Span, &'static str)>,
}

fn extension(value: &str) -> String {
    value.trim().trim_start_matches('.').to_ascii_lowercase()
}

impl Lowerer<'_> {
    pub(super) fn static_setting(
        &mut self,
        file: &str,
        directive: &Directive,
        name: &'static str,
        settings: &mut StaticSettings,
    ) {
        settings
            .first
            .get_or_insert_with(|| (file.to_owned(), directive.span, name));
        match name {
            // `media_type <type> <extension> ...;`, as nginx's `types` writes
            // each type.
            "media_type" => {
                let Some((type_arg, extensions)) = directive.args.split_first() else {
                    return;
                };
                let Some(media_type) = self.value(file, type_arg) else {
                    return;
                };
                for arg in extensions {
                    let Some(written) = self.value(file, arg) else {
                        continue;
                    };
                    let extension = extension(&written);
                    match settings.media_types.get(&extension) {
                        Some(other) if *other != media_type => self.error(
                            file,
                            arg.span,
                            codes::DUPLICATE,
                            format!("{extension} already has the media type {other}"),
                        ),
                        _ => {
                            settings.media_types.insert(extension, media_type.clone());
                        }
                    }
                }
            }
            "default_type" => {
                settings.default_type =
                    directive.args.first().and_then(|arg| self.value(file, arg));
            }
            "cache_control" => {
                if let Some(rule) = self.cache_rule(file, directive) {
                    settings.cache.push(rule);
                }
            }
            _ => unreachable!("the schema has no other static settings"),
        }
    }

    /// `cache_control max_age=<duration> [immutable] [for=<extension>,...];`
    /// or `cache_control no_cache [for=...];`
    fn cache_rule(&mut self, file: &str, directive: &Directive) -> Option<StaticCacheRule> {
        let params = Params::split(&directive.args);
        self.only_params(file, &params, &["max_age", "for"]);
        let mut rule = StaticCacheRule::default();
        if let Some((value, arg)) = params.named.get("for") {
            rule.extensions = self
                .expand(file, arg, value, Expansion::Text)?
                .split(',')
                .map(extension)
                .filter(|extension| !extension.is_empty())
                .collect();
        }
        let flags: Vec<&str> = params
            .positional
            .iter()
            .map(|arg| arg.value.as_str())
            .collect();
        match (params.named.get("max_age"), flags.as_slice()) {
            (Some((value, arg)), [] | ["immutable"]) => {
                rule.max_age_seconds = Some(self.duration(file, arg, value)? / 1_000);
                rule.immutable = !flags.is_empty();
            }
            (None, ["no_cache"]) => {}
            _ => {
                self.error_with_help(
                    file,
                    directive.span,
                    codes::ARGUMENTS,
                    "a cache rule is a max-age, maybe immutable, or no_cache",
                    "write `cache_control max_age=1y immutable for=css,js;` or `cache_control no_cache for=html;`",
                );
                return None;
            }
        }
        Some(rule)
    }

    /// Gives `settings` to the static action written beside them, or says
    /// that nothing there serves files.
    pub(super) fn attach_statics(
        &mut self,
        settings: StaticSettings,
        action: &mut Option<ActionDraft>,
    ) {
        let Some((file, span, name)) = settings.first.clone() else {
            return;
        };
        match action {
            Some(ActionDraft::Ready(Action::Static {
                media_types,
                default_type,
                cache,
                ..
            })) => {
                *media_types = settings.media_types;
                *default_type = settings.default_type;
                *cache = settings.cache;
            }
            _ => self.report(
                Diagnostic::warning(
                    codes::NO_EFFECT,
                    format!("{name} has no effect: nothing here serves files with root"),
                )
                .with_help("serve files with root here, or remove this"),
                &file,
                span,
            ),
        }
    }
}

/// The directives writing the media types and cache rules of a static
/// action.
pub(crate) fn print_statics(
    media_types: &BTreeMap<String, String>,
    default_type: Option<&str>,
    cache: &[StaticCacheRule],
) -> Vec<Directive> {
    let mut by_type: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (extension, media_type) in media_types {
        by_type.entry(media_type).or_default().push(extension);
    }
    let mut directives: Vec<Directive> = by_type
        .into_iter()
        .map(|(media_type, extensions)| {
            Directive::simple(
                "media_type",
                std::iter::once(escape(media_type).into_owned()).chain(
                    extensions
                        .into_iter()
                        .map(|extension| escape(extension).into_owned()),
                ),
            )
        })
        .collect();
    if let Some(default_type) = default_type {
        directives.push(Directive::simple(
            "default_type",
            [escape(default_type).into_owned()],
        ));
    }
    for rule in cache {
        let mut args = match rule.max_age_seconds {
            Some(seconds) => vec![format!(
                "max_age={}",
                print_duration_ms(seconds.saturating_mul(1_000))
            )],
            None => vec!["no_cache".to_owned()],
        };
        if rule.immutable && rule.max_age_seconds.is_some() {
            args.push("immutable".into());
        }
        if !rule.extensions.is_empty() {
            let extensions: Vec<&str> = rule.extensions.iter().map(String::as_str).collect();
            args.push(format!("for={}", escape(&extensions.join(","))));
        }
        directives.push(Directive::simple("cache_control", args));
    }
    directives
}
