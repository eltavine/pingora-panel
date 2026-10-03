//! What applies in a block of the configuration and where each value comes
//! from: written in the block, taken over from a block around it or from a
//! listener serving it, or a default. The rules are the schema's.

use crate::{
    schema::{self, Context},
    Lowered, Sources, Written,
};
use panel_config_model::{Listener, Route, Site, Upstream};
use panel_dsl::{LineIndex, Span};
use panel_ir::TlsProfile;
use serde::Serialize;
use std::collections::BTreeMap;

/// Where a value comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SettingSource {
    /// Written in the block.
    Here,
    /// Written in a block around it, or in a listener serving it.
    Inherited,
    /// Written nowhere.
    Default,
}

/// One value that applies in a block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Setting {
    /// The directive, `sni` for what a node sends, or `$name` for a constant.
    pub name: String,
    /// What in the block the value is for: a host or a node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The value as it is written; empty when there is none.
    pub value: String,
    pub source: SettingSource,
    /// The block it is taken over from, such as `server shop`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Where it is written, as `file:line.column-line.column`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_span: Option<String>,
    /// Where the value comes from when the block does not write it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<&'static str>,
}

impl Setting {
    fn new(name: impl Into<String>, value: impl Into<String>, source: SettingSource) -> Self {
        Self {
            name: name.into(),
            scope: None,
            value: value.into(),
            source,
            from: None,
            source_span: None,
            rule: None,
        }
    }

    fn scoped(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    fn from(mut self, block: impl Into<String>) -> Self {
        self.from = Some(block.into());
        self
    }

    fn inherited(mut self) -> Self {
        self.source = SettingSource::Inherited;
        self
    }

    fn at(mut self, span: Option<String>) -> Self {
        self.source_span = span;
        self
    }

    /// The rule of `directive` in `context`.
    fn rule(mut self, directive: &str, context: Context) -> Self {
        self.rule = schema::lookup(directive, context).and_then(|spec| spec.inheritance);
        self
    }
}

/// The values that apply in one block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Explanation {
    pub block: Context,
    /// The block's name, such as the server's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its resource path, such as `sites/<id>/routes/<id>`.
    pub resource: String,
    /// Where the block is written.
    pub source_span: String,
    pub settings: Vec<Setting>,
}

/// Explains the innermost server, route, listener, upstream or TLS profile
/// written at the 1-based `line` and `column` of `file`.
pub fn explain(
    sources: &Sources,
    lowered: &Lowered,
    file: &str,
    line: usize,
    column: usize,
) -> Option<Explanation> {
    let text = sources.get(file)?;
    let offset = LineIndex::new(text).offset(text, line, column)?;
    let (resource, origin) = lowered
        .origins
        .iter()
        .filter(|(resource, origin)| {
            origin.file == file
                && (origin.span.start..origin.span.end).contains(&offset)
                && !resource.contains("/domains/")
                && !resource.contains("/nodes/")
        })
        .min_by_key(|(_, origin)| origin.span.end - origin.span.start)?;
    let explainer = Explainer::new(sources, lowered);
    let (block, name, settings) = explainer.block(resource)?;
    Some(Explanation {
        block,
        name,
        resource: resource.clone(),
        source_span: explainer.describe(&origin.file, origin.span),
        settings,
    })
}

struct Explainer<'a> {
    sources: &'a Sources,
    lowered: &'a Lowered,
    indexes: BTreeMap<&'a str, LineIndex>,
}

fn route_label(route: &Route) -> String {
    route
        .name
        .clone()
        .unwrap_or_else(|| route.matcher.path.clone())
}

impl<'a> Explainer<'a> {
    fn new(sources: &'a Sources, lowered: &'a Lowered) -> Self {
        Self {
            sources,
            lowered,
            indexes: sources
                .files()
                .map(|(path, text)| (path, LineIndex::new(text)))
                .collect(),
        }
    }

    fn describe(&self, file: &str, span: Span) -> String {
        let text = self.sources.get(file).unwrap_or_default();
        self.indexes
            .get(file)
            .map_or_else(|| file.to_owned(), |index| index.describe(file, text, span))
    }

    fn written(&self, resource: &str) -> &'a [Written] {
        self.lowered
            .written
            .get(resource)
            .map_or(&[], Vec::as_slice)
    }

    fn first(&self, resource: &str, directive: &str) -> Option<&'a Written> {
        self.written(resource)
            .iter()
            .find(|written| written.directive == directive)
    }

    /// What `written` says after its name.
    fn arguments(&self, written: &Written) -> String {
        let text = self.sources.get(&written.file).unwrap_or_default();
        text.get(written.span.range())
            .and_then(|directive| directive.strip_prefix(written.directive))
            .unwrap_or_default()
            .trim()
            .trim_end_matches(';')
            .trim_end()
            .to_owned()
    }

    fn here(&self, written: &Written, context: Context) -> Setting {
        Setting::new(
            written.directive,
            self.arguments(written),
            SettingSource::Here,
        )
        .at(Some(self.describe(&written.file, written.span)))
        .rule(written.directive, context)
    }

    /// The block's own directives other than `id` and `skip`.
    fn own(&self, resource: &str, context: Context, skip: &[&str]) -> Vec<Setting> {
        self.written(resource)
            .iter()
            .filter(|written| written.directive != "id" && !skip.contains(&written.directive))
            .map(|written| self.here(written, context))
            .collect()
    }

    fn or_default(
        &self,
        settings: &mut Vec<Setting>,
        resource: &str,
        context: Context,
        directive: &str,
        value: &str,
    ) {
        if self.first(resource, directive).is_none() {
            settings.push(
                Setting::new(directive, value, SettingSource::Default).rule(directive, context),
            );
        }
    }

    fn all_listeners(&self) -> String {
        self.lowered
            .model
            .listeners
            .iter()
            .map(|listener| listener.id.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn site(&self, id: &str) -> Option<&'a Site> {
        self.lowered
            .model
            .sites
            .iter()
            .find(|site| !site.is_deleted() && site.id.to_string() == id)
    }

    /// How settings name the block at `resource`.
    fn label(&self, resource: &str) -> String {
        let parts: Vec<&str> = resource.split('/').collect();
        let site = |id: &str| self.site(id);
        match parts.as_slice() {
            ["sites", id] => site(id).map(|site| format!("server {}", site.name)),
            ["sites", id, "routes", route] => site(id)
                .and_then(|site| site.routes.iter().find(|r| r.id.to_string() == *route))
                .map(|route| format!("route {}", route_label(route))),
            _ => None,
        }
        .unwrap_or_else(|| resource.to_owned())
    }

    fn block(&self, resource: &str) -> Option<(Context, Option<String>, Vec<Setting>)> {
        let model = &self.lowered.model;
        let parts: Vec<&str> = resource.split('/').collect();
        Some(match parts.as_slice() {
            ["sites", id] => {
                let site = self.site(id)?;
                (Context::Server, Some(site.name.clone()), self.server(site))
            }
            ["sites", id, "routes", route] => {
                let site = self.site(id)?;
                let route = site.routes.iter().find(|r| r.id.to_string() == *route)?;
                (Context::Route, route.name.clone(), self.route(site, route))
            }
            ["listeners", id] => {
                let listener = model.listeners.iter().find(|l| l.id == *id)?;
                (
                    Context::Listener,
                    Some(listener.id.clone()),
                    self.listener(listener),
                )
            }
            ["upstreams", id] => {
                let upstream = model.upstreams.iter().find(|u| u.id.to_string() == *id)?;
                (
                    Context::Upstream,
                    Some(upstream.name.clone()),
                    self.upstream(upstream),
                )
            }
            ["tls-profiles", id] => {
                let profile = model.tls_profiles.iter().find(|p| p.id == *id)?;
                (
                    Context::TlsProfile,
                    Some(profile.id.clone()),
                    self.tls_profile(profile),
                )
            }
            _ => return None,
        })
    }

    fn server(&self, site: &Site) -> Vec<Setting> {
        let resource = format!("sites/{}", site.id);
        let mut settings = self.own(&resource, Context::Server, &[]);
        let all = self.all_listeners();
        for (directive, value) in [
            ("enabled", "on"),
            ("listen", all.as_str()),
            ("https_redirect", "off"),
            ("www_redirect", "off"),
        ] {
            self.or_default(&mut settings, &resource, Context::Server, directive, value);
        }
        self.certificates(&mut settings, site, None);
        self.constants(&mut settings, &resource);
        settings
    }

    fn route(&self, site: &Site, route: &Route) -> Vec<Setting> {
        let resource = format!("sites/{}/routes/{}", site.id, route.id);
        let server = format!("sites/{}", site.id);
        let from = format!("server {}", site.name);
        let mut settings = self.own(&resource, Context::Route, &["enabled"]);
        let own = self.first(&resource, "enabled");
        settings.push(match (route.enabled, site.enabled, own) {
            (true, false, _) => match self.first(&server, "enabled") {
                Some(written) => self.here(written, Context::Route).from(from.clone()),
                None => Setting::new("enabled", "off", SettingSource::Default),
            }
            .inherited(),
            (_, _, Some(written)) => self.here(written, Context::Route),
            _ => Setting::new("enabled", "on", SettingSource::Default)
                .rule("enabled", Context::Route),
        });
        if self.first(&resource, "priority").is_none() {
            settings.push(
                Setting::new(
                    "priority",
                    route.priority.to_string(),
                    SettingSource::Default,
                )
                .rule("priority", Context::Route),
            );
        }
        let all = self.all_listeners();
        for (directive, value) in [
            ("listen", all.as_str()),
            ("https_redirect", "off"),
            ("www_redirect", "off"),
        ] {
            settings.push(match self.first(&server, directive) {
                Some(written) => self
                    .here(written, Context::Server)
                    .from(from.clone())
                    .inherited(),
                None => Setting::new(directive, value, SettingSource::Default)
                    .rule(directive, Context::Server),
            });
        }
        self.certificates(&mut settings, site, Some(route));
        self.constants(&mut settings, &resource);
        settings
    }

    /// The certificate of each host the server, or one of its routes,
    /// answers for over TLS.
    fn certificates(&self, settings: &mut Vec<Setting>, site: &Site, route: Option<&Route>) {
        let tls: Vec<&Listener> = self
            .lowered
            .model
            .listeners
            .iter()
            .filter(|listener| {
                site.listener_ids.is_empty() || site.listener_ids.contains(&listener.id)
            })
            .filter(|listener| listener.tls_profile_id.is_some())
            .collect();
        if tls.is_empty() {
            return;
        }
        let server = format!("sites/{}", site.id);
        let from_server = |setting: Setting| match route {
            Some(_) => setting.from(format!("server {}", site.name)).inherited(),
            None => setting,
        };
        for domain in site.domains.iter().filter(|domain| domain.enabled) {
            if let Some(route) = route {
                let elsewhere = route
                    .matcher
                    .host
                    .as_ref()
                    .is_some_and(|host| *host != domain.host);
                if domain.redirect || elsewhere {
                    continue;
                }
            }
            let host = domain.host.as_str();
            if let Some(profile) = &domain.tls_profile_id {
                let span = self
                    .lowered
                    .origins
                    .get(&format!("{server}/domains/{host}"))
                    .map(|origin| self.describe(&origin.file, origin.span));
                settings.push(from_server(
                    Setting::new("tls_profile", profile, SettingSource::Here)
                        .scoped(host)
                        .at(span)
                        .rule("tls_profile", Context::Server),
                ));
            } else if let Some(profile) = &site.tls_profile_id {
                let span = self
                    .first(&server, "tls_profile")
                    .map(|written| self.describe(&written.file, written.span));
                settings.push(from_server(
                    Setting::new("tls_profile", profile, SettingSource::Here)
                        .scoped(host)
                        .at(span)
                        .rule("tls_profile", Context::Server),
                ));
            } else {
                for listener in &tls {
                    let span = self
                        .first(&format!("listeners/{}", listener.id), "tls_profile")
                        .map(|written| self.describe(&written.file, written.span));
                    settings.push(
                        Setting::new(
                            "tls_profile",
                            listener.tls_profile_id.clone().unwrap_or_default(),
                            SettingSource::Inherited,
                        )
                        .scoped(host)
                        .from(format!("listener {}", listener.id))
                        .at(span)
                        .rule("tls_profile", Context::Server),
                    );
                }
            }
        }
    }

    fn constants(&self, settings: &mut Vec<Setting>, resource: &str) {
        for constant in self.lowered.constants.get(resource).into_iter().flatten() {
            let setting = Setting::new(
                format!("${}", constant.name),
                constant.value.clone(),
                SettingSource::Here,
            )
            .at(Some(self.describe(&constant.file, constant.span)))
            .rule("set", Context::Http);
            settings.push(if constant.block == resource {
                setting
            } else {
                setting.from(self.label(&constant.block)).inherited()
            });
        }
    }

    fn listener(&self, listener: &Listener) -> Vec<Setting> {
        let resource = format!("listeners/{}", listener.id);
        let mut settings = self.own(&resource, Context::Listener, &[]);
        self.or_default(
            &mut settings,
            &resource,
            Context::Listener,
            "protocols",
            "http1 http2",
        );
        self.or_default(
            &mut settings,
            &resource,
            Context::Listener,
            "reuse_port",
            "off",
        );
        self.constants(&mut settings, &resource);
        settings
    }

    fn upstream(&self, upstream: &Upstream) -> Vec<Setting> {
        let resource = format!("upstreams/{}", upstream.id);
        let mut settings = self.own(&resource, Context::Upstream, &[]);
        for (directive, value) in [
            ("balance", "round_robin"),
            ("keepalive", "on"),
            ("http2", "off"),
        ] {
            self.or_default(
                &mut settings,
                &resource,
                Context::Upstream,
                directive,
                value,
            );
        }
        for node in upstream.nodes.iter().filter(|node| node.tls) {
            let target = if node.host.contains(':') {
                format!("[{}]:{}", node.host, node.port)
            } else {
                format!("{}:{}", node.host, node.port)
            };
            let setting = if let Some(sni) = &node.sni {
                let span = self
                    .lowered
                    .origins
                    .get(&format!("{resource}/nodes/{}", node.id))
                    .map(|origin| self.describe(&origin.file, origin.span));
                Setting::new("sni", sni, SettingSource::Here).at(span)
            } else if let Some(sni) = &upstream.tls.sni {
                let span = self
                    .first(&resource, "tls")
                    .map(|written| self.describe(&written.file, written.span));
                Setting::new("sni", sni, SettingSource::Here).at(span)
            } else if node.host.parse::<std::net::IpAddr>().is_ok() {
                Setting::new("sni", "", SettingSource::Default)
            } else {
                Setting::new("sni", &node.host, SettingSource::Default)
            };
            settings.push(setting.scoped(target).rule("tls", Context::Upstream));
        }
        self.constants(&mut settings, &resource);
        settings
    }

    fn tls_profile(&self, profile: &TlsProfile) -> Vec<Setting> {
        let resource = format!("tls-profiles/{}", profile.id);
        let mut settings = self.own(&resource, Context::TlsProfile, &[]);
        self.or_default(
            &mut settings,
            &resource,
            Context::TlsProfile,
            "min_protocol",
            "TLSv1.2",
        );
        self.constants(&mut settings, &resource);
        settings
    }
}
