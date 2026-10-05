#![forbid(unsafe_code)]

use chrono::Utc;
use panel_config_dsl::{
    codes, explain, format_files, lower, print, print_sources, reconcile, LowerOptions, Lowered,
    Sources,
};
use panel_config_model::{compile, Action, LuaCode, LuaFallback, LuaLogLevel};
use panel_domain::RevisionId;
use std::collections::BTreeMap;

const MAIN: &str = r#"language_version 1;

http {
    lua_shared_dict hits 1m;
    lua_memory_limit 32m;
    lua_time_limit 50ms;
    lua_allow upstream;
    init_by_lua_block {
        local limits = { per_minute = 60 }
        LIMITS = limits
    }
    access_by_lua_file lua/auth.lua;
    log_by_lua_block { local n = ngx.shared.hits:incr("all", 1, 0) }

    listener http {
        address 0.0.0.0:80;
    }

    upstream app {
        server 10.0.0.11:8080;
        balancer_by_lua_file lua/pick.lua;
    }

    server shop {
        server_name shop.example;
        lua_on_error continue;
        lua_log_level warn;
        header_filter_by_lua_block {
            ngx.header["X-Served-By"] = "shop" -- } in a comment
        }
        proxy app;

        route hello {
            match exact /hello;
            lua_allow body;
            lua_debug on;
            content_by_lua_block {
                local t = { "{", [[}]] }
                ngx.say("hello ", t[1])
            }
        }
    }
}
"#;

const AUTH: &str =
    "local keys = require(\"auth.keys\")\nif not keys.allowed() then return ngx.exit(403) end\n";

fn sources() -> Sources {
    let mut sources = Sources::single(MAIN);
    sources.insert("lua/auth.lua", AUTH);
    sources.insert(
        "lua/auth/keys.lua",
        "return { allowed = function() return true end }\n",
    );
    sources.insert(
        "lua/pick.lua",
        "require(\"ngx.balancer\").set_current_peer(\"10.0.0.12\", 8080)\n",
    );
    sources
}

fn read(sources: &Sources) -> Lowered {
    lower(
        sources,
        &LowerOptions {
            environment: &BTreeMap::new(),
            previous: None,
            now: Utc::now(),
        },
    )
}

fn messages(lowered: &Lowered) -> Vec<(String, String, String)> {
    lowered
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code.as_str().to_owned(),
                diagnostic.source_span.clone().unwrap_or_default(),
                diagnostic.message.clone(),
            )
        })
        .collect()
}

fn has(found: &[(String, String, String)], code: &str, at: &str, message: &str) -> bool {
    found.iter().any(|(found_code, span, text)| {
        found_code == code && span.starts_with(at) && text.contains(message)
    })
}

#[test]
fn lua_directives_read_into_the_model_at_each_level() {
    let lowered = read(&sources());
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let lua = &lowered.model.lua;
    assert!(!lua.disabled);
    assert_eq!(lua.memory_limit_bytes, Some(32 << 20));
    assert_eq!(
        (
            lua.shared_dicts[0].name.as_str(),
            lua.shared_dicts[0].capacity_bytes
        ),
        ("hits", 1 << 20)
    );
    assert_eq!(lua.http.time_limit_ms, Some(50));
    assert!(lua.http.allow.unwrap().upstream);
    assert_eq!(lua.http.access, Some(LuaCode::file("lua/auth.lua")));
    let Some(LuaCode::Inline { code, file, line }) = &lua.init else {
        panic!("init is inline");
    };
    assert!(code.contains("LIMITS = limits"));
    assert_eq!((file.as_deref(), *line), (Some("main.conf"), 8));
    assert_eq!(lua.files.len(), 3);
    assert_eq!(lua.files["lua/auth.lua"], AUTH);
    assert_eq!(
        lowered.model.upstreams[0].balancer,
        Some(LuaCode::file("lua/pick.lua"))
    );
    let site = &lowered.model.sites[0];
    assert_eq!(site.lua.on_error, Some(LuaFallback::Continue));
    assert_eq!(site.lua.log_level, Some(LuaLogLevel::Warn));
    let Some(LuaCode::Inline { code, .. }) = &site.lua.header_filter else {
        panic!("the header filter is inline");
    };
    assert!(code.contains("-- } in a comment"));
    let route = &site.routes[0];
    assert_eq!(route.lua.debug, Some(true));
    assert!(route.lua.allow.unwrap().body);
    let Action::Lua {
        code: LuaCode::Inline { code, .. },
    } = &route.action
    else {
        panic!("the route answers with Lua");
    };
    assert!(code.contains("[[}]]"));

    let snapshot = compile(&lowered.model, RevisionId::new(1)).unwrap();
    let modules: Vec<_> = snapshot
        .lua
        .scripts
        .iter()
        .filter_map(|script| script.module.as_deref())
        .collect();
    assert_eq!(modules, ["auth", "auth.keys", "pick"]);
    let init = snapshot.lua.init.as_ref().unwrap();
    let script = snapshot.lua.script(&init.script_id).unwrap();
    assert_eq!((script.file.as_str(), script.line), ("main.conf", 8));
}

#[test]
fn lua_prints_back_as_written() {
    let first = read(&sources());
    let printed = print_sources(&first.model);
    for (path, text) in sources()
        .files()
        .filter(|(path, _)| path.starts_with("lua/"))
    {
        assert_eq!(printed.get(path), Some(text), "{path}");
    }
    let main = printed.get("main.conf").unwrap();
    let second = read(&printed);
    assert!(second.is_valid(), "{:#?}\n{main}", second.diagnostics);
    for expected in [
        "    lua_memory_limit 32m;\n    lua_shared_dict hits 1m;\n    lua_time_limit 50ms;\n    lua_allow upstream;\n    init_by_lua_block {\n        local limits",
        "    access_by_lua_file lua/auth.lua;\n    log_by_lua_block { local n",
        "        balancer_by_lua_file lua/pick.lua;\n",
        "        lua_on_error continue;\n        lua_log_level warn;\n        header_filter_by_lua_block {\n",
        "            content_by_lua_block {\n                local t = { \"{\", [[}]] }\n",
    ] {
        assert!(main.contains(expected), "{expected}\n{main}");
    }
    assert_eq!(print(&second.model), print(&first.model));
    assert_eq!(print_sources(&second.model), printed);
    let (formatted, problems) = format_files(&printed);
    assert!(problems.is_empty(), "{problems:#?}");
    assert_eq!(formatted, printed);
}

#[test]
fn edits_carry_lua_files_over() {
    let sources = sources();
    let lowered = read(&sources);
    let mut next = lowered.model.clone();
    next.lua
        .files
        .insert("lua/auth.lua".into(), "return\n".into());
    next.lua.files.remove("lua/auth/keys.lua");
    next.lua
        .files
        .insert("lua/new.lua".into(), "return 1\n".into());
    let edited = reconcile(&sources, &lowered, &next);
    assert_eq!(edited.get("lua/auth.lua"), Some("return\n"));
    assert_eq!(edited.get("lua/auth/keys.lua"), None);
    assert_eq!(edited.get("lua/new.lua"), Some("return 1\n"));
    assert_eq!(edited.get("main.conf"), sources.get("main.conf"));
}

#[test]
fn lua_mistakes_are_reported_where_they_are_written() {
    let mut sources = Sources::single(
        r#"language_version 1;
http {
    lua_package_path "/usr/share/lua/?.lua";
    content_by_lua 'ngx.say(1)';
    lua_code_cache off;
    include lua/auth.lua;
    upstream app {
        server 10.0.0.1:80;
        balancer_by_lua_block { }
    }
    server s {
        server_name s.example;
        access_by_lua_block { ngx.exit(403) }
        access_by_lua_file lua/auth.lua;
        log_by_lua_file lua/missing.lua;
        lua_shared_dict x 1m;
        rewrite_by_lua_file other/a.lua;
        lua_on_error maybe;
        lua_allow everything;
        proxy app;
        route r {
            match prefix /;
            server_rewrite_by_lua_block { }
            lua_log_level loud;
            respond 200;
        }
    }
}
"#,
    );
    sources.insert("lua/auth.lua", AUTH);
    sources.insert("scripts/stray.lua", "return 1\n");
    let found = messages(&read(&sources));
    for (code, at, message) in [
        (
            codes::UNKNOWN_DIRECTIVE,
            "main.conf:3",
            "'lua_package_path' is not available",
        ),
        (
            codes::UNKNOWN_DIRECTIVE,
            "main.conf:4",
            "'content_by_lua' is not available",
        ),
        (
            codes::TYPE,
            "main.conf:5",
            "lua_code_cache off is not available",
        ),
        (
            codes::INCLUDE,
            "main.conf:6",
            "lua/auth.lua is Lua code, not configuration",
        ),
        (
            codes::DUPLICATE,
            "main.conf:14",
            "the server already has access_by_lua",
        ),
        (
            codes::REFERENCE,
            "main.conf:15",
            "lua/missing.lua is not a file of this configuration",
        ),
        (
            codes::CONTEXT,
            "main.conf:16",
            "'lua_shared_dict' is not allowed in server",
        ),
        (
            codes::TYPE,
            "main.conf:17",
            "\"other/a.lua\" is not a Lua file of the configuration",
        ),
        (
            codes::TYPE,
            "main.conf:18",
            "\"maybe\" is not fail, continue or an HTTP status",
        ),
        (
            codes::TYPE,
            "main.conf:19",
            "\"everything\" is not something scripts can be allowed",
        ),
        (
            codes::CONTEXT,
            "main.conf:23",
            "'server_rewrite_by_lua_block' is not allowed in route",
        ),
        (codes::TYPE, "main.conf:24", "\"loud\" is not a log level"),
        (
            "VALIDATION_FAILED",
            "main.conf:7",
            "needs the upstream permission",
        ),
        (
            "VALIDATION_FAILED",
            "scripts/stray.lua",
            "is not a Lua file name",
        ),
    ] {
        assert!(
            has(&found, code, at, message),
            "{code} {at} {message}: {found:#?}"
        );
    }
    let help = read(&sources)
        .diagnostics
        .into_iter()
        .find(|diagnostic| diagnostic.message.contains("'content_by_lua'"))
        .and_then(|diagnostic| diagnostic.help);
    assert_eq!(
        help.as_deref(),
        Some("write the code in braces, as content_by_lua_block { ... }")
    );
}

#[test]
fn lua_off_keeps_the_scripts_without_running_them() {
    let lowered = read(&Sources::single(
        "language_version 1;\nhttp {\n    lua off;\n    access_by_lua_block { ngx.exit(403) }\n}\n",
    ));
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    assert!(lowered.model.lua.disabled);
    assert!(lowered.model.lua.http.access.is_some());
    assert!(
        print(&lowered.model).contains("    lua off;\n    access_by_lua_block { ngx.exit(403) }\n")
    );
}

#[test]
fn a_lua_block_that_never_closes_is_a_syntax_error() {
    let found = messages(&read(&Sources::single(
        "language_version 1;\nhttp {\n    access_by_lua_block { ngx.say(\"open)\n    }\n}\n",
    )));
    assert!(
        has(
            &found,
            panel_dsl::SYNTAX_ERROR,
            "main.conf:3",
            "the Lua string is not closed on its line"
        ),
        "{found:#?}"
    );
}

#[test]
fn explain_shows_the_lua_a_route_takes_over() {
    let sources = sources();
    let lowered = read(&sources);
    let explanation = explain(&sources, &lowered, "main.conf", 34, 13).unwrap();
    let find = |name: &str| {
        explanation
            .settings
            .iter()
            .find(|setting| setting.name == name)
            .unwrap_or_else(|| panic!("{name}: {:#?}", explanation.settings))
    };
    assert_eq!(find("content_by_lua_block").value, "{ 4 lines of Lua }");
    let access = find("access_by_lua_file");
    assert_eq!(
        (access.value.as_str(), access.from.as_deref()),
        ("lua/auth.lua", Some("http"))
    );
    assert_eq!(find("lua_on_error").from.as_deref(), Some("server shop"));
    assert_eq!(find("lua_time_limit").value, "50ms");
    let work = find("lua_work_limit");
    assert_eq!(work.value, "10000000");
    assert_eq!(format!("{:?}", work.source), "Default");
    assert!(explanation
        .settings
        .iter()
        .all(|setting| !setting.name.starts_with("server_rewrite")));
}

#[test]
fn lua_terms_without_a_handler_are_warned_about() {
    let lowered = read(&Sources::single(
        r#"language_version 1;
http {
    lua_time_limit 5ms;
    upstream app {
        server 10.0.0.1:80;
    }
    server quiet {
        server_name quiet.example;
        lua_debug on;
        proxy app;
    }
    server busy {
        server_name busy.example;
        proxy app;
        route plain {
            match prefix /plain;
            lua_allow body;
            proxy app;
        }
        route scripted {
            match prefix /scripted;
            access_by_lua_block { }
            proxy app;
        }
    }
}
"#,
    ));
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let found = messages(&lowered);
    assert!(
        has(
            &found,
            codes::NO_EFFECT,
            "main.conf:9",
            "lua_debug has no effect: no Lua handler runs for server \"quiet\""
        ),
        "{found:#?}"
    );
    assert!(
        has(
            &found,
            codes::NO_EFFECT,
            "main.conf:17",
            "lua_allow has no effect: no Lua handler runs for route \"plain\""
        ),
        "{found:#?}"
    );
    assert!(
        !found
            .iter()
            .any(|(_, _, message)| message.starts_with("lua_time_limit")),
        "{found:#?}"
    );

    let idle = messages(&read(&Sources::single(
        "language_version 1;\nhttp {\n    lua_time_limit 5ms;\n}\n",
    )));
    assert!(
        has(
            &idle,
            codes::NO_EFFECT,
            "main.conf:3",
            "no Lua handler runs for any request"
        ),
        "{idle:#?}"
    );
}

#[test]
fn scripts_are_compiled_and_checked_where_they_are_written() {
    let mut sources = Sources::single(
        r#"language_version 1;
http {
    upstream app {
        server 10.0.0.1:80;
    }
    server s {
        server_name s.example;
        header_filter_by_lua_block {
            ngx.say("too late")
        }
        access_by_lua_file lua/gate.lua;
        proxy app;
        route broken {
            match prefix /broken;
            content_by_lua_block {
                if then
            }
        }
    }
}
"#,
    );
    sources.insert(
        "lua/gate.lua",
        "local redis = require \"resty.redis\"\nlocal res = ngx.location.capture(\"/auth\")\nseen = true\n",
    );
    sources.insert("lua/lib/util.lua", "counter = 0\nreturn {}\n");
    sources.insert("lua/bad.lua", "return function(\n");
    let found = messages(&read(&sources));
    for (code, at, message) in [
        (codes::LUA, "main.conf:16", "the Lua code does not compile"),
        (
            codes::LUA,
            "main.conf:9",
            "ngx.say is disabled in header_filter_by_lua*",
        ),
        (
            codes::LUA,
            "lua/gate.lua:1",
            "module \"resty.redis\" is neither built in nor a file under lua/",
        ),
        (
            codes::LUA,
            "lua/gate.lua:2",
            "ngx.location.capture is not available in Pingora Panel",
        ),
        (
            codes::LUA,
            "lua/gate.lua:3",
            "writes the global seen, which stays with this request",
        ),
        (
            codes::LUA,
            "lua/lib/util.lua:1",
            "the module writes the global counter",
        ),
        (codes::LUA, "lua/bad.lua:2", "the Lua code does not compile"),
        (
            codes::NO_EFFECT,
            "lua/lib/util.lua",
            "lua/lib/util.lua is not used",
        ),
    ] {
        assert!(
            has(&found, code, at, message),
            "{code} {at} {message}: {found:#?}"
        );
    }
    let lowered = read(&sources);
    let errors: Vec<_> = lowered
        .errors()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert!(!messages(&read(&self::sources()))
        .iter()
        .any(|(code, _, _)| code == codes::LUA || code == codes::NO_EFFECT));
}
