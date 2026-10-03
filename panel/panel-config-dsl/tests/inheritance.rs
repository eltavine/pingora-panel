#![forbid(unsafe_code)]

use chrono::Utc;
use panel_config_dsl::{
    codes,
    explain::{explain, SettingSource},
    lower,
    schema::Context,
    LowerOptions, Lowered, Sources,
};
use std::collections::BTreeMap;

fn read(text: &str) -> (Sources, Lowered) {
    let sources = Sources::single(text);
    let lowered = lower(
        &sources,
        &LowerOptions {
            environment: &BTreeMap::new(),
            previous: None,
            now: Utc::now(),
        },
    );
    (sources, lowered)
}

/// The GNU span of the first `needle` in `text`, which sits on one line.
fn span(text: &str, needle: &str) -> String {
    let offset = text.find(needle).expect(needle);
    let line = text[..offset].matches('\n').count() + 1;
    let column = offset - text[..offset].rfind('\n').map_or(0, |newline| newline + 1) + 1;
    format!(
        "main.conf:{line}.{column}-{}",
        column + needle.chars().count() - 1
    )
}

/// The 1-based line and column of the first `needle` in `text`.
fn position(text: &str, needle: &str) -> (usize, usize) {
    let offset = text.find(needle).expect(needle);
    let line = text[..offset].matches('\n').count() + 1;
    let column = offset - text[..offset].rfind('\n').map_or(0, |newline| newline + 1) + 1;
    (line, column)
}

const SHOP: &str = r#"language_version 1;

http {
    set $api /api;

    tls_profile edge-cert {
        certificate edge.crt;
        key edge.key;
    }

    tls_profile shop-cert {
        certificate shop.crt;
        key shop.key;
    }

    listener plain {
        address 0.0.0.0:80;
    }

    listener secure {
        address 0.0.0.0:443;
        tls_profile edge-cert;
    }

    upstream app {
        server 10.0.0.1:443 tls;
        server app.internal:443 tls sni=api.internal;
        server app.example:443 tls;
        tls sni=app.internal;
    }

    server shop {
        server_name shop.example;
        domain api.shop.example tls_profile=shop-cert;
        https_redirect on;
        set $landing https://shop.example/landing;
        return 302 $landing;

        route api {
            match prefix $api;
            proxy app;
        }
    }
}
"#;

fn setting<'a>(
    settings: &'a [panel_config_dsl::explain::Setting],
    name: &str,
    scope: Option<&str>,
) -> &'a panel_config_dsl::explain::Setting {
    settings
        .iter()
        .find(|setting| setting.name == name && setting.scope.as_deref() == scope)
        .unwrap_or_else(|| panic!("no {name} {scope:?} in {settings:#?}"))
}

#[test]
fn routes_show_what_they_take_over_and_from_where() {
    let (sources, lowered) = read(SHOP);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    let (line, column) = position(SHOP, "proxy app;");
    let explained = explain(&sources, &lowered, "main.conf", line, column).unwrap();
    assert_eq!(explained.block, Context::Route);
    assert_eq!(explained.name.as_deref(), Some("api"));
    assert!(explained.resource.contains("/routes/"));
    let settings = &explained.settings;

    let matched = setting(settings, "match", None);
    assert_eq!(
        (matched.value.as_str(), matched.source),
        ("prefix $api", SettingSource::Here)
    );
    assert_eq!(
        matched.source_span.as_deref(),
        Some(span(SHOP, "match prefix $api;").as_str())
    );

    let enabled = setting(settings, "enabled", None);
    assert_eq!(
        (enabled.value.as_str(), enabled.source),
        ("on", SettingSource::Default)
    );
    assert!(enabled
        .rule
        .unwrap()
        .contains("while its server is enabled"));

    let priority = setting(settings, "priority", None);
    assert_eq!(
        (priority.value.as_str(), priority.source),
        ("10", SettingSource::Default)
    );

    let listen = setting(settings, "listen", None);
    assert_eq!(
        (listen.value.as_str(), listen.source),
        ("plain secure", SettingSource::Default)
    );

    let redirect = setting(settings, "https_redirect", None);
    assert_eq!(
        (
            redirect.value.as_str(),
            redirect.source,
            redirect.from.as_deref()
        ),
        ("on", SettingSource::Inherited, Some("server shop"))
    );
    assert_eq!(
        redirect.source_span.as_deref(),
        Some(span(SHOP, "https_redirect on;").as_str())
    );
    assert_eq!(
        setting(settings, "www_redirect", None).source,
        SettingSource::Default
    );

    let listener = setting(settings, "tls_profile", Some("shop.example"));
    assert_eq!(
        (
            listener.value.as_str(),
            listener.source,
            listener.from.as_deref()
        ),
        (
            "edge-cert",
            SettingSource::Inherited,
            Some("listener secure")
        )
    );
    assert_eq!(
        listener.source_span.as_deref(),
        Some(span(SHOP, "tls_profile edge-cert;").as_str())
    );
    let own = setting(settings, "tls_profile", Some("api.shop.example"));
    assert_eq!(
        (own.value.as_str(), own.source, own.from.as_deref()),
        ("shop-cert", SettingSource::Inherited, Some("server shop"))
    );
    assert_eq!(
        own.source_span.as_deref(),
        Some(span(SHOP, "domain api.shop.example tls_profile=shop-cert;").as_str())
    );

    let api = setting(settings, "$api", None);
    assert_eq!(
        (api.value.as_str(), api.source, api.from.as_deref()),
        ("/api", SettingSource::Inherited, Some("http"))
    );
    let landing = setting(settings, "$landing", None);
    assert_eq!(
        (landing.source, landing.from.as_deref()),
        (SettingSource::Inherited, Some("server shop"))
    );
}

#[test]
fn servers_and_upstreams_explain_their_own_values() {
    let (sources, lowered) = read(SHOP);
    let (line, column) = position(SHOP, "https_redirect on;");
    let server = explain(&sources, &lowered, "main.conf", line, column).unwrap();
    assert_eq!(
        (server.block, server.name.as_deref()),
        (Context::Server, Some("shop"))
    );
    let settings = &server.settings;
    assert_eq!(
        setting(settings, "https_redirect", None).source,
        SettingSource::Here
    );
    assert_eq!(
        setting(settings, "enabled", None).source,
        SettingSource::Default
    );
    assert_eq!(
        setting(settings, "tls_profile", Some("api.shop.example")).source,
        SettingSource::Here
    );
    assert_eq!(
        setting(settings, "tls_profile", Some("shop.example")).source,
        SettingSource::Inherited
    );
    assert_eq!(
        setting(settings, "$landing", None).source,
        SettingSource::Here
    );

    let (line, column) = position(SHOP, "server 10.0.0.1:443 tls;");
    let upstream = explain(&sources, &lowered, "main.conf", line, column).unwrap();
    assert_eq!(
        (upstream.block, upstream.name.as_deref()),
        (Context::Upstream, Some("app"))
    );
    let settings = &upstream.settings;
    let shared = setting(settings, "sni", Some("10.0.0.1:443"));
    assert_eq!(
        (shared.value.as_str(), shared.source),
        ("app.internal", SettingSource::Here)
    );
    assert_eq!(
        shared.source_span.as_deref(),
        Some(span(SHOP, "tls sni=app.internal;").as_str())
    );
    let own = setting(settings, "sni", Some("app.internal:443"));
    assert_eq!(own.value, "api.internal");
    assert_eq!(
        own.source_span.as_deref(),
        Some(span(SHOP, "server app.internal:443 tls sni=api.internal;").as_str())
    );
    assert_eq!(
        setting(settings, "balance", None).value,
        "round_robin".to_owned()
    );
    assert_eq!(
        setting(settings, "$api", None).source,
        SettingSource::Inherited
    );

    let (line, column) = position(SHOP, "address 0.0.0.0:443;");
    let listener = explain(&sources, &lowered, "main.conf", line, column).unwrap();
    assert_eq!(listener.block, Context::Listener);
    assert_eq!(
        setting(&listener.settings, "protocols", None).value,
        "http1 http2"
    );

    assert!(explain(&sources, &lowered, "main.conf", 1, 1).is_none());
    assert!(explain(&sources, &lowered, "main.conf", 400, 1).is_none());
    assert!(explain(&sources, &lowered, "other.conf", 1, 1).is_none());
}

#[test]
fn settings_without_effect_are_reported_where_they_are_written() {
    let text = r#"language_version 1;
http {
    set $unused 1;
    set $twice a;
    set $twice b;
    tls_profile x {
        certificate x.crt;
        key x.key;
    }
    listener plain {
        address 0.0.0.0:80;
    }
    upstream app {
        server 10.0.0.1:80 sni=x.internal;
        tls verify=off;
        http2 on;
    }
    upstream secure {
        server 10.0.0.2:443 tls sni=a.internal;
        tls sni=b.internal;
    }
    server shop {
        server_name shop.example;
        domain api.shop.example tls_profile=x;
        tls_profile x;
        enabled off;
        proxy app;
        route api {
            match prefix /api/;
            enabled on;
            proxy secure;
        }
    }
}
"#;
    let (_, lowered) = read(text);
    let mut found: Vec<(String, String)> = lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.as_str() == codes::NO_EFFECT)
        .map(|diagnostic| {
            (
                diagnostic.source_span.clone().unwrap_or_default(),
                diagnostic.message.clone(),
            )
        })
        .collect();
    found.sort();
    let mut expected = vec![
        (span(text, "set $unused 1;"), "$unused is never used".to_owned()),
        (
            span(text, "set $twice a;"),
            "this value of $twice is never used: it is set again before any use".to_owned(),
        ),
        (span(text, "set $twice b;"), "$twice is never used".to_owned()),
        (
            span(text, "server 10.0.0.1:80 sni=x.internal;"),
            "sni= has no effect: the node does not use TLS".to_owned(),
        ),
        (
            span(text, "tls verify=off;"),
            "tls has no effect: no node of upstream \"app\" uses TLS".to_owned(),
        ),
        (
            span(text, "http2 on;"),
            "http2 has no effect: only nodes with the tls flag negotiate HTTP/2".to_owned(),
        ),
        (
            span(text, "tls sni=b.internal;"),
            "sni=b.internal has no effect: every node with the tls flag sets its own sni="
                .to_owned(),
        ),
        (
            span(text, "tls_profile x;"),
            "tls_profile x has no effect: none of the server's listeners use TLS".to_owned(),
        ),
        (
            span(text, "domain api.shop.example tls_profile=x;"),
            "tls_profile=x of api.shop.example has no effect: none of the server's listeners use TLS"
                .to_owned(),
        ),
        (
            span(text, "enabled on;"),
            "route \"api\" is enabled, but server \"shop\" is not, so it serves nothing".to_owned(),
        ),
    ];
    expected.sort();
    assert_eq!(found, expected);
    assert!(lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.as_str() == codes::NO_EFFECT)
        .all(|diagnostic| diagnostic.help.is_some()));
}

#[test]
fn a_server_profile_every_host_replaces_has_no_effect() {
    let text = r#"language_version 1;
http {
    tls_profile x {
        certificate x.crt;
        key x.key;
    }
    listener secure {
        address 0.0.0.0:443;
        tls_profile x;
    }
    server shop {
        domain shop.example tls_profile=x;
        tls_profile x;
        respond 204;
    }
}
"#;
    let (_, lowered) = read(text);
    let found: Vec<_> = lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.as_str() == codes::NO_EFFECT)
        .map(|diagnostic| {
            (
                diagnostic.source_span.clone().unwrap_or_default(),
                diagnostic.message.as_str(),
            )
        })
        .collect();
    let at = text.rfind("tls_profile x;").unwrap();
    let line = text[..at].matches('\n').count() + 1;
    assert_eq!(
        found,
        vec![(
            format!("main.conf:{line}.9-22"),
            "tls_profile x has no effect: every host of the server sets its own tls_profile="
        )]
    );
}
