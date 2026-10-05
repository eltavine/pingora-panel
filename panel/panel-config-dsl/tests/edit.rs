#![forbid(unsafe_code)]

use chrono::Utc;
use panel_config_dsl::{
    lower,
    plan::{diff_files, plan, Change},
    reconcile, write_identifiers, LowerOptions, Lowered, Sources,
};
use panel_config_model::{Action, Upstream};
use std::collections::BTreeMap;
use uuid::Uuid;

const DRAFT: &str = "\
language_version 1;

http {
    # Plain HTTP for everyone
    listener http {
        address 0.0.0.0:80;
    }

    # The application servers
    upstream app {
        server 10.0.0.11:8080;   # keep me
    }

    # Retired, kept for reference
    server old {
        server_name old.example;
        respond 410;
    }

    server shop {
        server_name shop.example;
        proxy app;
    }
}
";

fn read(sources: &Sources) -> Lowered {
    let environment = BTreeMap::new();
    lower(
        sources,
        &LowerOptions {
            environment: &environment,
            previous: None,
            now: Utc::now(),
        },
    )
}

/// The draft as the service stores it: identifiers written in.
fn stored() -> (Sources, Lowered) {
    let first = read(&Sources::single(DRAFT));
    assert!(first.is_valid(), "{:#?}", first.diagnostics);
    let sources = write_identifiers(&Sources::single(DRAFT), &first.insertions);
    let lowered = read(&sources);
    assert!(lowered.is_valid(), "{:#?}", lowered.diagnostics);
    assert!(lowered.insertions.is_empty());
    assert_eq!(lowered.model.upstreams[0].id, first.model.upstreams[0].id);
    (sources, lowered)
}

#[test]
fn identifiers_are_written_in_without_touching_anything_else() {
    let (sources, _) = stored();
    let text = sources.get("main.conf").unwrap();
    assert!(text.contains("    upstream app {\n        id "));
    assert!(text.contains("server 10.0.0.11:8080 id="));
    assert!(text.contains(";   # keep me\n"));
    let without_ids: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("id "))
        .map(|line| match line.find(" id=") {
            Some(at) => format!("{}{}\n", &line[..at], &line[line.find(';').unwrap()..]),
            None => format!("{line}\n"),
        })
        .collect();
    assert_eq!(without_ids, DRAFT);
}

#[test]
fn edits_reprint_only_what_changed() {
    let (sources, lowered) = stored();
    let mut next = lowered.model.clone();
    next.upstreams[0].note = Some("primary".into());
    next.sites.retain(|site| site.name != "old");
    let api = Upstream {
        id: Uuid::now_v7(),
        name: "api".into(),
        nodes: Vec::new(),
        ..next.upstreams[0].clone()
    };
    next.upstreams.push(api.clone());
    let edited = reconcile(&sources, &lowered, &next);
    let text = edited.get("main.conf").unwrap();
    let before = sources.get("main.conf").unwrap();

    let listener =
        "    # Plain HTTP for everyone\n    listener http {\n        address 0.0.0.0:80;\n    }\n";
    assert!(
        before.contains(listener) && text.contains(listener),
        "{text}"
    );
    assert!(
        text.contains("    # The application servers\n    upstream app {\n"),
        "{text}"
    );
    assert!(text.contains("        note primary;\n"), "{text}");
    assert!(
        !text.contains("Retired") && !text.contains("server old"),
        "{text}"
    );
    assert!(!text.contains("\n\n\n"), "{text}");
    let appended = format!(
        "\n    upstream api {{\n        id {};\n        note primary;\n    }}\n}}\n",
        api.id
    );
    assert!(text.ends_with(&appended), "{text}");

    let again = read(&edited);
    assert!(again.is_valid(), "{:#?}\n{text}", again.diagnostics);
    assert_eq!(again.model.upstreams.len(), 2);
    assert_eq!(again.model.upstreams[0].note.as_deref(), Some("primary"));
    assert_eq!(again.model.sites.len(), 1);
    assert!(matches!(again.model.sites[0].action, Action::Proxy { .. }));

    let changes = plan(&lowered.model, &again.model);
    let kinds: Vec<_> = changes
        .iter()
        .map(|change| (change.resource.split('/').next().unwrap(), change.change))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("sites", Change::Removed),
            ("upstreams", Change::Changed),
            ("upstreams", Change::Added)
        ]
    );
    assert!(
        changes[1].diff.contains("+    note primary;"),
        "{}",
        changes[1].diff
    );

    let files = diff_files(&sources, &edited);
    assert_eq!(files.len(), 1);
    assert!(
        files[0]
            .diff
            .starts_with("--- a/main.conf\n+++ b/main.conf\n"),
        "{}",
        files[0].diff
    );
    let mut more = edited.clone();
    more.insert("sites/new.conf", "server n {}\n");
    assert!(diff_files(&edited, &more)[0]
        .diff
        .starts_with("--- /dev/null\n+++ b/sites/new.conf\n"));
    assert!(diff_files(&more, &edited)[0]
        .diff
        .starts_with("--- a/sites/new.conf\n+++ /dev/null\n"));
}

#[test]
fn renaming_an_upstream_reprints_what_refers_to_it() {
    let (sources, lowered) = stored();
    let mut next = lowered.model.clone();
    next.upstreams[0].name = "backend".into();
    let edited = reconcile(&sources, &lowered, &next);
    let text = edited.get("main.conf").unwrap();
    assert!(
        text.contains("upstream backend {") && text.contains("proxy backend;"),
        "{text}"
    );
    assert!(
        text.contains("    # Retired, kept for reference\n    server old {\n        id "),
        "{text}"
    );
    assert!(read(&edited).is_valid());
}

#[test]
fn http_policies_join_and_leave_the_text() {
    let (sources, lowered) = stored();
    let mut next = lowered.model.clone();
    next.put_http_policy(panel_config_model::HttpPolicy {
        id: "headers".into(),
        response: panel_config_model::FieldChanges {
            set: vec![panel_ir::HeaderField {
                name: "X-Frame-Options".into(),
                value: "DENY".into(),
            }],
            ..Default::default()
        },
        ..Default::default()
    });
    let shop = next
        .sites
        .iter_mut()
        .find(|site| site.name == "shop")
        .unwrap();
    shop.http_policy_id = Some("headers".into());
    let edited = reconcile(&sources, &lowered, &next);
    let text = edited.get("main.conf").unwrap();
    assert!(
        text.contains(
            "    http_policy headers {\n        response_header set X-Frame-Options DENY;\n    }\n"
        ),
        "{text}"
    );
    assert!(text.contains("        http_policy headers;\n"), "{text}");
    let again = read(&edited);
    assert!(again.is_valid(), "{:#?}\n{text}", again.diagnostics);
    assert_eq!(again.model.http_policies, next.http_policies);
    assert!(
        plan(&lowered.model, &again.model)
            .iter()
            .any(|change| change.resource == "http-policies/headers"
                && change.change == Change::Added)
    );

    let mut back = again.model.clone();
    back.sites
        .iter_mut()
        .for_each(|site| site.http_policy_id = None);
    back.delete_http_policy("headers").unwrap();
    let removed = reconcile(&edited, &again, &back);
    let text = removed.get("main.conf").unwrap();
    assert!(!text.contains("http_policy"), "{text}");
    assert!(read(&removed).is_valid());
}

#[test]
fn security_policies_join_and_leave_the_text() {
    let (sources, lowered) = stored();
    let mut next = lowered.model.clone();
    next.put_security_policy(panel_config_model::SecurityPolicy {
        id: "office".into(),
        allowed_cidrs: vec!["10.0.0.0/8".into()],
        ..Default::default()
    });
    let shop = next
        .sites
        .iter_mut()
        .find(|site| site.name == "shop")
        .unwrap();
    shop.security_policy_id = Some("office".into());
    let edited = reconcile(&sources, &lowered, &next);
    let text = edited.get("main.conf").unwrap();
    assert!(
        text.contains("    security_policy office {\n        allow 10.0.0.0/8;\n    }\n"),
        "{text}"
    );
    assert!(text.contains("        security_policy office;\n"), "{text}");
    assert!(text.contains("   # keep me\n"), "{text}");
    let again = read(&edited);
    assert!(again.is_valid(), "{:#?}\n{text}", again.diagnostics);
    assert_eq!(again.model.security_policies, next.security_policies);
    let kinds: Vec<_> = plan(&lowered.model, &again.model)
        .iter()
        .map(|change| {
            (
                change.resource.split('/').next().unwrap().to_owned(),
                change.change,
            )
        })
        .collect();
    assert!(
        kinds.contains(&("security-policies".to_owned(), Change::Added)),
        "{kinds:?}"
    );

    let mut back = again.model.clone();
    back.sites
        .iter_mut()
        .for_each(|site| site.security_policy_id = None);
    back.delete_security_policy("office").unwrap();
    let removed = reconcile(&edited, &again, &back);
    let text = removed.get("main.conf").unwrap();
    assert!(!text.contains("security_policy"), "{text}");
    assert!(read(&removed).is_valid());
}

#[test]
fn an_empty_configuration_gains_an_http_block() {
    let empty = Sources::single("");
    let lowered = read(&empty);
    let mut next = lowered.model.clone();
    next.upstreams.push(Upstream {
        id: Uuid::now_v7(),
        name: "app".into(),
        nodes: Vec::new(),
        balancing: panel_ir::LoadBalancingPolicy::RoundRobin,
        host_header: None,
        tls: Default::default(),
        connection: Default::default(),
        health_check: None,
        passive_health: None,
        note: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    });
    let edited = reconcile(&empty, &lowered, &next);
    let text = edited.get("main.conf").unwrap();
    assert!(
        text.starts_with("language_version 1;\n\nhttp {\n    upstream app {\n"),
        "{text}"
    );
    assert!(read(&edited).is_valid());
}
