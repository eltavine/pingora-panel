#![forbid(unsafe_code)]

//! `ppanel config`, `ppanel revision` and `ppanel audit` against a stand-in
//! API that records what the command line sends.

use axum::{
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    process::Output,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug)]
struct Request {
    method: Method,
    path: String,
    query: String,
    if_match: Option<String>,
    body: Value,
}

type Log = Arc<Mutex<Vec<Request>>>;

const MAIN: &str = "language_version 1;\n\nhttp {\n    include sites/*.conf;\n}\n";
const SHOP: &str = "server shop {\n    server_name shop.example;\n}\n";

fn with_etag(etag: &str, body: Value) -> Response {
    ([("etag", format!("\"{etag}\""))], Json(body)).into_response()
}

async fn api(
    State(log): State<Log>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    log.lock().unwrap().push(Request {
        method: method.clone(),
        path: uri.path().to_owned(),
        query: uri.query().unwrap_or_default().to_owned(),
        if_match: headers
            .get("if-match")
            .map(|value| value.to_str().unwrap().to_owned()),
        body: body.clone(),
    });
    let revision = json!({
        "id": 3, "draft_version": 2, "language_version": 1, "content_hash": "sha256:aa",
        "author": "ops", "note": null, "created_at": "2026-10-01T10:00:00Z",
        "outcome": "superseded", "outcome_at": "2026-10-02T10:00:00Z", "diagnostics": [],
        "snapshot_hash": "sha256:bb", "gateway_revision": 9
    });
    let changes = json!({
        "resources": [{"resource": "sites/shop", "change": "changed", "diff": "-a\n+b\n"}],
        "files": [{"path": "sites/shop.conf", "change": "changed",
                   "diff": "--- a/sites/shop.conf\n+++ b/sites/shop.conf\n-a\n+b\n"}]
    });
    match (method.as_str(), uri.path()) {
        ("GET", "/api/v1/config/source") => with_etag(
            "draft-4",
            json!({"language_version": 1, "files": {"main.conf": MAIN, "sites/shop.conf": SHOP}, "diagnostics": []}),
        ),
        ("PUT", "/api/v1/config/source") => with_etag(
            "draft-5",
            json!({"language_version": 1, "files": body["files"], "diagnostics": [{
                "code": "DSL_DEPRECATED", "severity": "WARNING", "source_span": "main.conf:2.5-13",
                "message": "`location` is deprecated; use `route`"
            }]}),
        ),
        ("POST", "/api/v1/config/check") => {
            if body.to_string().contains("nowhere") {
                Json(json!({"valid": false, "diagnostics": [{
                    "code": "DSL_REFERENCE", "severity": "ERROR", "source_span": "main.conf:3.22-28",
                    "message": "no upstream is named \"nowhere\""
                }]}))
                .into_response()
            } else {
                Json(json!({"valid": true, "diagnostics": []})).into_response()
            }
        }
        ("POST", "/api/v1/config/ast") => Json(json!({
            "file": body["file"],
            "directives": [
                {"name": "language_version", "args": ["1"], "span": "main.conf:1.1-19"},
                {"name": "http", "args": [], "span": "main.conf:3.1-5.1", "comments": ["Edge"],
                 "block": [{"name": "include", "args": ["sites/*.conf"], "span": "main.conf:4.5-25"}]}
            ],
            "diagnostics": []
        }))
        .into_response(),
        ("POST", "/api/v1/config/explain") => Json(json!({
            "block": "route", "name": "api", "resource": "sites/s/routes/r",
            "source_span": "sites/shop.conf:4.5-7.5",
            "settings": [
                {"name": "match", "value": "prefix /api", "source": "here",
                 "source_span": "sites/shop.conf:5.9-26"},
                {"name": "https_redirect", "value": "on", "source": "inherited",
                 "from": "server shop", "source_span": "sites/shop.conf:2.5-22",
                 "rule": "Off when not written; applies to every host and route of the server."},
                {"name": "tls_profile", "scope": "shop.example", "value": "edge",
                 "source": "inherited", "from": "listener secure",
                 "source_span": "main.conf:9.9-25", "rule": "A host uses its domain's tls_profile=."},
                {"name": "priority", "value": "10", "source": "default",
                 "rule": "Without it, routes take 10, 20, 30 and so on in the order they are written."}
            ]
        }))
        .into_response(),
        ("GET", "/api/v1/config/ir") => {
            Json(json!({"schema_version": "panel.ir.v1", "listeners": [], "sites": []})).into_response()
        }
        ("POST", "/api/v1/config/format") => {
            Json(json!({"files": {"main.conf": "language_version 1;\n"}, "diagnostics": []}))
                .into_response()
        }
        ("GET", "/api/v1/config/plan") | ("GET", "/api/v1/revisions/3/diff") => {
            Json(changes).into_response()
        }
        ("POST", "/api/v1/config/import/nginx") => Json(json!({
            "files": {"main.conf": MAIN},
            "report": [{
                "code": "NGINX_UNSUPPORTED", "severity": "WARNING",
                "source_span": "nginx.conf:1.1-9", "message": "'events' is not carried over"
            }],
            "valid": true,
            "diagnostics": []
        }))
        .into_response(),
        ("GET", "/api/v1/audit-events") => Json(json!({
            "items": [{
                "sequence": 2, "occurred_at": "2026-10-03T10:00:00.000001Z", "actor_id": "ops",
                "event_type": "config.draft.applied", "subject": "configuration/draft",
                "correlation_id": "req-1"
            }],
            "next_before": null
        }))
        .into_response(),
        ("GET", "/api/v1/audit-events/verify") => {
            if uri.query().unwrap_or_default().contains("from=9") {
                Json(json!({"intact": false, "checked": 0, "first_mismatch": 9,
                            "head_sequence": 12, "head_hash": "ab"}))
                .into_response()
            } else {
                Json(json!({"intact": true, "checked": 12, "first_mismatch": null,
                            "head_sequence": 12, "head_hash": "ab"}))
                .into_response()
            }
        }
        ("GET", "/api/v1/audit-events/2") => Json(json!({
            "sequence": 2, "event_type": "config.draft.applied", "event_version": 1,
            "actor_id": "ops", "actor_type": "user", "data": {"revision": 1}
        }))
        .into_response(),
        ("GET", "/api/v1/config/draft") => {
            Json(json!({"version": 6, "pending": true})).into_response()
        }
        ("POST", "/api/v1/config/dry-run") => {
            Json(json!({"draft": {"version": 6}, "diagnostics": []})).into_response()
        }
        ("POST", "/api/v1/config/apply") => Json(json!({
            "draft": {"version": 6}, "revision": 7, "revision_id": 12, "content_hash": "sha256:cc"
        }))
        .into_response(),
        ("POST", "/api/v1/revisions/3/restore") => with_etag(
            "draft-6",
            json!({"language_version": 1, "files": {"main.conf": MAIN}, "diagnostics": []}),
        ),
        ("GET", "/api/v1/revisions") => {
            Json(json!({"items": [revision], "next_before": null})).into_response()
        }
        ("GET", "/api/v1/revisions/3") => Json(json!({
            "revision": revision, "files": {"main.conf": MAIN, "sites/shop.conf": SHOP}
        }))
        .into_response(),
        ("PUT", "/api/v1/revisions/3/note") => {
            let mut noted = revision;
            noted["note"] = body["note"].clone();
            Json(noted).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

struct Stub {
    base: String,
    log: Log,
}

impl Stub {
    fn start() -> Self {
        let log = Log::default();
        let router = Router::new()
            .route("/{*path}", any(api))
            .with_state(log.clone());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                    axum::serve(listener, router).await.unwrap();
                });
        });
        Self {
            base: format!("http://{address}"),
            log,
        }
    }

    fn ppanel(&self, arguments: &[&str]) -> Output {
        std::process::Command::new(env!("CARGO_BIN_EXE_ppanel"))
            .args(["--api", &self.base, "--actor", "ops"])
            .args(arguments)
            .output()
            .unwrap()
    }

    fn requests(&self, method: &str, path: &str) -> Vec<Request> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.method == method && request.path == path)
            .cloned()
            .collect()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn configuration_files_round_trip_through_a_directory() {
    let stub = Stub::start();
    let directory = tempfile::tempdir().unwrap();
    let conf = directory.path().join("conf");

    let exported = stub.ppanel(&["config", "export", "--dir", conf.to_str().unwrap()]);
    assert!(exported.status.success(), "{}", stderr(&exported));
    assert_eq!(
        std::fs::read_to_string(conf.join("main.conf")).unwrap(),
        MAIN
    );
    assert_eq!(
        std::fs::read_to_string(conf.join("sites").join("shop.conf")).unwrap(),
        SHOP
    );
    let printed = stub.ppanel(&["config", "export"]);
    assert_eq!(printed.status.code(), Some(2), "two files need --dir");

    let imported = stub.ppanel(&[
        "config",
        "import",
        conf.to_str().unwrap(),
        "--expected-version",
        "4",
    ]);
    assert!(imported.status.success(), "{}", stderr(&imported));
    assert_eq!(stdout(&imported).trim(), "Saved 2 files as draft 5");
    assert!(stderr(&imported).contains(
        "main.conf:2.5-13: warning: `location` is deprecated; use `route` [DSL_DEPRECATED]"
    ));
    let saved = stub.requests("PUT", "/api/v1/config/source");
    assert_eq!(saved[0].if_match.as_deref(), Some("\"draft-4\""));
    assert_eq!(
        saved[0].body,
        json!({"files": {"main.conf": MAIN, "sites/shop.conf": SHOP}})
    );
}

#[test]
fn checks_and_formatting_report_positions_and_exit_codes() {
    let stub = Stub::start();
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("edge.conf");
    std::fs::write(
        &file,
        "language_version  1 ;\nhttp {\n    server s { proxy nowhere; }\n}\n",
    )
    .unwrap();
    let path = file.to_str().unwrap();

    let checked = stub.ppanel(&["config", "check", path]);
    assert_eq!(checked.status.code(), Some(5));
    assert!(stderr(&checked)
        .contains("main.conf:3.22-28: error: no upstream is named \"nowhere\" [DSL_REFERENCE]"));
    let draft = stub.ppanel(&["config", "check"]);
    assert!(draft.status.success(), "{}", stderr(&draft));
    assert_eq!(
        stub.requests("POST", "/api/v1/config/check")[1].body["files"]["sites/shop.conf"],
        SHOP
    );

    let unformatted = stub.ppanel(&["config", "fmt", path, "--check"]);
    assert_eq!(unformatted.status.code(), Some(5));
    assert_eq!(stdout(&unformatted).trim(), "main.conf");
    let printed = stub.ppanel(&["config", "fmt", path]);
    assert_eq!(stdout(&printed), "language_version 1;\n");
    let written = stub.ppanel(&["config", "fmt", path, "--write"]);
    assert!(written.status.success(), "{}", stderr(&written));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "language_version 1;\n"
    );
}

#[test]
fn syntax_trees_and_snapshots_are_printed() {
    let stub = Stub::start();
    let tree = stub.ppanel(&["config", "ast", "--file", "main.conf"]);
    assert!(tree.status.success(), "{}", stderr(&tree));
    assert_eq!(
        stdout(&tree),
        "language_version 1  [main.conf:1.1-19]\n# Edge\nhttp  [main.conf:3.1-5.1]\n    include sites/*.conf  [main.conf:4.5-25]\n"
    );
    assert_eq!(
        stub.requests("POST", "/api/v1/config/ast")[0].body["files"]["main.conf"],
        MAIN
    );
    let snapshot = stub.ppanel(&["config", "ir"]);
    let snapshot: Value = serde_json::from_slice(&snapshot.stdout).unwrap();
    assert_eq!(snapshot["schema_version"], "panel.ir.v1");
}

#[test]
fn explain_shows_where_each_value_comes_from() {
    let stub = Stub::start();
    let explained = stub.ppanel(&["config", "explain", "sites/shop.conf:5.12"]);
    assert!(explained.status.success(), "{}", stderr(&explained));
    let request = &stub.requests("POST", "/api/v1/config/explain")[0].body;
    assert_eq!(
        (&request["file"], &request["line"], &request["column"]),
        (&json!("sites/shop.conf"), &json!(5), &json!(12))
    );
    assert_eq!(request["files"]["sites/shop.conf"], SHOP);
    let printed = stdout(&explained);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines[0], "route api at sites/shop.conf:4.5-7.5");
    let row = |name: &str| {
        lines
            .iter()
            .find(|line| line.starts_with(name))
            .map(|line| line.split_whitespace().collect::<Vec<_>>())
            .unwrap_or_else(|| panic!("no {name} in {printed}"))
    };
    assert_eq!(
        row("match"),
        [
            "match",
            "-",
            "prefix",
            "/api",
            "here",
            "sites/shop.conf:5.9-26"
        ]
    );
    assert_eq!(
        row("https_redirect"),
        [
            "https_redirect",
            "-",
            "on",
            "server",
            "shop",
            "sites/shop.conf:2.5-22"
        ]
    );
    assert_eq!(
        row("tls_profile"),
        [
            "tls_profile",
            "shop.example",
            "edge",
            "listener",
            "secure",
            "main.conf:9.9-25"
        ]
    );
    assert_eq!(row("priority"), ["priority", "-", "10", "default", "-"]);
    assert!(printed.contains(
        "\npriority: Without it, routes take 10, 20, 30 and so on in the order they are written.\n"
    ));
    assert!(!printed.contains("match:"));

    let invalid = stub.ppanel(&["config", "explain", "shop.conf"]);
    assert_eq!(invalid.status.code(), Some(2), "{}", stderr(&invalid));
}

#[test]
fn nginx_configuration_is_converted_with_its_report() {
    let stub = Stub::start();
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("conf.d")).unwrap();
    std::fs::write(
        directory.path().join("nginx.conf"),
        "events {}\nhttp { include conf.d/*.conf; }\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("conf.d/site.conf"),
        "server { return 204; }\n",
    )
    .unwrap();
    let entry = directory.path().join("nginx.conf");

    let converted = stub.ppanel(&["config", "import-nginx", entry.to_str().unwrap()]);
    assert!(converted.status.success(), "{}", stderr(&converted));
    assert_eq!(stdout(&converted), MAIN);
    assert!(stderr(&converted)
        .contains("nginx.conf:1.1-9: warning: 'events' is not carried over [NGINX_UNSUPPORTED]"));
    let request = &stub.requests("POST", "/api/v1/config/import/nginx")[0].body;
    assert_eq!(request["entry"], "nginx.conf");
    assert_eq!(
        request["files"]["conf.d/site.conf"],
        "server { return 204; }\n"
    );

    let saved = stub.ppanel(&[
        "config",
        "import-nginx",
        directory.path().to_str().unwrap(),
        "--save",
        "--expected-version",
        "4",
    ]);
    assert!(saved.status.success(), "{}", stderr(&saved));
    let put = &stub.requests("PUT", "/api/v1/config/source")[0];
    assert_eq!(put.if_match.as_deref(), Some("\"draft-4\""));
    assert_eq!(put.body["files"]["main.conf"], MAIN);
}

#[test]
fn audit_events_are_listed_shown_and_verified() {
    let stub = Stub::start();
    let listed = stub.ppanel(&[
        "audit",
        "list",
        "--type",
        "config.",
        "--correlation-id",
        "req-1",
        "--limit",
        "5",
    ]);
    assert!(
        stdout(&listed).contains("config.draft.applied"),
        "{}",
        stdout(&listed)
    );
    assert_eq!(
        stub.requests("GET", "/api/v1/audit-events")[0].query,
        "limit=5&type=config.&correlation_id=req-1"
    );
    let shown = stub.ppanel(&["audit", "show", "2"]);
    assert!(stdout(&shown).contains("ops (user)"));
    assert!(stdout(&shown).contains("\"revision\": 1"));
    let verified = stub.ppanel(&["audit", "verify"]);
    assert!(stdout(&verified).starts_with("The audit trail is intact: 12 events checked"));
    let tampered = stub.ppanel(&["audit", "verify", "--from", "9"]);
    assert_eq!(tampered.status.code(), Some(1));
    assert!(stderr(&tampered).contains("event 9 does not match its hash"));
}

#[test]
fn applying_rolling_back_and_revisions() {
    let stub = Stub::start();

    let planned = stub.ppanel(&["config", "plan"]);
    assert!(stdout(&planned).contains("changed  sites/shop"));
    assert!(stdout(&planned).contains("+++ b/sites/shop.conf"));
    let dry = stub.ppanel(&["config", "apply", "--dry-run"]);
    assert!(stdout(&dry).contains("Version 6 passed every check"));
    assert!(stub.requests("POST", "/api/v1/config/apply").is_empty());

    let applied = stub.ppanel(&["config", "apply", "--note", "launch"]);
    assert!(
        stdout(&applied).contains("as revision 7"),
        "{}",
        stdout(&applied)
    );
    assert_eq!(
        stub.requests("POST", "/api/v1/config/apply")[0].body["note"],
        "launch"
    );

    let rolled = stub.ppanel(&["config", "rollback", "--to", "3", "--reason", "errors"]);
    assert!(rolled.status.success(), "{}", stderr(&rolled));
    assert_eq!(
        stub.requests("POST", "/api/v1/revisions/3/restore").len(),
        1
    );
    assert_eq!(
        stub.requests("POST", "/api/v1/config/apply")[1].body,
        json!({"expected_version": 6, "note": "errors"})
    );

    let listed = stub.ppanel(&["revision", "list", "--before", "9"]);
    assert!(stdout(&listed)
        .lines()
        .nth(1)
        .unwrap()
        .starts_with("3   superseded"));
    let shown = stub.ppanel(&["revision", "show", "3"]);
    assert!(stdout(&shown).contains("main.conf, sites/shop.conf"));
    let file = stub.ppanel(&["revision", "show", "3", "--file", "sites/shop.conf"]);
    assert_eq!(stdout(&file), SHOP);
    let json = stub.ppanel(&["-o", "json", "revision", "diff", "3", "--against", "active"]);
    let changes: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(changes["resources"][0]["resource"], "sites/shop");
    let noted = stub.ppanel(&["revision", "note", "3", "first launch"]);
    assert_eq!(stdout(&noted).trim(), "Noted revision 3");
    let missing = stub.ppanel(&["revision", "show", "8"]);
    assert_eq!(missing.status.code(), Some(3));
}
