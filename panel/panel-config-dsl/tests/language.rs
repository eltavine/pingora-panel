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
fn literal_dollars_survive_a_round_trip() {
    let text = "language_version 1;\nhttp {\n    upstream app {\n        server 10.0.0.1:80;\n    }\n    server s {\n        server_name s.example;\n        note \"uses $HOME\";\n        respond 200 \"body=cost $$5 or $$amount\";\n    }\n}\n";
    let lowered = read(text);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let site = &lowered.model.sites[0];
    assert_eq!(site.note.as_deref(), Some("uses $HOME"));
    assert_eq!(
        site.action,
        Action::Respond {
            status: 200,
            body: Some("cost $5 or $amount".into()),
            content_type: None,
            retry_after_seconds: None
        }
    );
    let again = read(&print(&lowered.model));
    assert!(again.is_valid(), "{:#?}", again.diagnostics);
    assert_eq!(again.model.sites[0].action, site.action);
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
