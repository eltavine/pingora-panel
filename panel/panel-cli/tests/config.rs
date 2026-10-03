#![forbid(unsafe_code)]

//! `ppanel config` and `ppanel revision` against a stand-in API that records
//! what the command line sends.

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
        ("POST", "/api/v1/config/format") => {
            Json(json!({"files": {"main.conf": "language_version 1;\n"}, "diagnostics": []}))
                .into_response()
        }
        ("GET", "/api/v1/config/plan") | ("GET", "/api/v1/revisions/3/diff") => {
            Json(changes).into_response()
        }
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
