//! Lua scripts (ADR 0039): the code each level of the configuration runs and
//! the terms it runs on. A handler or term a level sets replaces the one of
//! the level around it, as NGINX inherits `*_by_lua*` directives: `http`,
//! then the site, then the route.

use crate::model::{Action, ConfigModel};
use panel_domain::ContentHash;
pub use panel_ir::{LuaFallback, LuaLogLevel, LuaPermissions, LuaSharedDict};
use panel_ir::{LuaHandler, LuaHandlers, LuaScript};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The directory of the configuration that holds its Lua files.
pub const LUA_DIRECTORY: &str = "lua/";

fn is_false(value: &bool) -> bool {
    !*value
}

const fn first_line() -> u32 {
    1
}

fn is_first_line(line: &u32) -> bool {
    *line == 1
}

/// A handler's code.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LuaCode {
    /// Code written where the handler is declared.
    Inline {
        code: String,
        /// The configuration file it is written in, for messages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        /// The line of `file` the code starts on.
        #[serde(default = "first_line", skip_serializing_if = "is_first_line")]
        line: u32,
    },
    /// A file of the configuration under `lua/`.
    File { path: String },
}

impl LuaCode {
    pub fn inline(code: impl Into<String>) -> Self {
        Self::Inline {
            code: code.into(),
            file: None,
            line: 1,
        }
    }

    pub fn file(path: impl Into<String>) -> Self {
        Self::File { path: path.into() }
    }
}

/// What one level runs and the terms its handlers run on; what it leaves
/// unset comes from the level around it.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LuaScope {
    /// Before the route is chosen; `http` and sites only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_rewrite: Option<LuaCode>,
    /// After the route is chosen, before security policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrite: Option<LuaCode>,
    /// After security policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<LuaCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_filter: Option<LuaCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_filter: Option<LuaCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<LuaCode>,
    /// Wall-clock milliseconds a run may take, waits included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_limit_ms: Option<u64>,
    /// Function calls and loop iterations a run may make.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<LuaPermissions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_error: Option<LuaFallback>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<LuaLogLevel>,
    /// Runs longer than this many milliseconds are logged and counted as
    /// slow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_threshold_ms: Option<u64>,
    /// Logs the start, end, duration and outcome of every run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
}

impl LuaScope {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// This level's handlers and terms over those of `outer`.
    pub fn over(&self, outer: &Self) -> Self {
        fn pick<T: Clone>(inner: &Option<T>, outer: &Option<T>) -> Option<T> {
            inner.clone().or_else(|| outer.clone())
        }
        Self {
            server_rewrite: pick(&self.server_rewrite, &outer.server_rewrite),
            rewrite: pick(&self.rewrite, &outer.rewrite),
            access: pick(&self.access, &outer.access),
            header_filter: pick(&self.header_filter, &outer.header_filter),
            body_filter: pick(&self.body_filter, &outer.body_filter),
            log: pick(&self.log, &outer.log),
            time_limit_ms: pick(&self.time_limit_ms, &outer.time_limit_ms),
            work_limit: pick(&self.work_limit, &outer.work_limit),
            allow: pick(&self.allow, &outer.allow),
            on_error: pick(&self.on_error, &outer.on_error),
            log_level: pick(&self.log_level, &outer.log_level),
            slow_threshold_ms: pick(&self.slow_threshold_ms, &outer.slow_threshold_ms),
            debug: pick(&self.debug, &outer.debug),
        }
    }

    /// The handlers it declares, with the names of their phases.
    pub fn handlers(&self) -> impl Iterator<Item = (&'static str, &LuaCode)> {
        [
            ("server_rewrite", &self.server_rewrite),
            ("rewrite", &self.rewrite),
            ("access", &self.access),
            ("header_filter", &self.header_filter),
            ("body_filter", &self.body_filter),
            ("log", &self.log),
        ]
        .into_iter()
        .filter_map(|(phase, code)| code.as_ref().map(|code| (phase, code)))
    }

    fn handlers_mut(&mut self) -> impl Iterator<Item = &mut LuaCode> {
        [
            &mut self.server_rewrite,
            &mut self.rewrite,
            &mut self.access,
            &mut self.header_filter,
            &mut self.body_filter,
            &mut self.log,
        ]
        .into_iter()
        .flatten()
    }

    /// Whether it sets any term, as opposed to only handlers.
    pub fn has_terms(&self) -> bool {
        self.time_limit_ms.is_some()
            || self.work_limit.is_some()
            || self.allow.is_some()
            || self.on_error.is_some()
            || self.log_level.is_some()
            || self.slow_threshold_ms.is_some()
            || self.debug.is_some()
    }
}

/// Lua across the configuration: what runs once in each VM, the shared
/// dictionaries, what every site runs and the files scripts load.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LuaConfig {
    /// `lua off`: the scripts are kept and checked, but none runs.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    /// Runs once in each VM when the configuration is activated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init: Option<LuaCode>,
    /// Runs once in each VM after `init`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_worker: Option<LuaCode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_dicts: Vec<LuaSharedDict>,
    /// Bytes each VM may allocate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit_bytes: Option<u64>,
    /// What every site runs, and the terms of every handler; balancers and
    /// `init` run on these terms.
    #[serde(default, skip_serializing_if = "LuaScope::is_empty")]
    pub http: LuaScope,
    /// The configuration's files under `lua/` by path: the files handlers
    /// name and the modules `require` loads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
}

impl LuaConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Every handler of `http`, the upstreams and the live sites: the resource
/// it is declared on (`lua` for `http`), its phase and its code.
pub fn lua_handlers(model: &ConfigModel) -> Vec<(String, &'static str, &LuaCode)> {
    let lua = &model.lua;
    let mut found: Vec<(String, &'static str, &LuaCode)> = Vec::new();
    for (phase, code) in [("init", &lua.init), ("init_worker", &lua.init_worker)] {
        if let Some(code) = code {
            found.push(("lua".into(), phase, code));
        }
    }
    found.extend(
        lua.http
            .handlers()
            .map(|(phase, code)| ("lua".to_owned(), phase, code)),
    );
    for upstream in &model.upstreams {
        if let Some(code) = &upstream.balancer {
            found.push((format!("upstreams/{}", upstream.id), "balancer", code));
        }
    }
    for site in model.sites.iter().filter(|site| !site.is_deleted()) {
        let resource = format!("sites/{}", site.id);
        found.extend(
            site.lua
                .handlers()
                .map(|(phase, code)| (resource.clone(), phase, code)),
        );
        if let Action::Lua { code } = &site.action {
            found.push((resource.clone(), "content", code));
        }
        for route in &site.routes {
            let resource = format!("{resource}/routes/{}", route.id);
            found.extend(
                route
                    .lua
                    .handlers()
                    .map(|(phase, code)| (resource.clone(), phase, code)),
            );
            if let Action::Lua { code } = &route.action {
                found.push((resource, "content", code));
            }
        }
    }
    found
}

/// Every handler's code in `model`, deleted sites included.
pub fn lua_codes_mut(model: &mut ConfigModel) -> Vec<&mut LuaCode> {
    let lua = &mut model.lua;
    let mut found: Vec<&mut LuaCode> = lua
        .init
        .iter_mut()
        .chain(lua.init_worker.iter_mut())
        .collect();
    found.extend(lua.http.handlers_mut());
    found.extend(
        model
            .upstreams
            .iter_mut()
            .filter_map(|upstream| upstream.balancer.as_mut()),
    );
    for site in &mut model.sites {
        found.extend(site.lua.handlers_mut());
        if let Action::Lua { code } = &mut site.action {
            found.push(code);
        }
        for route in &mut site.routes {
            found.extend(route.lua.handlers_mut());
            if let Action::Lua { code } = &mut route.action {
                found.push(code);
            }
        }
    }
    found
}

/// The name `require` loads a file under `lua/` by: `lua/a/b.lua` is `a.b`
/// and `lua/a/init.lua` is `a`, as `?.lua;?/init.lua` resolve in OpenResty.
pub fn module_name(path: &str) -> Option<String> {
    let relative = path.strip_prefix(LUA_DIRECTORY)?.strip_suffix(".lua")?;
    let relative = relative.strip_suffix("/init").unwrap_or(relative);
    let valid = !relative.is_empty()
        && relative.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        });
    valid.then(|| relative.replace('/', "."))
}

/// Scripts of a snapshot being compiled, by id.
#[derive(Default)]
pub(crate) struct Scripts {
    scripts: BTreeMap<String, LuaScript>,
}

impl Scripts {
    /// The id of the script `code` is, added on first use. A file the model
    /// lacks is reported by validation, so it compiles as empty here.
    pub(crate) fn add(&mut self, code: &LuaCode, files: &BTreeMap<String, String>) -> String {
        let (id, file, line, source) = match code {
            LuaCode::Inline { code, file, line } => {
                let id = match file {
                    Some(file) => format!("{file}:{line}"),
                    None => format!("inline:{}", &content_hash(code)[..16]),
                };
                let file = file.clone().unwrap_or_else(|| "inline".into());
                (id, file, *line, code.clone())
            }
            LuaCode::File { path } => (
                path.clone(),
                path.clone(),
                1,
                files.get(path).cloned().unwrap_or_default(),
            ),
        };
        self.scripts.entry(id.clone()).or_insert_with(|| LuaScript {
            id: id.clone(),
            file,
            line,
            sha256: content_hash(&source),
            module: None,
            source,
        });
        id
    }

    /// Adds every file as the module `require` loads it by.
    pub(crate) fn modules(&mut self, files: &BTreeMap<String, String>) {
        for (path, source) in files {
            let Some(name) = module_name(path) else {
                continue;
            };
            let script = self
                .scripts
                .entry(path.clone())
                .or_insert_with(|| LuaScript {
                    id: path.clone(),
                    file: path.clone(),
                    line: 1,
                    sha256: content_hash(source),
                    module: None,
                    source: source.clone(),
                });
            script.module = Some(name);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.scripts.is_empty()
    }

    pub(crate) fn into_scripts(self) -> Vec<LuaScript> {
        self.scripts.into_values().collect()
    }
}

fn content_hash(source: &str) -> String {
    ContentHash::from_bytes(source.as_bytes())
        .as_str()
        .to_owned()
}

/// The handler running `id` on the terms of `scope`.
pub(crate) fn handler(id: String, scope: &LuaScope) -> LuaHandler {
    let mut handler = LuaHandler::new(id);
    handler.time_limit_ms = scope.time_limit_ms.unwrap_or(0);
    handler.work_limit = scope.work_limit.unwrap_or(0);
    handler.allow = scope.allow.unwrap_or_default();
    handler.on_error = scope.on_error.unwrap_or_default();
    handler.log_level = scope.log_level.unwrap_or_default();
    handler.slow_threshold_ms = scope.slow_threshold_ms.unwrap_or(0);
    handler.debug = scope.debug.unwrap_or(false);
    handler
}

/// The handlers `scope` runs; a route's never include `server_rewrite`.
pub(crate) fn handlers(
    scope: &LuaScope,
    scripts: &mut Scripts,
    files: &BTreeMap<String, String>,
    route: bool,
) -> LuaHandlers {
    let mut compile = |code: &Option<LuaCode>| {
        code.as_ref()
            .map(|code| handler(scripts.add(code, files), scope))
    };
    LuaHandlers {
        server_rewrite: if route {
            None
        } else {
            compile(&scope.server_rewrite)
        },
        rewrite: compile(&scope.rewrite),
        access: compile(&scope.access),
        header_filter: compile(&scope.header_filter),
        body_filter: compile(&scope.body_filter),
        log: compile(&scope.log),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_under_lua_are_modules_by_their_path() {
        assert_eq!(module_name("lua/auth.lua").as_deref(), Some("auth"));
        assert_eq!(module_name("lua/a/b-c.lua").as_deref(), Some("a.b-c"));
        assert_eq!(module_name("lua/a/init.lua").as_deref(), Some("a"));
        assert_eq!(module_name("lua/init.lua").as_deref(), Some("init"));
        assert_eq!(module_name("lua/a.b.lua"), None);
        assert_eq!(module_name("other/a.lua"), None);
        assert_eq!(module_name("lua/a.txt"), None);
    }

    #[test]
    fn inner_levels_replace_what_they_set() {
        let outer = LuaScope {
            access: Some(LuaCode::inline("a")),
            log: Some(LuaCode::inline("l")),
            time_limit_ms: Some(50),
            ..LuaScope::default()
        };
        let inner = LuaScope {
            access: Some(LuaCode::file("lua/b.lua")),
            time_limit_ms: Some(5),
            debug: Some(true),
            ..LuaScope::default()
        };
        let merged = inner.over(&outer);
        assert_eq!(merged.access, Some(LuaCode::file("lua/b.lua")));
        assert_eq!(merged.log, Some(LuaCode::inline("l")));
        assert_eq!(merged.time_limit_ms, Some(5));
        assert_eq!(merged.debug, Some(true));
        assert_eq!(
            merged
                .handlers()
                .map(|(phase, _)| phase)
                .collect::<Vec<_>>(),
            ["access", "log"]
        );
    }

    #[test]
    fn scripts_are_identified_by_place_or_file() {
        let files = BTreeMap::from([("lua/m.lua".to_owned(), "return 1".to_owned())]);
        let mut scripts = Scripts::default();
        let inline = LuaCode::Inline {
            code: "ngx.exit(403)".into(),
            file: Some("main.conf".into()),
            line: 7,
        };
        assert_eq!(scripts.add(&inline, &files), "main.conf:7");
        assert_eq!(scripts.add(&inline, &files), "main.conf:7");
        assert!(scripts
            .add(&LuaCode::inline("x()"), &files)
            .starts_with("inline:"));
        assert_eq!(
            scripts.add(&LuaCode::file("lua/m.lua"), &files),
            "lua/m.lua"
        );
        scripts.modules(&files);
        let scripts = scripts.into_scripts();
        assert_eq!(scripts.len(), 3);
        let module = scripts
            .iter()
            .find(|script| script.id == "lua/m.lua")
            .unwrap();
        assert_eq!(module.module.as_deref(), Some("m"));
        assert_eq!(module.source, "return 1");
        assert_eq!(module.sha256.len(), 64);
    }
}
