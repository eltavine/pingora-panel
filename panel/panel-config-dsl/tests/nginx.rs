#![forbid(unsafe_code)]

use chrono::Utc;
use panel_config_dsl::{import_nginx, lower, nginx::codes, LowerOptions};
use panel_config_model::{Action, MatchKind};
use std::collections::BTreeMap;

const NGINX: &str = "\
user nginx;
worker_processes auto;
events {
    worker_connections 1024;
}

http {
    include mime.types;
    sendfile on;

    upstream app {
        least_conn;
        server 10.0.0.11:8080 weight=3 max_fails=3 fail_timeout=30s;
        server 10.0.0.12 backup;
        keepalive 32;
    }

    include conf.d/*.conf;
}
";

const SHOP: &str = r#"server {
    listen 80 default_server;
    listen [::]:80;
    server_name shop.example www.shop.example;
    root /var/www/shop;
    index index.html;

    location = /healthz { return 204; }
    location ^~ /static/ { root /var/www/assets; }
    location ~* \.(png|jpg)$ { expires 30d; root /var/www/images; }
    location /api/ { proxy_pass http://app; proxy_set_header Host $host; }
    location /v1/ { proxy_pass http://app/v2/; }
    location / { try_files $uri $uri/ /index.html; }
}

server {
    listen 443 ssl;
    server_name shop.example;
    ssl_certificate /etc/ssl/shop.pem;
}

server {
    listen 8080;
    server_name legacy.example;
    location / { proxy_pass http://127.0.0.1:9000; }
    location /old { return 301 https://shop.example/new; }
    location /maintenance { return 503 "back soon"; }
    return 301 https://$host$request_uri;
}
"#;

fn files() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("/etc/nginx/nginx.conf".to_owned(), NGINX.to_owned()),
        ("/etc/nginx/conf.d/shop.conf".to_owned(), SHOP.to_owned()),
    ])
}

#[test]
fn the_documented_subset_converts_and_everything_else_is_reported() {
    let imported = import_nginx(&files(), "/etc/nginx/nginx.conf").unwrap();
    let text = imported.sources.get("main.conf").unwrap();
    let environment = BTreeMap::new();
    let lowered = lower(
        &imported.sources,
        &LowerOptions {
            environment: &environment,
            previous: None,
            now: Utc::now(),
        },
    );
    assert!(
        lowered
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != panel_errors::DiagnosticSeverity::Error),
        "{:#?}\n{text}",
        lowered.diagnostics
    );
    let model = lowered.model;

    let listeners: Vec<_> = model
        .listeners
        .iter()
        .map(|listener| (listener.id.as_str(), listener.address.as_str()))
        .collect();
    assert_eq!(
        listeners,
        [
            ("http-80", "0.0.0.0:80"),
            ("http-8080", "0.0.0.0:8080"),
            ("http-v6-80", "[::]:80")
        ],
        "{text}"
    );
    assert_eq!(model.listeners[2].ipv6_only, Some(true));
    assert_eq!(model.upstreams.len(), 2, "{text}");
    let app = &model.upstreams[0];
    assert_eq!(app.name, "app");
    assert_eq!(app.nodes[0].weight, 3);
    assert_eq!(app.nodes[1].port, 80);
    assert!(app.nodes[1].backup);
    assert_eq!(
        app.passive_health
            .as_ref()
            .map(|policy| policy.failure_threshold),
        Some(3)
    );

    assert_eq!(model.sites.len(), 2, "{text}");
    let shop = &model.sites[0];
    assert_eq!(shop.name, "shop-example");
    assert_eq!(shop.domains.len(), 2);
    assert!(
        matches!(&shop.action, Action::Static { root, spa_fallback: true, .. } if root == "shop")
    );
    let routes: Vec<_> = shop
        .routes
        .iter()
        .map(|route| (route.matcher.kind, route.matcher.path.as_str()))
        .collect();
    assert_eq!(
        routes,
        [
            (MatchKind::Exact, "/healthz"),
            (MatchKind::Prefix, "/static/"),
            (MatchKind::Regex, r"(?i)\.(png|jpg)$"),
            (MatchKind::Prefix, "/api/"),
        ],
        "{text}"
    );
    let legacy = &model.sites[1];
    assert!(legacy.https_redirect);
    assert!(matches!(&legacy.action, Action::Proxy { .. }));
    assert_eq!(legacy.routes.len(), 2);

    let report: Vec<_> = imported
        .report
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code.as_str(),
                diagnostic.source_span.clone().unwrap_or_default(),
                diagnostic.message.as_str(),
            )
        })
        .collect();
    for (code, span, message) in [
        (
            codes::UNSUPPORTED,
            "/etc/nginx/nginx.conf:1.1-11",
            "'user' is not carried over: the gateway manages its own processes",
        ),
        (
            codes::UNSUPPORTED,
            "/etc/nginx/nginx.conf:8.5-23",
            "the included file \"mime.types\" was not provided",
        ),
        (
            codes::UNSUPPORTED,
            "/etc/nginx/nginx.conf:9.5-16",
            "'sendfile' is not supported",
        ),
        (
            codes::UNSUPPORTED,
            "/etc/nginx/nginx.conf:12.9-19",
            "'least_conn' is not supported in an upstream",
        ),
        (
            codes::CHANGED,
            "/etc/nginx/nginx.conf:15.9-21",
            "'keepalive' turns connection reuse on; its pool size is not carried over",
        ),
        (
            codes::UNSUPPORTED,
            "/etc/nginx/conf.d/shop.conf:10.32-43",
            "'expires' is not supported in a location",
        ),
        (
            codes::UNSUPPORTED,
            "/etc/nginx/conf.d/shop.conf:12.21-46",
            "\"http://app/v2/\" rewrites the request path or uses variables, which is not supported",
        ),
        (
            codes::UNSUPPORTED,
            "/etc/nginx/conf.d/shop.conf:16.1-20.1",
            "the server \"shop-example-2\" only listens with TLS and is not carried over",
        ),
    ] {
        assert!(
            report
                .iter()
                .any(|(c, s, m)| *c == code && s == span && *m == message),
            "missing {code} {span} {message}\n{report:#?}"
        );
    }
}

#[test]
fn absolute_includes_find_a_copied_configuration() {
    let files = BTreeMap::from([
        (
            "nginx/nginx.conf".to_owned(),
            "http {\n    include /etc/nginx/conf.d/*.conf;\n}\n".to_owned(),
        ),
        (
            "nginx/conf.d/site.conf".to_owned(),
            "server {\n    listen 8081;\n    server_name copy.example;\n    return 204;\n}\n"
                .to_owned(),
        ),
    ]);
    let imported = import_nginx(&files, "nginx/nginx.conf").unwrap();
    assert!(imported.report.is_empty(), "{:#?}", imported.report);
    assert!(imported
        .sources
        .get("main.conf")
        .unwrap()
        .contains("server_name copy.example;"));
}

#[test]
fn a_missing_entry_is_refused() {
    assert!(import_nginx(&files(), "nginx.conf").is_err());
}

#[test]
fn turning_access_logs_off_carries_over_and_log_files_are_reported() {
    let files = BTreeMap::from([(
        "/etc/nginx/nginx.conf".to_owned(),
        "http {\n    server {\n        listen 80;\n        server_name shop.example;\n        access_log /var/log/nginx/shop.log combined;\n        location /health {\n            access_log off;\n            return 204;\n        }\n        location / {\n            proxy_pass http://127.0.0.1:9000;\n        }\n    }\n}\n"
            .to_owned(),
    )]);
    let imported = import_nginx(&files, "/etc/nginx/nginx.conf").unwrap();
    let text = imported.sources.get("main.conf").unwrap();
    assert!(text.contains("access_log off;"), "{text}");
    assert!(
        imported
            .report
            .iter()
            .any(|note| note.code.as_str() == codes::UNSUPPORTED
                && note
                    .message
                    .contains("access log files and formats are not carried over")),
        "{:#?}",
        imported.report
    );
    let environment = BTreeMap::new();
    let lowered = lower(
        &imported.sources,
        &LowerOptions {
            environment: &environment,
            previous: None,
            now: Utc::now(),
        },
    );
    let site = &lowered.model.sites[0];
    assert!(site.access_log.is_unset(), "{text}");
    assert_eq!(site.routes[0].access_log.enabled, Some(false), "{text}");
}

#[test]
fn openresty_handlers_carry_over_with_their_files() {
    let nginx = r#"
http {
    lua_package_path "/usr/local/openresty/lualib/?.lua;;";
    lua_shared_dict limits 10m;
    lua_code_cache on;
    lua_ssl_trusted_certificate /etc/ssl/certs/ca-certificates.crt;
    lua_ssl_verify_depth 5;
    lua_ssl_certificate /etc/nginx/client.pem;
    init_by_lua_block {
        local cjson = require "cjson"
    }

    upstream app {
        server 10.0.0.11:8080;
        balancer_by_lua_file /usr/local/openresty/nginx/lua/pick.lua;
    }

    server {
        listen 80;
        server_name api.example;
        access_by_lua_file lua/auth.lua;

        location /hello {
            set_by_lua_block $x { return 1 }
            set_by_lua $y 'return ngx.arg[1] .. ngx.arg[2]' $host $x;
            set_by_lua_file $z /usr/local/openresty/nginx/lua/pick.lua $host;
            header_filter_by_lua 'ngx.header["X-Hello"] = "1"';
            content_by_lua_block {
                ngx.say("hello, ", ngx.var.arg_name or "world") -- }
            }
        }

        location / {
            proxy_pass http://app;
        }

        location @fallback {
            content_by_lua_block { ngx.say("fallback") }
        }
    }
}
"#;
    let files: BTreeMap<String, String> = [
        ("nginx.conf", nginx),
        (
            "lua/auth.lua",
            "if not ngx.var.http_x_key then return ngx.exit(401) end\n",
        ),
        (
            "lua/pick.lua",
            "require(\"ngx.balancer\").set_current_peer(\"10.0.0.12\", 8080)\n",
        ),
    ]
    .into_iter()
    .map(|(path, text)| (path.to_owned(), text.to_owned()))
    .collect();
    let imported = import_nginx(&files, "nginx.conf").unwrap();
    let main = imported.sources.get("main.conf").unwrap();
    for expected in [
        "    lua_shared_dict limits 10m;\n    lua_ssl_trusted_certificate system;\n    lua_ssl_verify_depth 5;\n    init_by_lua_block {\n        local cjson = require \"cjson\"\n    }\n",
        "        balancer_by_lua_file lua/pick.lua;\n",
        "        access_by_lua_file lua/auth.lua;\n",
        "            set_by_lua_block $x { return 1 }\n",
        "            match named fallback;\n",
        "            set_by_lua_block $y $host $x { return ngx.arg[1] .. ngx.arg[2] }\n",
        "            set_by_lua_file $z lua/pick.lua $host;\n",
        "            header_filter_by_lua_block { ngx.header[\"X-Hello\"] = \"1\" }\n",
        "            content_by_lua_block {\n                ngx.say(\"hello, \", ngx.var.arg_name or \"world\") -- }\n            }\n",
    ] {
        assert!(main.contains(expected), "{expected}\n{main}");
    }
    assert_eq!(
        imported.sources.get("lua/auth.lua"),
        files.get("lua/auth.lua").map(String::as_str)
    );
    let reported: Vec<_> = imported
        .report
        .iter()
        .map(|diagnostic| (diagnostic.code.as_str(), diagnostic.message.as_str()))
        .collect();
    for (code, message) in [
        (
            codes::UNSUPPORTED,
            "'lua_package_path' is not carried over: require loads",
        ),
        (
            codes::CHANGED,
            "/usr/local/openresty/nginx/lua/pick.lua is read from lua/pick.lua",
        ),
        (
            codes::CHANGED,
            "/etc/ssl/certs/ca-certificates.crt holds the system's trusted roots",
        ),
        (
            codes::UNSUPPORTED,
            "'lua_ssl_certificate' is not carried over: store /etc/nginx/client.pem as a secret",
        ),
    ] {
        assert!(
            reported
                .iter()
                .any(|(found, text)| *found == code && text.starts_with(message)),
            "{message}: {reported:#?}"
        );
    }

    let lowered = lower(
        &imported.sources,
        &LowerOptions {
            environment: &BTreeMap::new(),
            previous: None,
            now: Utc::now(),
        },
    );
    let errors: Vec<_> = lowered
        .errors()
        .map(|diagnostic| &diagnostic.message)
        .collect();
    assert_eq!(
        errors,
        ["upstream app chooses its endpoints with a Lua balancer, which needs the upstream permission for every site (lua_allow upstream in http)"]
    );
    let route = &lowered.model.sites[0].routes[0];
    assert!(matches!(route.action, Action::Lua { .. }));
    assert!(route.lua.header_filter.is_some());
}

#[test]
fn rewrites_and_internal_locations_carry_over() {
    use panel_ir::{RewriteFlag, RewriteRule};

    let site = r#"server {
    listen 80;
    server_name blog.example;
    rewrite ^/feed$ /rss.xml permanent;
    location / {
        proxy_pass http://127.0.0.1:9000;
    }
    location /api/ {
        rewrite ^/api/(?<rest>.*)$ /$rest?from=$host break;
        proxy_pass http://127.0.0.1:9000;
    }
    location /errors/ {
        internal;
        return 404 "gone";
    }
    location /legacy/ {
        rewrite ^/legacy/(\d+)$ /posts/$1?$args? last;
        return 410;
    }
    location /look/ {
        rewrite (?=x) /y;
        return 204;
    }
}
"#;
    let files = BTreeMap::from([(
        "/etc/nginx/nginx.conf".to_owned(),
        format!("http {{\n{site}}}\n"),
    )]);
    let imported = import_nginx(&files, "/etc/nginx/nginx.conf").unwrap();
    let text = imported.sources.get("main.conf").unwrap();
    let environment = BTreeMap::new();
    let lowered = lower(
        &imported.sources,
        &LowerOptions {
            environment: &environment,
            previous: None,
            now: Utc::now(),
        },
    );
    assert!(
        lowered
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != panel_errors::DiagnosticSeverity::Error),
        "{:#?}\n{text}",
        lowered.diagnostics
    );
    let site = &lowered.model.sites[0];
    assert_eq!(
        site.rewrites,
        [RewriteRule::Rewrite {
            pattern: "^/feed$".into(),
            replacement: "/rss.xml".into(),
            flag: RewriteFlag::Permanent,
        }],
        "{text}"
    );
    let route = |prefix: &str| {
        site.routes
            .iter()
            .find(|route| route.matcher.path == prefix)
            .unwrap_or_else(|| panic!("no route for {prefix} in\n{text}"))
    };
    assert_eq!(
        route("/api/").rewrites,
        [RewriteRule::Rewrite {
            pattern: "^/api/(?<rest>.*)$".into(),
            replacement: "/$rest?from=$host".into(),
            flag: RewriteFlag::Break,
        }]
    );
    assert!(route("/errors/").internal, "{text}");
    assert_eq!(
        route("/legacy/").rewrites,
        [RewriteRule::Rewrite {
            pattern: "^/legacy/(\\d+)$".into(),
            replacement: "/posts/$1?$args?".into(),
            flag: RewriteFlag::Last,
        }]
    );
    assert!(route("/look/").rewrites.is_empty(), "{text}");
    assert!(
        imported
            .report
            .iter()
            .any(|diagnostic| diagnostic.code.as_str() == codes::UNSUPPORTED
                && diagnostic.message.contains("(?=x)")),
        "{:#?}",
        imported.report
    );
}
