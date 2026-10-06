#![forbid(unsafe_code)]

use chrono::Utc;
use panel_config_dsl::{
    codes, explain, format_files, lower, print, print_sources, reconcile, LowerOptions, Lowered,
    Sources,
};
use panel_config_model::{compile, Action, LuaCode, LuaFallback, LuaLogLevel, LuaVariable};
use panel_domain::RevisionId;
use std::collections::BTreeMap;

const MAIN: &str = r#"language_version 1;

http {
    lua_shared_dict hits 1m;
    lua_memory_limit 32m; access_by_lua_no_postpone on; lua_max_pending_timers 64; lua_max_running_timers 8; lua_regex_cache_max_entries 0; lua_regex_match_limit 100000; lua_worker_thread_vm_pool_size 4; lua_capture_error_log 32k;
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
        lua_on_error continue; lua_socket_read_timeout 5s; lua_socket_pool_size 10; lua_socket_log_errors off; lua_transform_underscores_in_response_headers off; lua_use_default_type off; lua_need_request_body on; lua_ssl_trusted_certificate shop-ca; lua_ssl_verify_depth 2; lua_ssl_protocols TLSv1.3; lua_ssl_ciphers ECDHE-RSA-AES128-GCM-SHA256:HIGH:!MD5; lua_ssl_certificate shop-client; lua_ssl_certificate_key shop-client-key; lua_ssl_crl shop-crl;
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
            lua.max_pending_timers,
            lua.max_running_timers,
            lua.regex_cache_max_entries,
            lua.regex_match_limit
        ),
        (Some(64), Some(8), Some(0), Some(100_000))
    );
    assert_eq!(lua.access_no_postpone, Some(true));
    assert_eq!(lua.worker_thread_vm_pool_size, Some(4));
    assert_eq!(lua.capture_error_log_bytes, Some(32 << 10));
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
    assert_eq!(
        (
            site.lua.socket_read_timeout_ms,
            site.lua.socket_pool_size,
            site.lua.socket_log_errors,
            site.lua.transform_underscores,
            site.lua.use_default_type
        ),
        (Some(5_000), Some(10), Some(false), Some(false), Some(false))
    );
    assert_eq!(site.lua.need_request_body, Some(true));
    assert_eq!(
        (
            site.lua.ssl_trusted_certificate.as_deref(),
            site.lua.ssl_crl.as_deref(),
            site.lua.ssl_certificate.as_deref(),
            site.lua.ssl_certificate_key.as_deref(),
            site.lua.ssl_verify_depth
        ),
        (
            Some("shop-ca"),
            Some("shop-crl"),
            Some("shop-client"),
            Some("shop-client-key"),
            Some(2)
        )
    );
    assert_eq!(
        site.lua.ssl_protocols.as_deref(),
        Some(&["TLSv1.3".to_owned()][..])
    );
    assert_eq!(
        site.lua.ssl_ciphers.as_deref(),
        Some(&["TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256".to_owned()][..])
    );
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
    assert!(snapshot.lua.access_first);
    let filter = snapshot.sites[0].lua.header_filter.as_ref().unwrap();
    assert_eq!(
        (
            filter.sockets.read_timeout_ms,
            filter.sockets.pool_size,
            filter.sockets.quiet,
            filter.keep_underscores,
            filter.no_default_type
        ),
        (5_000, 10, true, true, true)
    );
    assert!(filter.read_body_first);
    let tls = &filter.sockets.tls;
    assert_eq!(
        (
            tls.trusted_certificate_secret_id.as_deref(),
            tls.verify_depth,
            tls.protocols.as_slice()
        ),
        (Some("shop-ca"), Some(2), &["TLSv1.3".to_owned()][..])
    );
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
        "    lua_memory_limit 32m;\n    lua_capture_error_log 32k;\n    access_by_lua_no_postpone on;\n    lua_max_pending_timers 64;\n    lua_max_running_timers 8;\n    lua_regex_cache_max_entries 0;\n    lua_regex_match_limit 100000;\n    lua_worker_thread_vm_pool_size 4;\n    lua_shared_dict hits 1m;\n    lua_time_limit 50ms;\n    lua_allow upstream;\n    init_by_lua_block {\n        local limits",
        "    access_by_lua_file lua/auth.lua;\n    log_by_lua_block { local n",
        "        balancer_by_lua_file lua/pick.lua;\n",
        "        lua_on_error continue;\n        lua_log_level warn;\n        lua_socket_read_timeout 5s;\n        lua_socket_pool_size 10;\n        lua_socket_log_errors off;\n        lua_transform_underscores_in_response_headers off;\n        lua_use_default_type off;\n        lua_need_request_body on;\n        lua_ssl_trusted_certificate shop-ca;\n        lua_ssl_crl shop-crl;\n        lua_ssl_certificate shop-client;\n        lua_ssl_certificate_key shop-client-key;\n        lua_ssl_verify_depth 2;\n        lua_ssl_protocols TLSv1.3;\n        lua_ssl_ciphers ECDHE-RSA-AES128-GCM-SHA256;\n        header_filter_by_lua_block {\n",
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
            codes::CONTEXT,
            "main.conf:4",
            "'content_by_lua_block' is not allowed in http",
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
        Some("write it as `content_by_lua_block { ... }`, as lua-nginx-module recommends and the configuration is printed")
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
        "local redis = require \"resty.redis\"\nlocal res = ngx.req.set_body_file(\"/auth\")\nseen = true\n",
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
            "ngx.req.set_body_file is not available in Pingora Panel",
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

#[test]
fn plans_name_the_lua_they_change() {
    let lowered = read(&sources());
    let mut next = lowered.model.clone();
    next.lua
        .files
        .insert("lua/auth.lua".into(), "return\n".into());
    next.lua.http.time_limit_ms = Some(20);
    next.sites[0].lua.debug = Some(true);
    let changes = panel_config_dsl::plan::plan(&lowered.model, &next);
    let resources: Vec<_> = changes
        .iter()
        .map(|change| change.resource.as_str())
        .collect();
    let site = format!("sites/{}", next.sites[0].id);
    assert_eq!(resources, ["lua", "lua/auth.lua", site.as_str()]);
    assert!(
        changes[0].diff.contains("+lua_time_limit 20ms;"),
        "{}",
        changes[0].diff
    );
    assert!(changes[1].diff.contains("+return"), "{}", changes[1].diff);
}

#[test]
fn the_library_lists_each_script_where_it_runs() {
    let lowered = read(&sources());
    let library = panel_config_dsl::lua_library(&lowered);
    let ids: Vec<_> = library
        .scripts
        .iter()
        .map(|script| script.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "lua/auth.lua",
            "lua/auth/keys.lua",
            "lua/pick.lua",
            "main.conf:13",
            "main.conf:28",
            "main.conf:37",
            "main.conf:8",
        ]
    );
    let auth = &library.scripts[0];
    assert_eq!(auth.module.as_deref(), Some("auth"));
    assert_eq!(auth.requires, ["auth.keys"]);
    assert_eq!(
        (auth.uses[0].label.as_str(), auth.uses[0].phase.as_str()),
        ("http", "access")
    );
    assert_eq!(auth.sha256.len(), 64);
    assert!(library.scripts[1].uses.is_empty());
    assert_eq!(library.scripts[2].uses[0].label, "upstream app");
    let content = &library.scripts[5];
    assert_eq!(content.uses[0].label, "server shop, route hello");
    assert_eq!(content.uses[0].phase, "content");
    assert_eq!(library.shared_dicts.len(), 1);
    assert!(library.diagnostics.is_empty(), "{:#?}", library.diagnostics);
}

#[test]
fn openresty_directives_with_nothing_to_tune_read_with_a_warning() {
    let lowered = read(&Sources::single(
        r#"language_version 1;
http {
    lua_load_resty_core on;
    lua_malloc_trim 1000;
    upstream app {
        server 10.0.0.1:80;
        balancer_keepalive 32;
    }
    server s {
        server_name s.example;
        lua_http10_buffering off;
        proxy app;
        route r {
            match prefix /;
            lua_upstream_skip_openssl_default_verify on;
            respond 200;
        }
    }
}
"#,
    ));
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let warned: Vec<&str> = lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.as_str() == codes::NO_EFFECT)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    for name in [
        "lua_load_resty_core",
        "lua_malloc_trim",
        "balancer_keepalive",
        "lua_http10_buffering",
        "lua_upstream_skip_openssl_default_verify",
    ] {
        assert!(
            warned
                .iter()
                .any(|message| message.starts_with(&format!("'{name}' has no effect: "))),
            "{name}: {warned:?}"
        );
    }
}

#[test]
fn precontent_handlers_read_compile_and_print() {
    let sources = Sources::single(
        r#"language_version 1;
http {
    server s {
        server_name s.example;
        respond 404;
        route r {
            match prefix /;
            precontent_by_lua_block {
                ngx.req.set_header("X-Pre", "1")
            }
            respond 200;
        }
    }
}
"#,
    );
    let lowered = read(&sources);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let route = &lowered.model.sites[0].routes[0];
    assert!(route.lua.precontent.is_some());
    let snapshot = compile(&lowered.model, RevisionId::new(1)).unwrap();
    assert!(snapshot.routes[0].lua.precontent.is_some());
    let printed = print_sources(&lowered.model);
    assert!(printed
        .get("main.conf")
        .unwrap()
        .contains("            precontent_by_lua_block {\n"));
}

#[test]
fn exit_worker_handlers_read_compile_and_print() {
    let sources = Sources::single(
        r#"language_version 1;
http {
    exit_worker_by_lua_block {
        ngx.log(ngx.NOTICE, "bye")
    }
    server s {
        server_name s.example;
        respond 200;
    }
}
"#,
    );
    let lowered = read(&sources);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    assert!(lowered.model.lua.exit_worker.is_some());
    let snapshot = compile(&lowered.model, RevisionId::new(1)).unwrap();
    assert!(snapshot.lua.exit_worker.is_some());
    let printed = print_sources(&lowered.model);
    assert!(printed
        .get("main.conf")
        .unwrap()
        .contains("    exit_worker_by_lua_block {\n"));
}

#[test]
fn handshake_handlers_read_compile_print_and_stay_off_routes() {
    let sources = Sources::single(
        r#"language_version 1;
http {
    ssl_client_hello_by_lua_block {
        local name = require("ngx.ssl.clienthello").get_client_hello_server_name()
    }
    server s {
        server_name s.example;
        ssl_certificate_by_lua_block {
            local ssl = require "ngx.ssl"
        }
        respond 404;
        route r {
            match prefix /;
            respond 200;
        }
    }
}
"#,
    );
    let lowered = read(&sources);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    assert!(lowered.model.lua.http.ssl_client_hello.is_some());
    assert!(lowered.model.sites[0].lua.ssl_cert.is_some());
    let snapshot = compile(&lowered.model, RevisionId::new(1)).unwrap();
    assert!(snapshot.sites[0].lua.ssl_client_hello.is_some());
    assert!(snapshot.sites[0].lua.ssl_cert.is_some());
    assert!(snapshot.routes[0].lua.ssl_cert.is_none());
    let printed = print_sources(&lowered.model);
    let main = printed.get("main.conf").unwrap();
    assert!(
        main.contains("    ssl_client_hello_by_lua_block {\n"),
        "{main}"
    );
    assert!(
        main.contains("        ssl_certificate_by_lua_block {\n"),
        "{main}"
    );

    let misplaced = read(&Sources::single(
        r#"language_version 1;
http {
    server s {
        server_name s.example;
        ssl_session_store_by_lua_block { }
        route r {
            match prefix /;
            ssl_certificate_by_lua_block { }
            respond 200;
        }
    }
}
"#,
    ));
    let found = messages(&misplaced);
    assert!(
        has(
            &found,
            codes::CONTEXT,
            "main.conf:8",
            "'ssl_certificate_by_lua_block' is not allowed in route"
        ),
        "{found:#?}"
    );
    assert!(
        has(
            &found,
            codes::CONTEXT,
            "main.conf:5",
            "'ssl_session_store_by_lua_block' is not allowed in server"
        ),
        "{found:#?}"
    );
}

#[test]
fn session_handlers_read_compile_and_print_in_http() {
    let sources = Sources::single(
        r#"language_version 1;
http {
    lua_shared_dict sessions 1m;
    ssl_session_fetch_by_lua_block {
        local session = require "ngx.ssl.session"
    }
    ssl_session_store_by_lua_block {
        local id = require("ngx.ssl.session").get_session_id()
    }
    server s {
        server_name s.example;
        respond 200;
    }
}
"#,
    );
    let lowered = read(&sources);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    assert!(lowered.model.lua.ssl_session_fetch.is_some());
    assert!(lowered.model.lua.ssl_session_store.is_some());
    let snapshot = compile(&lowered.model, RevisionId::new(1)).unwrap();
    assert!(snapshot.lua.ssl_session_fetch.is_some());
    assert!(snapshot.lua.ssl_session_store.is_some());
    let printed = print_sources(&lowered.model);
    let main = printed.get("main.conf").unwrap();
    assert!(
        main.contains("    ssl_session_fetch_by_lua_block {\n"),
        "{main}"
    );
    assert!(
        main.contains("    ssl_session_store_by_lua_block {\n"),
        "{main}"
    );
}

#[test]
fn set_and_set_by_lua_give_variables_templates_read_per_request() {
    let mut sources = Sources::single(
        r#"language_version 1;
http {
    set $base b;
    server s {
        server_name s.example;
        set $site s1;
        set_by_lua_block $tenant $http_x_key {
            return ngx.arg[1] .. ngx.var.base
        }
        respond 404;
        route r {
            match prefix /;
            set_by_lua_file $hash lua/hash.lua $host ${lua:tenant};
            respond 200 "body=$tenant $hash $site $$";
        }
    }
}
"#,
    );
    sources.insert("lua/hash.lua", "return ngx.md5(ngx.arg[1])\n");
    let lowered = read(&sources);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let model = &lowered.model;
    assert_eq!(model.lua.http.variables, [LuaVariable::value("base", "b")]);
    let site = &model.sites[0];
    let names: Vec<_> = site.lua.variables.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["site", "tenant"]);
    assert_eq!(site.lua.variables[1].args, ["$http_x_key"]);
    let route = &site.routes[0];
    assert_eq!(
        route.lua.variables,
        [LuaVariable::script(
            "hash",
            LuaCode::file("lua/hash.lua"),
            vec!["$host".into(), "${lua:tenant}".into()],
        )]
    );
    let Action::Respond { body, .. } = &route.action else {
        panic!("{:?}", route.action);
    };
    assert_eq!(
        body.as_deref(),
        Some("${lua:tenant} ${lua:hash} ${lua:site} $$")
    );

    let snapshot = compile(model, RevisionId::new(1)).unwrap();
    let site_variables: Vec<_> = snapshot.sites[0]
        .lua
        .variables
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(site_variables, ["base", "site", "tenant"]);
    let route = snapshot
        .routes
        .iter()
        .find(|route| !route.lua.variables.is_empty())
        .unwrap();
    assert_eq!(route.lua.variables[0].name, "hash");
    assert!(route.lua.variables[0].handler.is_some());

    let printed = print_sources(model);
    let main = printed.get("main.conf").unwrap();
    for written in [
        "    set $base b;\n",
        "        set $site s1;\n",
        "        set_by_lua_block $tenant $http_x_key {\n",
        "            set_by_lua_file $hash lua/hash.lua $host ${lua:tenant};\n",
    ] {
        assert!(main.contains(written), "{written}: {main}");
    }
    let again = read(&printed);
    assert!(again.is_valid(), "{:#?}", again.diagnostics);
    assert_eq!(again.model.lua.http.variables, model.lua.http.variables);
    assert_eq!(
        again.model.sites[0].routes[0].action,
        model.sites[0].routes[0].action
    );
    assert_eq!(
        again.model.sites[0].routes[0].lua.variables,
        model.sites[0].routes[0].lua.variables
    );
}

#[test]
fn variables_scripts_set_are_known_only_per_request() {
    let sources = Sources::single(
        r#"language_version 1;
http {
    set_by_lua_block $h { return 1 }
    server s {
        server_name s.example;
        set_by_lua_block $uri { return 1 }
        set_by_lua_block $t { return 1 }
        set_by_lua 'return 1';
        set_by_lua_file $f lua/missing.lua;
        respond 200 body=${lua:anything};
        route r {
            match prefix /$t;
            respond 200;
        }
    }
}
"#,
    );
    let found = messages(&read(&sources));
    for (code, message) in [
        (codes::CONTEXT, "'set_by_lua_block'"),
        (
            codes::VARIABLE,
            "$uri is a request variable and cannot be set",
        ),
        (codes::UNKNOWN_DIRECTIVE, "'set_by_lua' is not available"),
        (
            codes::REFERENCE,
            "lua/missing.lua is not a file of this configuration",
        ),
        (
            codes::VARIABLE,
            "$t is set per request by set_by_lua and cannot be used here",
        ),
    ] {
        assert!(
            found
                .iter()
                .any(|(found, _, text)| found == code && text.contains(message)),
            "{message}: {found:#?}"
        );
    }
    assert!(
        !found
            .iter()
            .any(|(_, _, text)| text.contains("lua:anything")),
        "{found:#?}"
    );
}

#[test]
fn lua_ssl_terms_refuse_what_cosockets_cannot_do() {
    let sources = Sources::single(
        r#"language_version 1;
http {
    lua_ssl_protocols TLSv1 TLSv1.2;
    server s {
        server_name s.example;
        lua_ssl_protocols SSLv3;
        lua_ssl_ciphers !ECDHE-ECDSA-AES256-GCM-SHA384:!ECDHE-ECDSA-AES128-GCM-SHA256:!ECDHE-ECDSA-CHACHA20-POLY1305:!ECDHE-RSA-AES256-GCM-SHA384:!ECDHE-RSA-AES128-GCM-SHA256:!ECDHE-RSA-CHACHA20-POLY1305;
        lua_ssl_key_log /tmp/keys.log;
        lua_ssl_conf_command Options -SessionTicket;
        content_by_lua_block { ngx.say("hi") }
    }
}
"#,
    );
    let found = messages(&read(&sources));
    for (code, message) in [
        (codes::NO_EFFECT, "TLSv1 is not offered: RFC 8996 retires"),
        (
            codes::TYPE,
            "lua_ssl_protocols offers no version cosockets speak",
        ),
        (codes::TYPE, "leaves out every TLS 1.2 suite"),
        (
            codes::UNKNOWN_DIRECTIVE,
            "'lua_ssl_key_log' is not available",
        ),
        (
            codes::UNKNOWN_DIRECTIVE,
            "'lua_ssl_conf_command' is not available",
        ),
    ] {
        assert!(
            found
                .iter()
                .any(|(found, _, text)| found == code && text.contains(message)),
            "{message}: {found:#?}"
        );
    }
    let mut model = read(&Sources::single(
        r#"language_version 1;
http {
    server s {
        server_name s.example;
        lua_ssl_trusted_certificate system;
        lua_ssl_crl revoked;
        lua_ssl_verify_depth 101;
        content_by_lua_block { ngx.say("hi") }
    }
}
"#,
    ))
    .model;
    let problems: Vec<String> = panel_config_model::validate(&model)
        .into_iter()
        .map(|problem| problem.message)
        .collect();
    for expected in [
        "lua_ssl_crl checks the authorities of lua_ssl_trusted_certificate, not the system's",
        "lua_ssl_verify_depth is over 100",
    ] {
        assert!(
            problems.iter().any(|problem| problem.contains(expected)),
            "{expected}: {problems:#?}"
        );
    }
    model.sites[0].lua.ssl_crl = None;
    model.sites[0].lua.ssl_verify_depth = Some(3);
    let snapshot = compile(&model, RevisionId::new(1)).unwrap();
    let content = snapshot
        .routes
        .iter()
        .find_map(|route| match &route.action {
            panel_ir::RouteAction::Lua { handler } => Some(handler),
            _ => None,
        })
        .unwrap();
    assert_eq!(content.sockets.tls.trusted_certificate_secret_id, None);
    assert_eq!(content.sockets.tls.verify_depth, Some(3));
}

#[test]
fn handlers_watch_for_clients_that_leave_with_lua_check_client_abort() {
    let lowered = read(&Sources::single(
        r#"language_version 1;
http {
    lua_check_client_abort on;
    server s {
        server_name s.example;
        content_by_lua_block {
            ngx.on_abort(function() ngx.exit(499) end)
            ngx.sleep(1)
        }
    }
}
"#,
    ));
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    assert_eq!(lowered.model.lua.http.check_client_abort, Some(true));
    let snapshot = compile(&lowered.model, RevisionId::new(1)).unwrap();
    let content = snapshot
        .routes
        .iter()
        .find_map(|route| match &route.action {
            panel_ir::RouteAction::Lua { handler } => Some(handler),
            _ => None,
        })
        .unwrap();
    assert!(content.check_client_abort);
    let printed = print_sources(&lowered.model);
    assert!(printed
        .get("main.conf")
        .unwrap()
        .contains("    lua_check_client_abort on;\n"));
}

#[test]
fn named_locations_read_compile_and_print() {
    let lowered = read(&Sources::single(
        r#"language_version 1;
http {
    server s {
        server_name s.example;
        content_by_lua_block { ngx.exec("@fallback") }
        location @fallback {
            content_by_lua_block { ngx.say("fallback") }
        }
    }
}
"#,
    ));
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let route = &lowered.model.sites[0].routes[0];
    assert_eq!(route.named.as_deref(), Some("fallback"));
    let snapshot = compile(&lowered.model, RevisionId::new(1)).unwrap();
    assert!(snapshot.routes.iter().any(|route| matches!(
        &route.matcher,
        panel_ir::RouteMatcher::Named { name } if name == "fallback"
    )));
    assert!(snapshot
        .required_capabilities
        .iter()
        .any(|capability| capability.name == "route.named"));
    let printed = print_sources(&lowered.model);
    let main = printed.get("main.conf").unwrap();
    assert!(
        main.contains("            match named fallback;\n"),
        "{main}"
    );
    let again = read(&printed);
    assert!(again.is_valid(), "{:#?}", again.diagnostics);
    assert_eq!(again.model.sites[0].routes[0].named, route.named);
}

#[test]
fn directives_that_take_code_as_a_string_read_as_blocks() {
    let sources = Sources::single(
        r#"language_version 1;
http {
    server s {
        server_name s.example;
        set_by_lua $tenant 'return ngx.arg[1] .. "!"' $host;
        access_by_lua 'if ngx.var.arg_deny then return ngx.exit(403) end';
        respond 200 "body=$tenant";
    }
}
"#,
    );
    let lowered = read(&sources);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let found = messages(&lowered);
    assert!(
        has(
            &found,
            codes::DEPRECATED,
            "main.conf:5",
            "'set_by_lua' takes its code as a string"
        ),
        "{found:#?}"
    );
    assert!(
        has(
            &found,
            codes::DEPRECATED,
            "main.conf:6",
            "it is read as 'access_by_lua_block'"
        ),
        "{found:#?}"
    );
    let site = &lowered.model.sites[0];
    let Some(LuaCode::Inline { code, .. }) = &site.lua.access else {
        panic!("{:?}", site.lua.access);
    };
    assert_eq!(code, "if ngx.var.arg_deny then return ngx.exit(403) end");
    let tenant = &site.lua.variables[0];
    assert_eq!(tenant.name, "tenant");
    assert_eq!(tenant.args, ["$host"]);
    assert!(
        matches!(&tenant.code, Some(LuaCode::Inline { code, .. }) if code == r#"return ngx.arg[1] .. "!""#)
    );
    let printed = print_sources(&lowered.model);
    let main = printed.get("main.conf").unwrap();
    assert!(main.contains("access_by_lua_block {"), "{main}");
    assert!(main.contains("set_by_lua_block $tenant $host {"), "{main}");

    let refused = read(&Sources::single(
        "language_version 1;\nhttp {\n    server_rewrite_by_lua 'x';\n}\n",
    ));
    assert!(!refused.is_valid());
}

#[test]
fn proxy_certificate_handlers_read_compile_and_print_where_routes_proxy() {
    let sources = Sources::single(
        r#"language_version 1;
http {
    upstream secure {
        server 10.0.0.1:443 tls;
    }
    server s {
        server_name s.example;
        respond 404;
        route r {
            match prefix /;
            proxy_ssl_certificate_by_lua_block {
                local proxy = require "ngx.ssl.proxysslcert"
            }
            proxy secure;
        }
    }
}
"#,
    );
    let lowered = read(&sources);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    assert!(lowered.model.sites[0].routes[0]
        .lua
        .proxy_ssl_cert
        .is_some());
    let snapshot = compile(&lowered.model, RevisionId::new(1)).unwrap();
    assert!(snapshot.routes[0].lua.proxy_ssl_cert.is_some());
    let printed = print_sources(&lowered.model);
    let main = printed.get("main.conf").unwrap();
    assert!(
        main.contains("            proxy_ssl_certificate_by_lua_block {\n"),
        "{main}"
    );

    let verify = read(&Sources::single(
        "language_version 1;\nhttp {\n    proxy_ssl_verify_by_lua_block { }\n}\n",
    ));
    assert!(verify.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("proxy_ssl_verify_by_lua_block")
            && diagnostic
                .help
                .as_deref()
                .is_some_and(|help| help.contains("upstream TLS terms"))
    }));
}
