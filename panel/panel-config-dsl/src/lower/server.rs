//! `server` blocks: domains, redirects and HSTS.

use super::{route::placeholder_action, Lowerer, Origin, ServerDraft};
use crate::{
    codes,
    schema::{Context, DirectiveSpec},
    values::Params,
};
use panel_config_model::{Domain, Site};
use panel_dsl::{Argument, Directive};
use panel_ir::{StrictTransportSecurity, WwwRedirect};
use std::collections::BTreeSet;

impl<'a> Lowerer<'a> {
    pub(super) fn server(&mut self, file: &str, directive: &Directive, depth: usize) {
        let name = Self::literal(&directive.args[0]);
        if self
            .servers
            .iter()
            .any(|draft| draft.site.name.eq_ignore_ascii_case(&name))
        {
            self.error(
                file,
                directive.args[0].span,
                codes::DUPLICATE,
                format!("server {name:?} is defined twice"),
            );
            return;
        }
        let id = self.identity(file, directive, depth);
        let now = self.options.now;
        let mut draft = ServerDraft {
            site: Site {
                lua: Default::default(),
                id,
                name,
                action: placeholder_action(),
                enabled: true,
                domains: Vec::new(),
                routes: Vec::new(),
                listener_ids: BTreeSet::new(),
                https_redirect: false,
                www_redirect: WwwRedirect::None,
                tls_profile_id: None,
                hsts: None,
                group: None,
                tags: BTreeSet::new(),
                note: None,
                favorite: false,
                deleted_at: None,
                created_at: now,
                updated_at: now,
                security_policy_id: None,
                http_policy_id: None,
                access_log: Default::default(),
            },
            action: None,
            routes: Vec::new(),
            origin: Self::origin(file, directive, depth),
        };
        let Some(block) = directive.block() else {
            return;
        };
        let mut explicit_primary = false;
        self.with_scope(format!("sites/{}", draft.site.id), |lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::Server,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, depth| {
                    lowerer.server_directive(
                        file,
                        directive,
                        spec,
                        depth,
                        &mut draft,
                        &mut explicit_primary,
                    );
                },
            );
        });
        if !explicit_primary {
            if let Some(first) = draft
                .site
                .domains
                .iter_mut()
                .find(|domain| !domain.redirect)
            {
                first.primary = true;
            }
        }
        if draft.action.is_none() {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                format!("server {:?} has no action", draft.site.name),
                "add one of proxy, root, return, respond or content_by_lua_block",
            );
        }
        self.origins
            .insert(format!("sites/{}", draft.site.id), draft.origin.clone());
        self.servers.push(draft);
    }

    /// Adds a domain written at `origin`, whose host is `arg`.
    pub(super) fn add_domain(
        &mut self,
        arg: &Argument,
        origin: Origin,
        draft: &mut ServerDraft,
        domain: Domain,
    ) {
        if draft
            .site
            .domains
            .iter()
            .any(|other| other.host == domain.host)
        {
            self.error(
                &origin.file,
                arg.span,
                codes::DUPLICATE,
                format!("{} is listed twice in this server", domain.host),
            );
        } else {
            self.origins.insert(
                format!("sites/{}/domains/{}", draft.site.id, domain.host),
                origin,
            );
            draft.site.domains.push(domain);
        }
    }

    pub(super) fn server_directive(
        &mut self,
        file: &str,
        directive: &Directive,
        spec: &DirectiveSpec,
        depth: usize,
        draft: &mut ServerDraft,
        explicit_primary: &mut bool,
    ) {
        let arg = directive.args.first();
        match spec.name {
            "id" => {}
            "server_name" | "alias" => {
                for arg in &directive.args {
                    let Some(value) = self.value(file, arg) else {
                        continue;
                    };
                    if let Some(host) = self.host(file, arg, &value) {
                        let domain = Domain {
                            host,
                            enabled: true,
                            primary: false,
                            redirect: spec.name == "alias",
                            tls_profile_id: None,
                        };
                        let origin = Origin {
                            file: file.to_owned(),
                            span: arg.span,
                            outer: arg.span,
                            depth,
                        };
                        self.add_domain(arg, origin, draft, domain);
                    }
                }
            }
            "domain" => {
                let params = Params::split(&directive.args);
                let Some((host_arg, flags)) = params.positional.split_first() else {
                    return;
                };
                let Some(value) = self.value(file, host_arg) else {
                    return;
                };
                let Some(host) = self.host(file, host_arg, &value) else {
                    return;
                };
                let mut domain = Domain {
                    host,
                    enabled: true,
                    primary: false,
                    redirect: false,
                    tls_profile_id: None,
                };
                for flag in flags {
                    match flag.value.as_str() {
                        "primary" => {
                            domain.primary = true;
                            *explicit_primary = true;
                        }
                        "alias" => domain.redirect = true,
                        "off" => domain.enabled = false,
                        other => self.error_with_help(
                            file,
                            flag.span,
                            codes::ARGUMENTS,
                            format!("unknown domain flag {other:?}"),
                            "expected primary, alias or off",
                        ),
                    }
                }
                self.only_params(file, &params, &["tls_profile"]);
                if let Some((value, _)) = params.named.get("tls_profile") {
                    domain.tls_profile_id = Some((*value).to_owned());
                }
                self.add_domain(
                    host_arg,
                    Self::origin(file, directive, depth),
                    draft,
                    domain,
                );
            }
            "listen" => draft
                .site
                .listener_ids
                .extend(directive.args.iter().map(Self::literal)),
            "tls_profile" => draft.site.tls_profile_id = arg.map(Self::literal),
            "security_policy" => draft.site.security_policy_id = arg.map(Self::literal),
            "http_policy" => draft.site.http_policy_id = arg.map(Self::literal),
            "https_redirect" => {
                draft.site.https_redirect = arg
                    .and_then(|arg| self.bool_arg(file, arg))
                    .unwrap_or_default()
            }
            "hsts" => draft.site.hsts = self.hsts(file, directive),
            "access_log" => self.access_log(file, directive, &mut draft.site.access_log),
            "log_field" => self.log_field(file, directive, &mut draft.site.access_log),
            "www_redirect" => {
                let Some(arg) = arg else { return };
                draft.site.www_redirect = match arg.value.as_str() {
                    "off" => WwwRedirect::None,
                    "add" => WwwRedirect::AddWww,
                    "remove" => WwwRedirect::RemoveWww,
                    other => {
                        self.error(
                            file,
                            arg.span,
                            codes::TYPE,
                            format!("{other:?} is not off, add or remove"),
                        );
                        return;
                    }
                };
            }
            "enabled" => {
                draft.site.enabled = arg.and_then(|arg| self.bool_arg(file, arg)).unwrap_or(true)
            }
            "group" => draft.site.group = arg.map(Self::literal),
            "tags" => draft
                .site
                .tags
                .extend(directive.args.iter().map(Self::literal)),
            "note" => draft.site.note = arg.map(Self::literal),
            "route" | "location" => {
                if let Some(route) = self.route(file, directive, depth, &draft.site) {
                    draft.routes.push(route);
                }
            }
            name if super::lua::SCOPE.contains(&name) => {
                self.lua_scope(file, directive, &mut draft.site.lua, "the server");
            }
            name if super::lua::inert(name).is_some() => self.lua_inert(file, directive),
            action => {
                let Some(found) = self.action(file, directive, action) else {
                    return;
                };
                if draft.action.is_some() {
                    self.error(
                        file,
                        directive.name.span,
                        codes::DUPLICATE,
                        "the server already has an action",
                    );
                } else {
                    draft.action = Some(found);
                }
            }
        }
    }

    /// `hsts max_age=<duration> [include_subdomains] [preload];` or `hsts off;`.
    pub(super) fn hsts(
        &mut self,
        file: &str,
        directive: &Directive,
    ) -> Option<StrictTransportSecurity> {
        let params = Params::split(&directive.args);
        if params.named.is_empty()
            && matches!(params.positional.as_slice(), [arg] if arg.value == "off")
        {
            return None;
        }
        self.only_params(file, &params, &["max_age"]);
        let mut policy = StrictTransportSecurity {
            max_age_seconds: 0,
            include_subdomains: false,
            preload: false,
        };
        match params.named.get("max_age") {
            Some((value, arg)) => {
                policy.max_age_seconds = self.duration(file, arg, value)? / 1_000;
            }
            None => {
                self.error(
                    file,
                    directive.span,
                    codes::ARGUMENTS,
                    "hsts needs max_age=, such as max_age=365d",
                );
                return None;
            }
        }
        for flag in &params.positional {
            match flag.value.as_str() {
                "include_subdomains" => policy.include_subdomains = true,
                "preload" => policy.preload = true,
                other => self.error_with_help(
                    file,
                    flag.span,
                    codes::ARGUMENTS,
                    format!("unknown hsts flag {other:?}"),
                    "expected include_subdomains or preload",
                ),
            }
        }
        Some(policy)
    }
}
