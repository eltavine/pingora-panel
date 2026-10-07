//! NGINX's proxy cache in the language (ADR 0043): the zones of
//! `proxy_cache_path` share the gateway's in-memory store, and `proxy_cache`
//! with the settings in effect where it is written becomes a cache policy of
//! the server or route.

use super::{args, identifier, Importer, Located};
use crate::values::{parse_duration_ms, parse_size, print_duration_ms, print_size};
use panel_dsl::{format_directive, Directive};
use std::collections::BTreeMap;

/// The directives a block's [`CacheScope`] reads.
pub(super) const DIRECTIVES: &[&str] = &[
    "proxy_cache",
    "proxy_cache_key",
    "proxy_cache_valid",
    "proxy_cache_bypass",
    "proxy_no_cache",
    "proxy_ignore_headers",
    "proxy_cache_use_stale",
    "proxy_cache_lock",
    "proxy_cache_revalidate",
    "proxy_cache_background_update",
    "proxy_cache_methods",
    "proxy_cache_min_uses",
    "proxy_cache_lock_timeout",
    "proxy_cache_lock_age",
    "proxy_cache_max_range_offset",
    "proxy_cache_convert_head",
];

/// NGINX's `inactive` when a zone does not write it.
const INACTIVE_MS: u64 = 10 * 60_000;

/// The statuses NGINX's `proxy_cache_valid` covers without any.
const NGINX_VALID: [&str; 3] = ["200", "301", "302"];

/// The zones `proxy_cache_path` declares and the policies made of them.
#[derive(Default)]
pub(super) struct Caches {
    /// `inactive` by zone.
    zones: BTreeMap<String, u64>,
    /// What the zones' `max_size` add up to.
    store_bytes: u64,
    /// A zone without `max_size`.
    unbounded: bool,
    /// Policies with their bodies as written, in the order they are made.
    policies: Vec<(String, String, Directive)>,
}

impl Caches {
    /// `cache_store` and the policies, for the start of `http`.
    pub(super) fn directives(self) -> Vec<Directive> {
        let mut out = Vec::new();
        if self.store_bytes > 0 && !self.unbounded {
            out.push(Directive::simple(
                "cache_store",
                [format!("max_size={}", print_size(self.store_bytes))],
            ));
        }
        out.extend(self.policies.into_iter().map(|(_, _, policy)| policy));
        out
    }
}

#[derive(Clone, Copy, Default)]
struct Stale {
    updating: bool,
    error: bool,
}

/// The cache settings of a block. Each directive is inherited from the
/// enclosing block unless the block writes it, as in NGINX.
#[derive(Clone, Default)]
pub(super) struct CacheScope {
    /// `Some(None)` for `proxy_cache off`.
    zone: Option<Option<String>>,
    key: Option<String>,
    valid: Vec<Directive>,
    bypass: Vec<Directive>,
    no_cache: Vec<Directive>,
    ignore_origin: bool,
    stale: Stale,
    /// The lists this block writes, which replace the inherited ones.
    own: [bool; 3],
    /// The block writes a cache directive of its own.
    pub(super) written: bool,
}

impl CacheScope {
    /// The settings a block inside this one starts with.
    pub(super) fn inner(&self) -> Self {
        Self {
            own: [false; 3],
            written: false,
            ..self.clone()
        }
    }

    /// The list at `slot`, emptied when the block writes it first.
    fn list(&mut self, slot: usize) -> &mut Vec<Directive> {
        let fresh = !self.own[slot];
        self.own[slot] = true;
        let list = match slot {
            0 => &mut self.valid,
            1 => &mut self.bypass,
            _ => &mut self.no_cache,
        };
        if fresh {
            list.clear();
        }
        list
    }

    fn body(&self, inactive_ms: u64) -> Vec<Directive> {
        let mut body = Vec::new();
        if let Some(key) = &self.key {
            body.push(Directive::simple("key", [key.clone()]));
        }
        body.extend(self.valid.iter().cloned());
        if self.ignore_origin {
            body.push(Directive::simple("honor_origin", ["off"]));
        }
        let mut bypass: Vec<Directive> = Vec::new();
        for condition in self.bypass.iter().chain(&self.no_cache) {
            let text = format_directive(condition, 0);
            if !bypass
                .iter()
                .any(|known| format_directive(known, 0) == text)
            {
                bypass.push(condition.clone());
            }
        }
        if !bypass.is_empty() {
            body.push(Directive::with_block(
                "bypass",
                Vec::<String>::new(),
                bypass,
            ));
        }
        let stale = print_duration_ms(inactive_ms.div_ceil(1_000).max(1) * 1_000);
        if self.stale.updating {
            body.push(Directive::simple("stale_while_revalidate", [stale.clone()]));
        }
        if self.stale.error {
            body.push(Directive::simple("stale_if_error", [stale]));
        }
        body
    }
}

impl Importer<'_> {
    /// `proxy_cache_path <path> keys_zone=<name>:<size> [max_size=<size>]
    /// [inactive=<time>] ...;`
    pub(super) fn cache_path(&mut self, located: &Located) {
        let values = args(&located.directive);
        let mut zone = None;
        let mut inactive_ms = INACTIVE_MS;
        let mut max_bytes = None;
        for value in values.iter().skip(1) {
            match value.split_once('=') {
                Some(("keys_zone", spec)) => {
                    zone = Some(spec.split_once(':').map_or(spec, |(name, _)| name));
                }
                Some(("max_size", size)) => max_bytes = parse_size(size),
                Some(("inactive", time)) => {
                    inactive_ms = parse_duration_ms(time).unwrap_or(INACTIVE_MS);
                }
                _ => {}
            }
        }
        let Some(zone) = zone else {
            self.unsupported(
                located,
                "'proxy_cache_path' without keys_zone is not carried over".into(),
            );
            return;
        };
        let caches = &mut self.out.caches;
        match max_bytes {
            Some(bytes) => caches.store_bytes = caches.store_bytes.saturating_add(bytes),
            None => caches.unbounded = true,
        }
        caches.zones.insert(zone.to_owned(), inactive_ms);
        let path = values.first().copied().unwrap_or_default();
        self.changed(
            located,
            format!(
                "the zone {zone} is kept with the others in the gateway's memory, not in {path}{}",
                if max_bytes.is_some() {
                    ""
                } else {
                    ", 256m unless cache_store says"
                }
            ),
        );
    }

    /// Reads a cache directive of a block into the block's settings.
    pub(super) fn cache_setting(&mut self, located: &Located, scope: &mut CacheScope) {
        scope.written = true;
        let values = args(&located.directive);
        match located.directive.name.value.as_str() {
            "proxy_cache" => match values.as_slice() {
                ["off"] => scope.zone = Some(None),
                [zone] if !zone.contains('$') => scope.zone = Some(Some((*zone).to_owned())),
                _ => self.unsupported(
                    located,
                    "a cache zone chosen per request is not supported".into(),
                ),
            },
            "proxy_cache_key" => {
                let written = values.first().copied().unwrap_or_default();
                if written.contains("$proxy_host") {
                    self.changed(
                        located,
                        "$proxy_host is read as $host, the host the client asked for".into(),
                    );
                }
                if let Some(key) = self.template(located, &written.replace("$proxy_host", "$host"))
                {
                    scope.key = Some(key);
                }
            }
            "proxy_cache_valid" => {
                if let Some(valid) = self.cache_valid(located, &values) {
                    scope.list(0).push(valid);
                }
            }
            name @ ("proxy_cache_bypass" | "proxy_no_cache") => {
                let conditions = self.cache_conditions(located, &values, name == "proxy_no_cache");
                scope
                    .list(if name == "proxy_cache_bypass" { 1 } else { 2 })
                    .extend(conditions);
            }
            "proxy_ignore_headers" => {
                scope.ignore_origin = false;
                for field in &values {
                    match field.to_ascii_lowercase().as_str() {
                        "cache-control" | "expires" | "x-accel-expires" => {
                            scope.ignore_origin = true;
                        }
                        "set-cookie" => self.changed(
                            located,
                            "responses with Set-Cookie are never stored".into(),
                        ),
                        "vary" => self.changed(located, "Vary is always honored".into()),
                        _ => {}
                    }
                }
                if scope.ignore_origin {
                    self.changed(
                        located,
                        "the origin's Cache-Control and Expires are ignored together".into(),
                    );
                }
            }
            "proxy_cache_use_stale" => {
                scope.stale = Stale::default();
                for condition in &values {
                    match *condition {
                        "updating" => scope.stale.updating = true,
                        "off" => {}
                        _ => scope.stale.error = true,
                    }
                }
                if scope.stale.updating || scope.stale.error {
                    self.changed(
                        located,
                        "stale responses are served for at most the zone's inactive time, and when the upstream fails rather than for each status listed".into(),
                    );
                }
            }
            name @ ("proxy_cache_lock" | "proxy_cache_revalidate" | "proxy_cache_background_update") => {
                if values == ["off"] {
                    let always = match name {
                        "proxy_cache_lock" => "collapses concurrent misses",
                        "proxy_cache_revalidate" => "revalidates with the stored validators",
                        _ => "revalidates in the background",
                    };
                    self.changed(located, format!("the gateway always {always}"));
                }
            }
            "proxy_cache_methods" => {
                if values
                    .iter()
                    .any(|method| !matches!(*method, "GET" | "HEAD"))
                {
                    self.unsupported(
                        located,
                        "only GET and HEAD requests use the cache".into(),
                    );
                }
            }
            "proxy_cache_min_uses" => self.unsupported(
                located,
                "'proxy_cache_min_uses' is not carried over: the cache admits responses by how often they are asked for".into(),
            ),
            name => self.unsupported(located, format!("'{name}' is not supported")),
        }
    }

    /// `proxy_cache_valid [<code> ...] <time>;` as `valid`, with NGINX's
    /// statuses when it lists none.
    fn cache_valid(&mut self, located: &Located, values: &[&str]) -> Option<Directive> {
        let (time, codes) = values.split_last()?;
        let Some(ms) = parse_duration_ms(time) else {
            self.unsupported(located, format!("{time:?} is not a duration"));
            return None;
        };
        let time = print_duration_ms(ms.div_ceil(1_000) * 1_000);
        if codes.contains(&"any") {
            self.changed(
                located,
                "any is read as 200, 203, 204, 300, 301 and 308, the statuses stored by default"
                    .into(),
            );
            return Some(Directive::simple("valid", [time]));
        }
        let mut statuses = Vec::new();
        for code in if codes.is_empty() {
            &NGINX_VALID[..]
        } else {
            codes
        } {
            match code.parse::<u16>() {
                Ok(status) if (100..=599).contains(&status) => statuses.push((*code).to_owned()),
                _ => {
                    self.unsupported(located, format!("{code:?} is not a status"));
                    return None;
                }
            }
        }
        statuses.push(time);
        Some(Directive::simple("valid", statuses))
    }

    /// The variables of `proxy_cache_bypass` or `proxy_no_cache` as
    /// conditions of `bypass`.
    fn cache_conditions(
        &mut self,
        located: &Located,
        values: &[&str],
        no_cache: bool,
    ) -> Vec<Directive> {
        let mut conditions = Vec::new();
        for value in values {
            let condition = value.strip_prefix('$').and_then(|name| {
                [
                    ("cookie_", "cookie"),
                    ("arg_", "query"),
                    ("http_", "header"),
                ]
                .iter()
                .find_map(|(prefix, kind)| {
                    let field = name.strip_prefix(prefix)?;
                    let field = if *kind == "header" {
                        field.replace('_', "-")
                    } else {
                        field.to_owned()
                    };
                    (!field.is_empty()).then_some((*kind, field))
                })
            });
            match condition {
                Some((kind, field)) => {
                    conditions.push(Directive::simple(kind, [field, "present".to_owned()]));
                }
                None => self.unsupported(
                    located,
                    format!("{value:?} has no counterpart among request conditions"),
                ),
            }
        }
        if !conditions.is_empty() {
            self.changed(
                located,
                format!(
                    "requests are bypassed when these are present, even as 0{}",
                    if no_cache {
                        ", and requests not stored do not use the cache either"
                    } else {
                        ""
                    }
                ),
            );
        }
        conditions
    }

    /// The policy a block caches with, made once for each zone and settings;
    /// `None` when the block caches nothing.
    pub(super) fn cache_policy(&mut self, located: &Located, scope: &CacheScope) -> Option<String> {
        let zone = scope.zone.clone().flatten()?;
        let Some(inactive_ms) = self.out.caches.zones.get(&zone).copied() else {
            self.unsupported(
                located,
                format!("the cache zone {zone} is not declared with proxy_cache_path"),
            );
            return None;
        };
        let body = scope.body(inactive_ms);
        let text: String = body
            .iter()
            .map(|directive| format_directive(directive, 0))
            .collect();
        let policies = &mut self.out.caches.policies;
        let base = identifier(&zone).to_ascii_lowercase();
        if let Some((name, _, _)) = policies.iter().find(|(name, written, _)| {
            *written == text && (*name == base || name.starts_with(&format!("{base}-")))
        }) {
            return Some(name.clone());
        }
        let mut name = base.clone();
        let mut counter = 2;
        while policies.iter().any(|(taken, _, _)| *taken == name) || name == crate::lower::OFF {
            name = format!("{base}-{counter}");
            counter += 1;
        }
        policies.push((
            name.clone(),
            text,
            Directive::with_block("cache_policy", [name.clone()], body),
        ));
        Some(name)
    }
}
