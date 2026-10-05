#![forbid(unsafe_code)]

use chrono::Utc;
use panel_config_dsl::{codes, lower, print, LowerOptions, Lowered, Sources};
use panel_config_model::{Action, ConfigModel, MatchKind};
use panel_ir::{HealthCheckProtocol, LoadBalancingPolicy, WwwRedirect};
use std::collections::BTreeMap;

const SHOP: &str = r#"language_version 1;

http {
    set $backend 10.0.0.11;

    tls_profile shop-cert {
        certificate shop.crt;
        key shop.key;
        min_protocol TLSv1.3;
        alpn h2 http/1.1;
    }

    listener http {
        address 0.0.0.0:80;
    }

    listener https {
        address [::]:443;
        protocols http1 http2;
        tls_profile shop-cert;
        reuse_port on;
        ipv6_only off;
        default_server shop;
    }

    # The application servers
    upstream app {
        server $backend:8080 weight=3 id=0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b;
        server app-2.internal:8080 backup tls sni=app.internal;
        server 10.0.0.13:8080 down "note=being replaced";
        balance hash key=$cookie_session;
        host_header app.internal;
        tls verify_hostname=off;
        connect_timeout 2s;
        read_timeout 1m;
        keepalive off;
        max_connections 64;
        http2 on;
        health_check http path=/healthz method=head interval=10s timeout=2s rise=3 fall=2 status=200,204;
        passive_health fails=4 eject=1m30s;
        note "Primary pool";
    }

    server shop {
        id 0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a50;
        server_name shop.example www.shop.example *.shop.example;
        alias shop.example.net;
        domain legacy.shop.example off;
        listen http https;
        https_redirect on;
        www_redirect remove;
        group retail;
        tags prod eu;
        note "The storefront";
        proxy app;

        route health {
            match exact /healthz;
            priority 5;
            respond 204;
        }

        route assets {
            match prefix /assets/ host=shop.example;
            root shop index=index.html,index.htm spa=on;
        }

        route old {
            match regex "^/old/(\d+)$";
            return 301 https://shop.example/new preserve_path=off;
        }

        location = /maintenance {
            respond 503 "body=Back soon" type=text/plain retry_after=2m;
        }
    }
}
"#;

fn options(environment: &BTreeMap<String, String>) -> LowerOptions<'_> {
    LowerOptions {
        environment,
        previous: None,
        now: Utc::now(),
    }
}

fn read(text: &str) -> Lowered {
    lower(&Sources::single(text), &options(&BTreeMap::new()))
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

#[test]
fn a_complete_configuration_reads_into_the_model() {
    let lowered = read(SHOP);
    let errors: Vec<_> = lowered.errors().collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let model = &lowered.model;

    let profile = &model.tls_profiles[0];
    assert_eq!(
        (profile.id.as_str(), profile.min_protocol.as_str()),
        ("shop-cert", "TLSv1.3")
    );
    assert_eq!(profile.alpn.len(), 2);

    let https = &model.listeners[1];
    assert_eq!(https.address, "[::]:443");
    assert_eq!(https.tls_profile_id.as_deref(), Some("shop-cert"));
    assert!(https.reuse_port);
    assert_eq!(https.ipv6_only, Some(false));
    assert_eq!(https.default_site_id, Some(model.sites[0].id));

    let app = &model.upstreams[0];
    assert_eq!(app.nodes.len(), 3);
    assert_eq!(
        (
            app.nodes[0].host.as_str(),
            app.nodes[0].port,
            app.nodes[0].weight
        ),
        ("10.0.0.11", 8080, 3)
    );
    assert_eq!(
        app.nodes[0].id.to_string(),
        "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"
    );
    assert!(app.nodes[1].backup && app.nodes[1].tls);
    assert_eq!(app.nodes[1].sni.as_deref(), Some("app.internal"));
    assert!(!app.nodes[2].enabled);
    assert_eq!(app.nodes[2].note.as_deref(), Some("being replaced"));
    assert_eq!(
        app.balancing,
        LoadBalancingPolicy::ConsistentHash {
            key: "cookie:session".into()
        }
    );
    assert!(!app.tls.verify_hostname && app.tls.verify_certificate);
    assert_eq!(app.connection.connect_timeout_ms, Some(2_000));
    assert_eq!(app.connection.read_timeout_ms, Some(60_000));
    assert!(!app.connection.keepalive && app.connection.http2);
    assert_eq!(app.connection.max_connections, Some(64));
    let check = app.health_check.as_ref().unwrap();
    assert_eq!(check.protocol, HealthCheckProtocol::Http);
    assert_eq!(
        (
            check.method.as_str(),
            check.interval_ms,
            check.healthy_threshold
        ),
        ("HEAD", 10_000, 3)
    );
    assert_eq!(check.expected_statuses.len(), 2);
    assert_eq!(app.passive_health.as_ref().unwrap().ejection_ms, 90_000);

    let shop = &model.sites[0];
    assert_eq!(shop.id.to_string(), "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a50");
    let hosts: Vec<_> = shop
        .domains
        .iter()
        .map(|domain| {
            (
                domain.host.as_str(),
                domain.primary,
                domain.redirect,
                domain.enabled,
            )
        })
        .collect();
    assert_eq!(
        hosts,
        vec![
            ("shop.example", true, false, true),
            ("www.shop.example", false, false, true),
            ("*.shop.example", false, false, true),
            ("shop.example.net", false, true, true),
            ("legacy.shop.example", false, false, false),
        ]
    );
    assert_eq!(shop.listener_ids.len(), 2);
    assert!(shop.https_redirect);
    assert_eq!(shop.www_redirect, WwwRedirect::RemoveWww);
    assert_eq!(
        shop.action,
        Action::Proxy {
            upstream_id: app.id
        }
    );
    let routes: Vec<_> = shop
        .routes
        .iter()
        .map(|route| (route.name.as_deref(), route.priority, route.matcher.kind))
        .collect();
    assert_eq!(
        routes,
        vec![
            (Some("health"), 5, MatchKind::Exact),
            (Some("assets"), 20, MatchKind::Prefix),
            (Some("old"), 30, MatchKind::Regex),
            (None, 40, MatchKind::Exact),
        ]
    );
    assert_eq!(shop.routes[2].matcher.path, r"^/old/(\d+)$");
    assert_eq!(
        shop.routes[3].action,
        Action::Respond {
            status: 503,
            body: Some("Back soon".into()),
            content_type: Some("text/plain".into()),
            retry_after_seconds: Some(120)
        }
    );
    let line = SHOP
        .lines()
        .position(|line| line.trim_start().starts_with("location"))
        .unwrap()
        + 1;
    let location = format!("main.conf:{line}.9-16");
    assert!(messages(&lowered)
        .iter()
        .any(|(code, span, _)| code == codes::DEPRECATED && *span == location));
}

fn same_configuration(left: &ConfigModel, right: &ConfigModel) -> bool {
    let normalize = |model: &ConfigModel| {
        let mut model = model.clone();
        for site in &mut model.sites {
            site.created_at = chrono::DateTime::UNIX_EPOCH;
            site.updated_at = chrono::DateTime::UNIX_EPOCH;
            site.domains.sort_by(|a, b| a.host.cmp(&b.host));
        }
        for upstream in &mut model.upstreams {
            upstream.created_at = chrono::DateTime::UNIX_EPOCH;
            upstream.updated_at = chrono::DateTime::UNIX_EPOCH;
        }
        model
    };
    normalize(left) == normalize(right)
}

#[test]
fn printing_round_trips_and_is_canonical() {
    let first = read(SHOP);
    let printed = print(&first.model);
    let second = read(&printed);
    assert!(second.is_valid(), "{:#?}\n{printed}", second.diagnostics);
    assert!(second.insertions.is_empty(), "{printed}");
    assert!(same_configuration(&first.model, &second.model), "{printed}");
    assert_eq!(print(&second.model), printed);
    assert!(printed.starts_with("language_version 1;\n\nhttp {\n    tls_profile shop-cert {"));
    assert!(printed.contains(
        "        server 10.0.0.11:8080 weight=3 id=0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b;\n"
    ));
    assert!(printed.contains("        balance hash key=$cookie_session;\n"));
    assert!(printed.contains("            match regex \"^/old/(\\d+)$\";\n"));
    assert!(printed.contains("            match prefix /assets/ host=shop.example;\n"));
    assert!(printed.contains("        route {\n"));
}

#[test]
fn tls_profiles_name_certificates_of_the_inventory() {
    let text = "language_version 1;\nhttp {\n    tls_profile edge {\n        certificate_id example.com;\n    }\n}\n";
    let lowered = read(text);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let profile = &lowered.model.tls_profiles[0];
    assert_eq!(
        profile.certificate_id.as_ref().map(|id| id.as_str()),
        Some("example.com")
    );
    assert!(profile.certificate_secret_id.is_empty());
    let printed = print(&lowered.model);
    assert!(
        printed.contains("    tls_profile edge {\n        certificate_id example.com;\n    }\n"),
        "{printed}"
    );
    assert!(same_configuration(&lowered.model, &read(&printed).model));

    let invalid = read(&text.replace("example.com", "Example_COM"));
    assert!(invalid
        .errors()
        .any(|diagnostic| diagnostic.code.as_str() == codes::TYPE
            && diagnostic.message.contains("certificate id")));
    let both = read(&text.replace(
        "certificate_id example.com;",
        "certificate_id example.com;\n        certificate site.pem;\n        key site.key;",
    ));
    assert!(both
        .errors()
        .any(|diagnostic| diagnostic.message.contains("not both")));
}

#[test]
fn tls_settings_and_hsts_read_and_print() {
    let text = "language_version 1;\nhttp {\n    tls_profile edge {\n        certificate_id example.com;\n        max_protocol TLSv1.2;\n        ciphers TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256;\n        session_resumption off;\n    }\n    server shop {\n        server_name shop.example;\n        tls_profile edge;\n        hsts max_age=365d include_subdomains preload;\n        respond 200 \"body=ok\";\n    }\n}\n";
    let lowered = read(text);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let profile = &lowered.model.tls_profiles[0];
    assert_eq!(profile.max_protocol.as_deref(), Some("TLSv1.2"));
    assert_eq!(profile.cipher_suites.len(), 2);
    assert!(!profile.session_resumption);
    let hsts = lowered.model.sites[0].hsts.unwrap();
    assert_eq!(
        (hsts.max_age_seconds, hsts.include_subdomains, hsts.preload),
        (31_536_000, true, true)
    );
    let printed = print(&lowered.model);
    for line in [
        "        max_protocol TLSv1.2;\n",
        "        ciphers TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256;\n",
        "        session_resumption off;\n",
        "        hsts max_age=365d include_subdomains preload;\n",
    ] {
        assert!(printed.contains(line), "{line}{printed}");
    }
    assert!(same_configuration(&lowered.model, &read(&printed).model));

    for (from, to, code, message) in [
        (
            "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
            "RC4_MD5",
            codes::TYPE,
            "is not a cipher suite",
        ),
        ("max_age=365d ", "", codes::ARGUMENTS, "hsts needs max_age="),
        (
            "session_resumption off",
            "ocsp_stapling on",
            codes::NO_EFFECT,
            "OCSP stapling is reserved",
        ),
        (
            "max_protocol TLSv1.2",
            "max_protocol TLSv1.1",
            codes::TYPE,
            "is not TLSv1.2 or TLSv1.3",
        ),
    ] {
        let lowered = read(&text.replace(from, to));
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_str() == code
                    && diagnostic.message.contains(message)),
            "{message}: {:#?}",
            lowered.diagnostics
        );
    }
}

#[test]
fn security_policies_read_print_and_warn() {
    let text = r#"language_version 1;
http {
    security_policy staff {
        allow 10.0.0.0/8 2001:db8::/32;
        deny 10.9.0.0/16;
        methods get POST;
        deny_paths /.git /admin/internal;
        deny_user_agents "^curl/" sqlmap;
        referers none *.shop.example shop.example;
        basic_auth staff.htpasswd "realm=Staff area";
        max_header_size 16k;
        max_body_size 10m;
        body_timeout 30s;
        rate_limit 10r/s burst=20;
        rate_limit 300r/m key=$http_x_api_key;
        rate_limit 5r/10s key=$route;
        max_concurrent 8;
        limited_response 503 "body=Slow down" type=text/plain;
    }
    security_policy login {
        rate_limit 5r/m key=$client_ip;
    }
    listener http {
        address 0.0.0.0:80;
        trusted_proxies 192.0.2.0/24 198.51.100.7;
        real_ip_header x-real-ip;
        request_head_timeout 15s;
    }
    server shop {
        server_name shop.example;
        security_policy staff;
        respond 200 "body=ok";
        route {
            match exact /login;
            security_policy login;
            respond 200 "body=login";
        }
    }
}
"#;
    let lowered = read(text);
    assert!(
        lowered.errors().next().is_none(),
        "{:#?}",
        lowered.diagnostics
    );
    let policy = &lowered.model.security_policies[0];
    assert_eq!(policy.allowed_methods, ["GET", "POST"]);
    assert_eq!(policy.denied_user_agents, ["^curl/", "sqlmap"]);
    let referer = policy.referer.as_ref().unwrap();
    assert!(referer.allow_empty);
    assert_eq!(referer.allowed_hosts, ["*.shop.example", "shop.example"]);
    let auth = policy.basic_auth.as_ref().unwrap();
    assert_eq!(
        (auth.realm.as_str(), auth.users_secret_id.as_str()),
        ("Staff area", "staff.htpasswd")
    );
    assert_eq!(policy.max_body_bytes, Some(10 << 20));
    assert_eq!(policy.body_timeout_seconds, Some(30));
    assert_eq!(policy.rate_limits.len(), 3);
    assert_eq!(
        policy.rate_limits[1].key,
        panel_ir::RateLimitKey::Header {
            name: "x-api-key".into()
        }
    );
    assert_eq!(
        (
            policy.rate_limits[2].requests,
            policy.rate_limits[2].per_seconds
        ),
        (5, 10)
    );
    assert_eq!(policy.limited_response.as_ref().unwrap().status, 503);
    let listener = &lowered.model.listeners[0];
    assert_eq!(listener.trusted_proxies.len(), 2);
    assert_eq!(listener.real_ip_header, panel_ir::RealIpHeader::XRealIp);
    assert_eq!(listener.request_head_timeout_seconds, Some(15));
    let site = &lowered.model.sites[0];
    assert_eq!(site.security_policy_id.as_deref(), Some("staff"));
    assert_eq!(site.routes[0].security_policy_id.as_deref(), Some("login"));
    assert!(
        lowered.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_str() == codes::EXPOSURE
                && diagnostic
                    .message
                    .contains("asks for passwords on listener http")
        }),
        "{:#?}",
        lowered.diagnostics
    );

    let printed = print(&lowered.model);
    for line in [
        "        methods GET POST;\n",
        "        referers none *.shop.example shop.example;\n",
        "        basic_auth staff.htpasswd \"realm=Staff area\";\n",
        "        rate_limit 10r/s burst=20;\n",
        "        rate_limit 300r/m key=$http_x_api_key;\n",
        "        rate_limit 5r/10s key=$route;\n",
        "        rate_limit 5r/m;\n",
        "        trusted_proxies 192.0.2.0/24 198.51.100.7;\n",
        "        real_ip_header x-real-ip;\n",
        "        request_head_timeout 15s;\n",
        "        security_policy staff;\n",
        "            security_policy login;\n",
    ] {
        assert!(printed.contains(line), "{line}{printed}");
    }
    assert!(same_configuration(&lowered.model, &read(&printed).model));

    let secure = read(&text.replace(
        "server_name shop.example;",
        "server_name shop.example;\n        https_redirect on;",
    ));
    assert!(!secure
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code.as_str() == codes::EXPOSURE));
    let everyone = read(&text.replace("198.51.100.7", "0.0.0.0/0"));
    assert!(everyone
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("trusts 0.0.0.0/0 as proxies")));

    for (from, to, message) in [
        ("10.9.0.0/16", "10.9.0.0/33", "is not an IP network"),
        ("methods get", "methods \"GE T\"", "is not an HTTP method"),
        (
            "deny_paths /.git",
            "deny_paths .git",
            "does not start with /",
        ),
        ("10r/s", "10/s", "is not a rate"),
        ("key=$route", "key=$uri", "is not a rate limit key"),
        (
            "body_timeout 30s",
            "body_timeout 1500ms",
            "whole number of seconds",
        ),
        (
            "request_head_timeout 15s",
            "request_head_timeout 10m",
            "request head timeout must be 1 to 300 seconds",
        ),
        ("max_body_size 10m", "max_body_size 0", "is not a size"),
        ("referers none", "referers none bad*host", "is not a host"),
        ("\"^curl/\"", "\"(\"", "user agent pattern"),
        (
            "security_policy login;",
            "security_policy missing;",
            "does not exist",
        ),
        (
            "real_ip_header x-real-ip",
            "real_ip_header via",
            "is not x-forwarded-for",
        ),
        (
            "limited_response 503",
            "limited_response 200",
            "status must be 400 to 599",
        ),
    ] {
        let lowered = read(&text.replace(from, to));
        assert!(
            lowered
                .errors()
                .any(|diagnostic| diagnostic.message.contains(message)),
            "{message}: {:#?}",
            lowered.diagnostics
        );
    }
}

#[test]
fn http_policies_read_print_and_refuse_mistakes() {
    let text = r#"language_version 1;
http {
    set $brand shop;
    http_policy site {
        response_header set Strict-Transport-Security "max-age=63072000; includeSubDomains";
        response_header remove X-Powered-By;
        server_header replace $brand;
    }
    http_policy api {
        request_header remove X-Internal X-Debug;
        request_header set X-Tenant $host;
        request_header add X-Hop "via $$edge";
        response_header add Link "</app.css>; rel=preload";
        cors {
            origins https://*.shop.example https://admin.shop.example;
            methods PUT DELETE;
            headers X-Api-Key;
            expose X-Request-Id;
            credentials on;
            max_age 10m;
        }
        compress gzip br zstd types=text/*,application/json min_size=1k;
    }
    server shop {
        server_name shop.example;
        http_policy site;
        respond 200 "body=ok";
        route {
            match prefix /api;
            http_policy api;
            respond 200 "body=api";
        }
    }
}
"#;
    let lowered = read(text);
    assert!(
        lowered.errors().next().is_none(),
        "{:#?}",
        lowered.diagnostics
    );
    let [site_policy, api] = lowered.model.http_policies.as_slice() else {
        panic!("{:#?}", lowered.model.http_policies);
    };
    assert_eq!(site_policy.response.remove, ["X-Powered-By"]);
    assert_eq!(
        site_policy.response.set[0].value,
        "max-age=63072000; includeSubDomains"
    );
    assert_eq!(
        site_policy.server,
        panel_ir::ServerHeader::Replace {
            value: "shop".into()
        }
    );
    assert_eq!(api.request.remove, ["X-Internal", "X-Debug"]);
    assert_eq!(api.request.set[0].value, "$host");
    assert_eq!(api.request.add[0].value, "via $$edge");
    let cors = api.cors.as_ref().unwrap();
    assert_eq!(cors.allowed_origins.len(), 2);
    assert!(cors.allow_credentials);
    assert_eq!(cors.max_age_seconds, Some(600));
    let compression = api.compression.as_ref().unwrap();
    assert_eq!(compression.algorithms.len(), 3);
    assert_eq!(compression.types, ["text/*", "application/json"]);
    assert_eq!(compression.min_bytes, 1024);
    let site = &lowered.model.sites[0];
    assert_eq!(site.http_policy_id.as_deref(), Some("site"));
    assert_eq!(site.routes[0].http_policy_id.as_deref(), Some("api"));

    let printed = print(&lowered.model);
    for line in [
        "        response_header set Strict-Transport-Security \"max-age=63072000; includeSubDomains\";\n",
        "        response_header remove X-Powered-By;\n",
        "        server_header replace shop;\n",
        "        request_header remove X-Internal X-Debug;\n",
        "        request_header set X-Tenant $host;\n",
        "        request_header add X-Hop \"via $$edge\";\n",
        "            origins https://*.shop.example https://admin.shop.example;\n",
        "            credentials on;\n",
        "            max_age 10m;\n",
        "        compress gzip br zstd types=text/*,application/json min_size=1k;\n",
        "        http_policy site;\n",
        "            http_policy api;\n",
    ] {
        assert!(printed.contains(line), "{line}{printed}");
    }
    assert!(same_configuration(&lowered.model, &read(&printed).model));

    for (from, to, message) in [
        (
            "request_header set X-Tenant $host;",
            "request_header set X-Tenant;",
            "takes remove <name> ..., set <name> <value> or add <name> <value>",
        ),
        (
            "request_header set X-Tenant $host;",
            "request_header set X-Tenant $nothing;",
            "nothing",
        ),
        (
            "request_header set X-Tenant",
            "request_header set Host",
            "changes the request field host, which the gateway keeps",
        ),
        (
            "server_header replace $brand;",
            "server_header hide;",
            "server_header is keep, remove or replace <value>",
        ),
        (
            "origins https://*.shop.example",
            "origins *",
            "allows credentials for every origin",
        ),
        ("max_age 10m;", "max_age 1500ms;", "whole number of seconds"),
        ("gzip br zstd", "gzip deflate", "is not a content coding"),
        (
            " types=text/*,application/json",
            "",
            "compress names the media types it compresses",
        ),
        ("min_size=1k", "min_size=lots", "is not a size"),
        ("http_policy api;", "http_policy missing;", "does not exist"),
    ] {
        let lowered = read(&text.replace(from, to));
        assert!(
            lowered
                .errors()
                .any(|diagnostic| diagnostic.message.contains(message)),
            "{message}: {:#?}",
            lowered.diagnostics
        );
    }
}

#[test]
fn unverified_tls_nodes_are_warned_about() {
    let text = "language_version 1;\nhttp {\n    upstream app {\n        server 10.0.0.1:443 tls;\n        tls verify=off;\n    }\n    server s {\n        server_name s.example;\n        proxy app;\n    }\n}\n";
    let lowered = read(text);
    assert!(
        lowered.errors().next().is_none(),
        "{:#?}",
        lowered.diagnostics
    );
    assert!(
        messages(&lowered)
            .iter()
            .any(|(code, span, message)| code == codes::EXPOSURE
                && span.starts_with("main.conf:3.")
                && message.contains("does not verify its TLS nodes")),
        "{:#?}",
        lowered.diagnostics
    );
    let verified = read(&text.replace("        tls verify=off;\n", ""));
    assert!(!verified
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code.as_str() == codes::EXPOSURE));
}

#[test]
fn literal_dollars_survive_a_round_trip() {
    let text = "language_version 1;\nhttp {\n    upstream app {\n        server 10.0.0.1:80;\n    }\n    server s {\n        server_name s.example;\n        note \"uses $HOME\";\n        respond 200 \"body=cost $$5 or $$amount\";\n    }\n}\n";
    let lowered = read(text);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let site = &lowered.model.sites[0];
    assert_eq!(site.note.as_deref(), Some("uses $HOME"));
    // Response bodies are templates, so literal dollars stay escaped.
    assert_eq!(
        site.action,
        Action::Respond {
            status: 200,
            body: Some("cost $$5 or $$amount".into()),
            content_type: None,
            retry_after_seconds: None
        }
    );
    assert_eq!(
        panel_ir::template::literal("cost $$5 or $$amount").as_deref(),
        Some("cost $5 or $amount")
    );
    let again = read(&print(&lowered.model));
    assert!(again.is_valid(), "{:#?}", again.diagnostics);
    assert_eq!(again.model.sites[0].action, site.action);
}

#[test]
fn redirects_and_responses_keep_request_variables() {
    let text = "language_version 1;\nhttp {\n    set $landing /landing;\n    server s {\n        server_name s.example;\n        return 302 https://$host$landing$uri preserve_path=off;\n        route {\n            match exact /whoami;\n            respond 200 \"body=${client_ip}x from $http_x_tenant\";\n        }\n    }\n}\n";
    let lowered = read(text);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let site = &lowered.model.sites[0];
    assert!(matches!(
        &site.action,
        Action::Redirect { location, .. } if location == "https://$host/landing$uri"
    ));
    assert!(matches!(
        &site.routes[0].action,
        Action::Respond { body: Some(body), .. } if body == "${client_ip}x from $http_x_tenant"
    ));
    let printed = print(&lowered.model);
    assert!(
        printed.contains("return 302 https://$host/landing$uri preserve_path=off;"),
        "{printed}"
    );
    assert!(
        printed.contains("respond 200 \"body=${client_ip}x from $http_x_tenant\";"),
        "{printed}"
    );
    assert_eq!(read(&printed).model.sites[0].action, site.action);

    let refused = read("language_version 1;\nhttp {\n    server s {\n        server_name s.example;\n        respond 200 body=$nope;\n    }\n}\n");
    assert!(
        messages(&refused)
            .iter()
            .any(|(_, span, message)| span == "main.conf:5.21-30"
                && message == "$nope is not defined")
    );
}

#[test]
fn missing_identifiers_are_assigned_and_written_back() {
    let text = "language_version 1;\nhttp {\n    upstream app {\n        server 10.0.0.1:80;\n    }\n    server s { # storefront\n        server_name s.example;\n        proxy app;\n    }\n}\n";
    let lowered = read(text);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    assert_eq!(lowered.insertions.len(), 3);
    let mut patched = text.to_owned();
    let mut insertions = lowered.insertions.clone();
    insertions.sort_by_key(|insertion| std::cmp::Reverse(insertion.offset));
    for insertion in insertions {
        patched.insert_str(insertion.offset, &insertion.text);
    }
    let again = read(&patched);
    assert!(again.is_valid(), "{:#?}\n{patched}", again.diagnostics);
    assert!(again.insertions.is_empty(), "{patched}");
    assert_eq!(again.model.sites[0].id, lowered.model.sites[0].id);
    assert_eq!(
        again.model.upstreams[0].nodes[0].id,
        lowered.model.upstreams[0].nodes[0].id
    );
    assert!(
        patched.contains("    server s { # storefront\n        id "),
        "{patched}"
    );
}

#[test]
fn each_problem_is_reported_at_its_source() {
    let text = "\
language_version 1;
http {
    upstream app {
        server 10.0.0.1:80 wieght=2;
        connect_timeout soon;
    }
    server s {
        server_nmae s.example;
        proxy missing;
        listener x { address 0.0.0.0:80; }
        route {
            match prefix /a;
            proxy app;
            root /srv;
        }
    }
    server s {
        server_name t.example;
        respond 200 \"body=$undefined\";
    }
}
";
    let lowered = read(text);
    let found = messages(&lowered);
    let expect = [
        (
            codes::ARGUMENTS,
            "main.conf:4.28-35",
            "unknown parameter \"wieght\"",
        ),
        (
            codes::TYPE,
            "main.conf:5.25-28",
            "\"soon\" is not a duration",
        ),
        (
            codes::UNKNOWN_DIRECTIVE,
            "main.conf:8.9-19",
            "unknown directive 'server_nmae'",
        ),
        (
            codes::CONTEXT,
            "main.conf:10.9-16",
            "'listener' is not allowed in server",
        ),
        (
            codes::DUPLICATE,
            "main.conf:14.13-16",
            "the route already has an action",
        ),
        (
            codes::DUPLICATE,
            "main.conf:17.12",
            "server \"s\" is defined twice",
        ),
        (
            codes::REFERENCE,
            "main.conf:9.15-21",
            "no upstream is named \"missing\"",
        ),
    ];
    for (code, span, message) in expect {
        assert!(
            found
                .iter()
                .any(|(c, s, m)| c == code && s == span && m == message),
            "missing {code} {span} {message}\n{found:#?}"
        );
    }
    let typo = lowered
        .diagnostics
        .iter()
        .find(|d| d.code.as_str() == codes::UNKNOWN_DIRECTIVE)
        .unwrap();
    assert_eq!(typo.help.as_deref(), Some("did you mean 'server_name'?"));
    assert!(!lowered.is_valid());
}

#[test]
fn regular_expressions_are_compiled_when_read() {
    let lowered = read(
        "language_version 1;\nhttp {\n    server s {\n        server_name s.example;\n        route {\n            match regex \"^(/api\";\n            respond 204;\n        }\n    }\n}\n",
    );
    let found = messages(&lowered);
    assert!(
        found
            .iter()
            .any(|(code, span, message)| code == "VALIDATION_FAILED"
                && span == "main.conf:5.9-8.9"
                && message == "the regular expression \"^(/api\" does not compile: unclosed group"),
        "{found:#?}"
    );
    assert!(!lowered.is_valid());
}

#[test]
fn quotes_inside_a_parameter_are_part_of_the_value() {
    let text = "\
language_version 1;
http {
    server s {
        server_name s.example;
        respond 200 body=\"ok\";
    }
    server t {
        server_name t.example;
        respond 503 \"body=be right back\";
    }
}
";
    let lowered = read(text);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let warnings: Vec<_> = lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.as_str() == codes::QUOTES)
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert_eq!(
        warnings[0].source_span.as_deref(),
        Some("main.conf:5.21-29")
    );
    assert_eq!(warnings[0].help.as_deref(), Some("write it as body=ok"));
}

#[test]
fn variables_includes_and_versions() {
    let mut files = BTreeMap::new();
    files.insert(
        "main.conf".to_owned(),
        "language_version 1;\nhttp {\n    set $root /srv/www;\n    include sites/*.conf;\n}\n"
            .to_owned(),
    );
    files.insert(
        "sites/a.conf".to_owned(),
        "server a {\n    server_name ${env:PINGORA_PANEL_DSL_HOST};\n    root $root;\n}\n"
            .to_owned(),
    );
    files.insert(
        "sites/b.conf".to_owned(),
        "include ../main.conf;\n".to_owned(),
    );
    let sources = Sources::new(files.clone()).unwrap();
    let mut environment = BTreeMap::new();
    environment.insert("PINGORA_PANEL_DSL_HOST".to_owned(), "a.example".to_owned());
    let lowered = lower(&sources, &options(&environment));
    let found = messages(&lowered);
    assert_eq!(lowered.model.sites[0].domains[0].host.as_str(), "a.example");
    assert_eq!(
        lowered.model.sites[0].action,
        Action::Static {
            root: "/srv/www".into(),
            index_files: vec!["index.html".into()],
            spa_fallback: false
        }
    );
    assert!(
        found
            .iter()
            .any(|(code, span, _)| code == codes::INCLUDE && span == "sites/b.conf:1.9-20"),
        "{found:#?}"
    );

    files.insert("sites/b.conf".to_owned(), "include b.conf;\n".to_owned());
    let lowered = lower(
        &Sources::new(files.clone()).unwrap(),
        &options(&environment),
    );
    assert!(messages(&lowered)
        .iter()
        .any(|(code, _, message)| code == codes::INCLUDE
            && message == "files include each other in a cycle: sites/b.conf -> sites/b.conf"));

    files.remove("sites/b.conf");
    files.insert(
        "sites/a.conf".to_owned(),
        "server a { server_name ${env:HOME}; root $host; }\n".to_owned(),
    );
    let found = messages(&lower(
        &Sources::new(files.clone()).unwrap(),
        &options(&environment),
    ));
    assert!(found
        .iter()
        .any(|(code, _, message)| code == codes::VARIABLE
            && message == "${env:HOME} is not readable"));
    assert!(found
        .iter()
        .any(|(code, _, message)| code == codes::VARIABLE
            && message.starts_with("$host is known only per request")));

    let found = messages(&read("http {\n}\n"));
    assert_eq!(found[0].0, codes::VERSION);
    let found = messages(&read("language_version 2;\n"));
    assert_eq!(
        found[0],
        (
            codes::VERSION.to_owned(),
            "main.conf:1.18".to_owned(),
            "language version \"2\" is not supported".to_owned()
        )
    );
}

#[test]
fn model_validation_points_at_the_resource() {
    let text =
        "language_version 1;\nhttp {\n    listener a {\n        address 0.0.0.0:0;\n    }\n}\n";
    let lowered = read(text);
    let found = messages(&lowered);
    assert!(
        found
            .iter()
            .any(|(_, span, message)| span == "main.conf:3.5-5.5"
                && message.contains("must not be zero")),
        "{found:#?}"
    );
}

#[test]
fn logging_directives_read_and_print() {
    use panel_ir::{AccessLogFormat, LogFiles};
    use std::collections::BTreeSet;

    let text = "language_version 1;\nhttp {\n    access_log format=combined;\n    log_field tenant $http_x_tenant;\n    log_redact_query token sig;\n    log_redact_headers X-Api-Key;\n    log_files max_size=50m rotate=size keep=14d max_files=10;\n\n    upstream app {\n        server 10.0.0.11:8080;\n    }\n\n    server shop {\n        server_name shop.example;\n        access_log on format=json;\n        log_field region eu;\n        proxy app;\n\n        route health {\n            match prefix /health;\n            access_log off;\n            proxy app;\n        }\n    }\n}\n";
    let lowered = read(text);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let logging = &lowered.model.logging;
    assert_eq!(logging.access.format, Some(AccessLogFormat::Combined));
    assert_eq!(logging.access.fields["tenant"], "$http_x_tenant");
    assert_eq!(
        logging.redact_query,
        Some(BTreeSet::from(["sig".to_owned(), "token".to_owned()]))
    );
    assert!(logging.redact_headers.contains("x-api-key"));
    assert_eq!(
        logging.files,
        LogFiles {
            max_size_bytes: 50 * 1024 * 1024,
            rotate_daily: false,
            keep_days: 14,
            max_files: 10,
        }
    );
    let site = &lowered.model.sites[0];
    assert_eq!(site.access_log.enabled, Some(true));
    assert_eq!(site.access_log.format, Some(AccessLogFormat::Json));
    assert_eq!(site.access_log.fields["region"], "eu");
    assert_eq!(site.routes[0].access_log.enabled, Some(false));

    let printed = print(&lowered.model);
    for line in [
        "    access_log format=combined;\n",
        "    log_field tenant $http_x_tenant;\n",
        "    log_redact_query sig token;\n",
        "    log_redact_headers x-api-key;\n",
        "    log_files max_size=50m rotate=size keep=14d max_files=10;\n",
        "        access_log on format=json;\n",
        "        log_field region eu;\n",
        "            access_log off;\n",
    ] {
        assert!(printed.contains(line), "{line}{printed}");
    }
    assert!(same_configuration(&lowered.model, &read(&printed).model));
    let unlogged = read("language_version 1;\nhttp {\n    log_redact_query off;\n}\n");
    assert_eq!(unlogged.model.logging.redact_query, Some(BTreeSet::new()));
    assert!(print(&unlogged.model).contains("    log_redact_query off;\n"));

    for (from, to, code, message) in [
        (
            "log_field tenant",
            "log_field url.path",
            codes::TYPE,
            "cannot name a log field",
        ),
        (
            "format=combined",
            "format=xml",
            codes::TYPE,
            "is not json or combined",
        ),
        (
            "keep=14d",
            "keep=36h",
            codes::TYPE,
            "is not a number of days",
        ),
        (
            "rotate=size",
            "rotate=hourly",
            codes::TYPE,
            "is not daily or size",
        ),
        (
            "log_field region eu;",
            "log_field region eu;\n        log_field region us;",
            codes::DUPLICATE,
            "written twice",
        ),
        (
            "access_log off;",
            "access_log off on;",
            codes::ARGUMENTS,
            "takes on or off once",
        ),
    ] {
        let broken = read(&text.replacen(from, to, 1));
        assert!(
            broken
                .errors()
                .any(|diagnostic| diagnostic.code.as_str() == code
                    && diagnostic.message.contains(message)),
            "{message}: {:#?}",
            broken.diagnostics
        );
    }
}

const CONDITIONED: &str = r#"language_version 1;

http {
    upstream app {
        server 10.0.0.11:8080;
    }

    server shop {
        server_name shop.example *.shop.example;

        route canary {
            match prefix /api/;
            method GET HEAD;
            host *.shop.example;
            header x-env = staging ignore_case;
            query tag ^= "new arrivals";
            cookie beta ~ "^on$";
            client 10.0.0.0/8 2001:db8::1;
            user_agent ~* bot;
            referer $= /cart;
            content_type application/json text/*;
            any {
                header x-canary present;
                cookie canary *= 1;
            }
            not {
                client 192.0.2.0/24;
                query debug absent;
            }
            proxy app;
        }

        proxy app;
    }
}
"#;

#[test]
fn route_conditions_read_into_the_model_and_print_back() {
    use panel_config_model::{RouteCondition, ValueTest};

    let first = read(CONDITIONED);
    assert!(first.is_valid(), "{:#?}", first.diagnostics);
    let conditions = &first.model.sites[0].routes[0].matcher.conditions;
    assert_eq!(conditions.len(), 11);
    assert_eq!(
        conditions[0],
        RouteCondition::Method {
            methods: vec!["GET".into(), "HEAD".into()]
        }
    );
    assert_eq!(
        conditions[2],
        RouteCondition::Header {
            name: "x-env".into(),
            test: ValueTest::Equals {
                value: "staging".into(),
                ignore_case: true
            }
        }
    );
    assert_eq!(
        conditions[3],
        RouteCondition::Query {
            name: "tag".into(),
            test: ValueTest::Prefix {
                value: "new arrivals".into(),
                ignore_case: false
            }
        }
    );
    assert_eq!(
        conditions[6],
        RouteCondition::UserAgent {
            test: ValueTest::Regex {
                pattern: "bot".into(),
                ignore_case: true
            }
        }
    );
    let RouteCondition::Not { condition } = &conditions[10] else {
        panic!("the last condition negates two");
    };
    assert!(
        matches!(condition.as_ref(), RouteCondition::All { conditions } if conditions.len() == 2)
    );

    let printed = print(&first.model);
    let second = read(&printed);
    assert!(second.is_valid(), "{:#?}\n{printed}", second.diagnostics);
    assert!(same_configuration(&first.model, &second.model), "{printed}");
    assert_eq!(print(&second.model), printed);
    for line in [
        "            method GET HEAD;\n",
        "            header x-env = staging ignore_case;\n",
        "            query tag ^= \"new arrivals\";\n",
        "            user_agent ~* bot;\n",
        "            any {\n                header x-canary present;\n",
        "            not {\n                client 192.0.2.0/24;\n                query debug absent;\n",
    ] {
        assert!(printed.contains(line), "{line:?} in\n{printed}");
    }
}

#[test]
fn malformed_conditions_are_reported_where_they_are_written() {
    let text = "language_version 1;\nhttp {\n    upstream app {\n        server 10.0.0.1:80;\n    }\n    server s {\n        server_name s.example;\n        route {\n            match prefix /;\n            method \"GET POST\";\n            client 10.0.0.0/33;\n            header x-env equals staging;\n            cookie beta ~ \"(\";\n            content_type json;\n            any {\n            }\n            proxy app;\n        }\n        proxy app;\n    }\n}\n";
    let lowered = read(text);
    let found = messages(&lowered);
    let positions: Vec<&str> = found.iter().map(|(_, span, _)| span.as_str()).collect();
    assert_eq!(
        positions,
        [
            "main.conf:10.20-29",
            "main.conf:11.20-30",
            "main.conf:12.26-31",
            "main.conf:13.27-29",
            "main.conf:14.26-29",
            "main.conf:15.13-16.13",
        ],
        "{found:#?}"
    );
    assert!(
        found[2].2.contains("expected present, absent"),
        "{found:#?}"
    );
}
