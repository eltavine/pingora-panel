//! The configuration's Lua scripts checked with the gateway's compiler
//! (ADR 0039): what does not compile, then what checking finds without
//! running them — modules `require` cannot load, `ngx` functions the gateway
//! does not provide or a phase does not allow, and globals scripts write —
//! and files under `lua/` that no handler runs and no script loads. The
//! library lists every script with where it runs and its version.

use crate::{codes, Lowered};
use panel_config_model::{
    lua_handlers, module_name, script_id, ConfigModel, LuaCode, LuaSharedDict,
};
use panel_domain::ContentHash;
use panel_errors::Diagnostic;
use panel_lua::{FindingKind, Phase, Role, Source};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Where a script runs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaUse {
    /// `lua` for `http`, or `sites/<id>`, `sites/<id>/routes/<id>` or
    /// `upstreams/<id>`.
    pub resource: String,
    /// How the configuration names it, such as `server shop`.
    pub label: String,
    /// `init`, `init_worker`, `server_rewrite`, `rewrite`, `access`,
    /// `content`, `balancer`, `header_filter`, `body_filter` or `log`.
    pub phase: String,
}

/// A script of the configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaScriptInfo {
    /// The file's path, or `file:line` of code written in a block.
    pub id: String,
    pub file: String,
    /// The line of `file` it starts on.
    pub line: u32,
    /// What `require` loads it by: files under `lua/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Lowercase hexadecimal SHA-256 of the code, which names its version.
    pub sha256: String,
    pub bytes: usize,
    pub lines: usize,
    pub code: String,
    pub uses: Vec<LuaUse>,
    /// The modules it loads by name.
    pub requires: Vec<String>,
}

/// The Lua of a configuration.
#[derive(Clone, Debug, Serialize)]
pub struct LuaLibrary {
    /// `lua off`: the scripts are kept, but none runs.
    pub disabled: bool,
    pub scripts: Vec<LuaScriptInfo>,
    pub shared_dicts: Vec<LuaSharedDict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_limit_bytes: Option<u64>,
    /// What checking found in the scripts and the Lua directives.
    pub diagnostics: Vec<Diagnostic>,
}

/// A module `require` finds without a file of the configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaBuiltInModule {
    /// What `require` loads it by.
    pub name: String,
    /// The library it stands in for: an OpenResty library such as
    /// `lua-resty-redis`, `luajit` for LuaJIT's extensions, or `panel` for
    /// the gateway's own.
    pub library: String,
}

/// An OpenResty module scripts may not load.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaRefusedModule {
    pub name: String,
    pub reason: String,
}

/// The modules scripts load without a file of the configuration, and
/// those they may not load.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaModules {
    pub built_in: Vec<LuaBuiltInModule>,
    pub refused: Vec<LuaRefusedModule>,
}

/// The modules the gateway's runtime provides and refuses.
pub fn lua_modules() -> LuaModules {
    LuaModules {
        built_in: panel_lua::module_catalog()
            .iter()
            .map(|(name, library)| LuaBuiltInModule {
                name: (*name).to_owned(),
                library: (*library).to_owned(),
            })
            .collect(),
        refused: panel_lua::refused_modules()
            .iter()
            .map(|(name, reason)| LuaRefusedModule {
                name: (*name).to_owned(),
                reason: (*reason).to_owned(),
            })
            .collect(),
    }
}

/// How the configuration names `resource` of `model`.
fn label(model: &ConfigModel, resource: &str) -> String {
    let parts: Vec<&str> = resource.split('/').collect();
    let site = |id: &str| model.sites.iter().find(|site| site.id.to_string() == id);
    match parts.as_slice() {
        ["lua"] => Some("http".to_owned()),
        ["sites", id] => site(id).map(|site| format!("server {}", site.name)),
        ["sites", id, "routes", route_id] => site(id).and_then(|site| {
            let route = site
                .routes
                .iter()
                .find(|route| route.id.to_string() == *route_id)?;
            Some(format!(
                "server {}, route {}",
                site.name,
                route
                    .name
                    .clone()
                    .unwrap_or_else(|| route.matcher.path.clone())
            ))
        }),
        ["upstreams", id] => model
            .upstreams
            .iter()
            .find(|upstream| upstream.id.to_string() == *id)
            .map(|upstream| format!("upstream {}", upstream.name)),
        _ => None,
    }
    .unwrap_or_else(|| resource.to_owned())
}

/// Every script of `lowered` with where it runs, and the diagnostics about
/// Lua among those reading it found.
pub fn lua_library(lowered: &Lowered) -> LuaLibrary {
    let model = &lowered.model;
    let files = &model.lua.files;
    let mut scripts: BTreeMap<String, LuaScriptInfo> = BTreeMap::new();
    let mut roles: BTreeMap<String, Role> = BTreeMap::new();
    fn entry<'s>(
        scripts: &'s mut BTreeMap<String, LuaScriptInfo>,
        id: String,
        file: String,
        line: u32,
        code: &str,
    ) -> &'s mut LuaScriptInfo {
        scripts.entry(id.clone()).or_insert_with(|| LuaScriptInfo {
            id,
            module: module_name(&file),
            file,
            line,
            sha256: ContentHash::from_bytes(code.as_bytes()).as_str().to_owned(),
            bytes: code.len(),
            lines: code.lines().count(),
            code: code.to_owned(),
            uses: Vec::new(),
            requires: Vec::new(),
        })
    }
    for (resource, name, code) in lua_handlers(model) {
        let id = script_id(code);
        let info = match code {
            LuaCode::Inline { code, file, line } => entry(
                &mut scripts,
                id.clone(),
                file.clone().unwrap_or_else(|| "inline".into()),
                *line,
                code,
            ),
            LuaCode::File { path } => entry(
                &mut scripts,
                id.clone(),
                path.clone(),
                1,
                files.get(path).map_or("", String::as_str),
            ),
        };
        info.uses.push(LuaUse {
            label: label(model, &resource),
            resource,
            phase: name.to_owned(),
        });
        roles.entry(id).or_insert(Role::Handler(phase(name)));
    }
    for (path, text) in files {
        entry(&mut scripts, path.clone(), path.clone(), 1, text);
    }
    let known: BTreeSet<String> = files.keys().filter_map(|path| module_name(path)).collect();
    for info in scripts.values_mut() {
        let role = roles.get(&info.id).copied().unwrap_or(Role::Module);
        let source = Source::new(info.file.as_str(), info.code.as_str(), info.line);
        info.requires = panel_lua::lint(&source, role, &|name| known.contains(name))
            .requires
            .into_iter()
            .collect();
    }
    let diagnostics = lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code.as_str() == codes::LUA
                || diagnostic
                    .source_span
                    .as_deref()
                    .is_some_and(|span| span.starts_with("lua/"))
                || diagnostic
                    .resource_id
                    .as_deref()
                    .is_some_and(|resource| resource == "lua" || resource.starts_with("lua/"))
                || diagnostic.message.to_ascii_lowercase().contains("lua")
        })
        .cloned()
        .collect();
    LuaLibrary {
        disabled: model.lua.disabled,
        scripts: scripts.into_values().collect(),
        shared_dicts: model.lua.shared_dicts.clone(),
        memory_limit_bytes: model.lua.memory_limit_bytes,
        diagnostics,
    }
}

fn phase(name: &str) -> Phase {
    match name {
        "init" => Phase::Init,
        "init_worker" => Phase::InitWorker,
        "exit_worker" => Phase::ExitWorker,
        "server_rewrite" => Phase::ServerRewrite,
        "rewrite" => Phase::Rewrite,
        "access" => Phase::Access,
        "precontent" => Phase::Precontent,
        "set" => Phase::Set,
        "content" => Phase::Content,
        "balancer" => Phase::Balancer,
        "header_filter" => Phase::HeaderFilter,
        "body_filter" => Phase::BodyFilter,
        "ssl_client_hello" => Phase::SslClientHello,
        "ssl_cert" => Phase::SslCertificate,
        "ssl_session_fetch" => Phase::SslSessionFetch,
        "ssl_session_store" => Phase::SslSessionStore,
        "proxy_ssl_cert" => Phase::ProxySslCertificate,
        "proxy_ssl_verify" => Phase::ProxySslVerify,
        _ => Phase::Log,
    }
}

/// Every problem of the model's scripts, with the file and line it is at.
pub(crate) fn check(model: &ConfigModel, report: &mut dyn FnMut(&str, Option<u32>, Diagnostic)) {
    let files = &model.lua.files;
    let modules: BTreeSet<String> = files.keys().filter_map(|path| module_name(path)).collect();
    let known = |name: &str| modules.contains(name);
    let mut seen = BTreeSet::new();
    let mut handled = BTreeSet::new();
    let mut required = BTreeSet::new();
    let mut dynamic = false;
    let mut lint =
        |source: &Source, role: Role, report: &mut dyn FnMut(&str, Option<u32>, Diagnostic)| {
            let found = panel_lua::lint(source, role, &known);
            required.extend(found.requires);
            dynamic |= found.dynamic_require;
            for finding in found.findings {
                if !seen.insert((source.name.clone(), finding.line, finding.message.clone())) {
                    continue;
                }
                let help = match finding.kind {
                    FindingKind::UnknownModule => {
                        "add the module under lua/, or load one that is built in"
                    }
                    FindingKind::Unavailable => "the call raises an error when it runs",
                    FindingKind::NotInPhase => "move the call to a phase that allows it",
                    _ => "declare it with local",
                };
                report(
                    &source.name,
                    Some(finding.line),
                    Diagnostic::warning(codes::LUA, finding.message).with_help(help),
                );
            }
        };
    let compiled = |source: &Source, report: &mut dyn FnMut(&str, Option<u32>, Diagnostic)| {
        match panel_lua::compile(source) {
            Ok(_) => true,
            Err(error) => {
                report(
                    &error.source,
                    error.line,
                    Diagnostic::error(
                        codes::LUA,
                        format!("the Lua code does not compile: {}", error.message),
                    ),
                );
                false
            }
        }
    };
    for (_, name, code) in lua_handlers(model) {
        let role = Role::Handler(phase(name));
        match code {
            LuaCode::Inline { code, file, line } => {
                let source = Source::new(file.as_deref().unwrap_or("inline"), code.as_str(), *line);
                if compiled(&source, report) {
                    lint(&source, role, report);
                }
            }
            LuaCode::File { path } => {
                handled.insert(path.clone());
                if let Some(text) = files.get(path) {
                    if panel_lua::compile(&Source::new(path.as_str(), text.as_str(), 1)).is_ok() {
                        lint(&Source::new(path.as_str(), text.as_str(), 1), role, report);
                    }
                }
            }
        }
    }
    for (path, text) in files {
        let source = Source::new(path.as_str(), text.as_str(), 1);
        if compiled(&source, report) && !handled.contains(path) {
            lint(&source, Role::Module, report);
        }
    }
    if dynamic {
        return;
    }
    for path in files.keys() {
        let loaded = module_name(path).is_some_and(|name| required.contains(&name));
        if !handled.contains(path) && !loaded {
            report(
                path,
                None,
                Diagnostic::warning(
                    codes::NO_EFFECT,
                    format!("{path} is not used: no handler runs it and no script requires it"),
                )
                .with_help("run it with a *_by_lua_file directive, require it, or remove it"),
            );
        }
    }
}
