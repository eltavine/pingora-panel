//! The configuration's Lua scripts checked with the gateway's compiler
//! (ADR 0039): what does not compile, then what checking finds without
//! running them — modules `require` cannot load, `ngx` functions the gateway
//! does not provide or a phase does not allow, and globals scripts write —
//! and files under `lua/` that no handler runs and no script loads.

use crate::codes;
use panel_config_model::{lua_handlers, module_name, ConfigModel, LuaCode};
use panel_errors::Diagnostic;
use panel_lua::{FindingKind, Phase, Role, Source};
use std::collections::BTreeSet;

fn phase(name: &str) -> Phase {
    match name {
        "init" => Phase::Init,
        "init_worker" => Phase::InitWorker,
        "server_rewrite" => Phase::ServerRewrite,
        "rewrite" => Phase::Rewrite,
        "access" => Phase::Access,
        "content" => Phase::Content,
        "balancer" => Phase::Balancer,
        "header_filter" => Phase::HeaderFilter,
        "body_filter" => Phase::BodyFilter,
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
