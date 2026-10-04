#![forbid(unsafe_code)]

//! `ppanel` against a stand-in API that records what the command line
//! sends.

use axum::{
    extract::{
        ws::{self, WebSocketUpgrade},
        State,
    },
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Output, Stdio},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug)]
struct Request {
    method: Method,
    path: String,
    query: String,
    if_match: Option<String>,
    authorization: Option<String>,
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
        authorization: headers
            .get("authorization")
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
        ("GET", "/api/v1/traffic") => Json(json!({
            "observed_at": "2026-10-04T10:00:00Z", "window_seconds": 900,
            "requests": 119.6, "requests_per_second": 0.13,
            "statuses": {"informational": 0, "success": 110, "redirection": 0,
                         "client_error": 0, "server_error": 9.6},
            "latency": {"p50": 0.012, "p90": null, "p95": 0.25, "p99": null},
            "bytes_received": 2048, "bytes_sent": 1_572_864,
            "open_connections": 3, "tls_handshakes": 7,
            "upstreams": [{"upstream": "app", "requests": 50, "error_ratio": 0.125,
                           "connection_reuse_ratio": 0.875,
                           "latency": {"p50": null, "p90": null, "p95": 1.5, "p99": null}}],
            "routes": [{"site": "shop", "route": "checkout", "requests": 60}],
            "domains": [{"site": "shop", "domain": "*.shop.example", "requests": 45}],
            "upstream_failures": [{"upstream": "app", "address": "10.0.0.7", "port": 8080,
                                   "error_type": "connect_refused", "failures": 4}],
            "revision": 7, "activated_at": "2026-10-04T09:00:00Z"
        }))
        .into_response(),
        ("GET", "/api/v1/traffic/series") => Json(json!({
            "points": [{"at": "2026-10-04T09:59:00Z", "requests_per_second": 2,
                        "server_errors_per_second": 0.1, "p95": null}]
        }))
        .into_response(),
        ("GET", "/api/v1/logs") => Json(json!({
            "records": [{
                "time": "2026-10-04T10:00:00.5Z", "kind": "access", "line": "{}",
                "site": "shop", "route": "checkout", "status": 502, "method": "GET",
                "path": "/cart", "client": "192.0.2.1", "request_id": "req-9", "fields": {}
            }],
            "next_until": "2026-10-04T10:00:00.5Z"
        }))
        .into_response(),
        ("GET", "/api/v1/logs/download") => (
            [("content-type", "text/plain; charset=utf-8")],
            "newest\noldest\n",
        )
            .into_response(),
        ("POST", "/api/v1/logs/deletions") => (
            StatusCode::ACCEPTED,
            Json(json!({
                "site": body["site"], "since": "1970-01-01T00:00:00Z",
                "until": "2026-10-04T10:00:00Z", "requested_at": "2026-10-04T10:00:00Z",
                "state": "pending"
            })),
        )
            .into_response(),
        ("GET", "/api/v1/host") => Json(json!({
            "reporting": true, "hostname": "web-1", "operating_system": "Ubuntu 24.04.2 LTS",
            "kernel_release": "6.8.0", "architecture": "x86_64",
            "host_time": "2026-10-04T10:00:00Z", "time_zone": "UTC", "uptime_seconds": 90_000,
            "cpu_count": 4, "cpu_usage": 0.25, "load1": 0.5, "load5": 0.75, "load15": 1.0,
            "memory_total_bytes": 8_589_934_592_u64, "memory_available_bytes": 2_147_483_648_u64,
            "filesystems": [{"mountpoint": "/var", "device": "/dev/sdb1", "fstype": "ext4",
                             "size_bytes": 1_073_741_824, "available_bytes": 53_687_091,
                             "used_ratio": 0.95, "level": "critical"}],
            "network_devices": [{"device": "eth0", "receive_bytes_per_second": 2048,
                                 "transmit_bytes_per_second": 512}]
        }))
        .into_response(),
        ("GET", "/api/v1/host/agent") => Json(json!({
            "status": "connected", "build": "0.1.0", "hostname": "web-1",
            "capabilities": [
                {"capability": "directories", "state": "available", "detail": ""},
                {"capability": "listeners", "state": "denied",
                 "detail": "grant CAP_DAC_READ_SEARCH"}
            ]
        }))
        .into_response(),
        ("GET", "/api/v1/host/directories") => Json(json!({
            "observed_at": "2026-10-04T10:00:00Z",
            "directories": [
                {"kind": "configuration", "path": "/var/lib/pingora-panel", "present": true,
                 "bytes": 1_048_576, "files": 12, "unreadable": 0, "truncated": false},
                {"kind": "logs", "path": "/var/log/pingora-panel", "present": true,
                 "bytes": 2048, "files": 3, "unreadable": 3, "truncated": true},
                {"kind": "certificates", "path": "/etc/pingora-panel/certificates",
                 "present": false, "bytes": 0, "files": 0, "unreadable": 0, "truncated": false}
            ]
        }))
        .into_response(),
        ("GET", "/api/v1/alert-rules") => Json(json!([{
            "id": "shop-errors", "version": 3, "etag": "\"3\"", "state": "firing",
            "since": "2026-10-04T09:58:00Z", "value": 0.12,
            "spec": {"name": "Shop errors", "measure": "server_error_ratio",
                     "comparison": "above", "threshold": 0.05, "pending_seconds": 300,
                     "site": "shop", "route": null, "upstream": null, "severity": "critical",
                     "enabled": true, "channels": ["ops"]}
        }]))
        .into_response(),
        ("PUT", path) if path.starts_with("/api/v1/alert-rules/") => (
            StatusCode::OK,
            Json(json!({"id": path.trim_start_matches("/api/v1/alert-rules/"), "spec": body})),
        )
            .into_response(),
        ("DELETE", "/api/v1/alert-rules/shop-errors")
        | ("DELETE", "/api/v1/alert-channels/ops") => StatusCode::NO_CONTENT.into_response(),
        ("GET", "/api/v1/alert-channels") => Json(json!([{
            "id": "ops", "kind": "webhook", "target": "https://hooks.example", "version": 1,
            "etag": "\"1\""
        }]))
        .into_response(),
        ("POST", "/api/v1/alert-channels") => (
            StatusCode::CREATED,
            Json(json!({
                "channel": {"id": "ops", "kind": "webhook", "target": "https://hooks.example"},
                "secret": "whsec_c2VjcmV0"
            })),
        )
            .into_response(),
        ("POST", "/api/v1/alert-channels/ops/rotate") => Json(json!({
            "channel": {"id": "ops", "kind": "webhook", "target": "https://other.example"},
            "secret": "whsec_bmV3"
        }))
        .into_response(),
        ("POST", "/api/v1/alert-channels/ops/test") => Json(json!({
            "delivered": false, "status": 503, "failure": "the receiver answered 503"
        }))
        .into_response(),
        ("GET", "/api/v1/alert-notifications") => Json(json!([{
            "id": "0192", "rule": "shop-errors", "channel": "ops", "kind": "firing",
            "state": "abandoned", "attempts": 2, "created_at": "2026-10-04T09:58:00Z",
            "last_failure": "the receiver answered 410"
        }]))
        .into_response(),
        ("GET", "/api/v1/logs/deletions") => Json(json!({
            "deletions": [{
                "site": null, "since": "1970-01-01T00:00:00Z", "until": "2026-10-03T10:00:00Z",
                "requested_at": "2026-10-03T10:00:00Z", "state": "applied"
            }]
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
        ("POST", "/api/v1/config/apply") if body["note"] == "covered" => {
            (StatusCode::ACCEPTED, Json(approval_request("pending"))).into_response()
        }
        ("POST", "/api/v1/config/apply") => Json(json!({
            "draft": {"version": 6}, "revision": 7, "revision_id": 12, "content_hash": "sha256:cc"
        }))
        .into_response(),
        ("GET", "/api/v1/approvals") => Json(json!({
            "items": [approval_request("pending")], "next_before": null
        }))
        .into_response(),
        ("POST", "/api/v1/approvals/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a77/approve") => {
            Json(approval_request("approved")).into_response()
        }
        ("PUT", "/api/v1/approval-policies/prod") => {
            let mut saved = body.clone();
            saved["id"] = json!("prod");
            saved["version"] = json!(2);
            Json(saved).into_response()
        }
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
        ("POST", "/api/v1/session") => (
            StatusCode::CREATED,
            Json(json!({
                "account": {"username": body["username"], "roles": ["operator"]},
                "permissions": ["config.read"], "credential": "bearer",
                "session": {"expires_at": "2026-10-04T08:00:00Z"},
                "secret": "session-secret"
            })),
        )
            .into_response(),
        ("GET", "/api/v1/session") => {
            if headers.contains_key("authorization") {
                Json(json!({
                    "account": {"username": "ops", "roles": ["operator"]},
                    "permissions": ["config.read", "config.write"], "credential": "bearer",
                    "session": {"expires_at": "2026-10-04T08:00:00Z"}
                }))
                .into_response()
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"status": 401, "detail": "log in or present an API token"})),
                )
                    .into_response()
            }
        }
        ("DELETE", "/api/v1/session") => StatusCode::NO_CONTENT.into_response(),
        ("DELETE", "/api/v1/account/sessions") => Json(json!({"ended": 2})).into_response(),
        ("POST", "/api/v1/roles") => (
            StatusCode::CREATED,
            Json(json!({"id": body["id"], "name": body["name"], "permissions": body["permissions"],
                        "description": "", "built_in": false})),
        )
            .into_response(),
        ("POST", "/api/v1/account/tokens/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c/rotate") => (
            StatusCode::CREATED,
            Json(json!({"token": {"name": "ci", "expires_at": "2026-11-02T08:00:00Z"},
                        "secret": "ppat_rotated"})),
        )
            .into_response(),
        ("POST", "/api/v1/account/tokens") => (
            StatusCode::CREATED,
            Json(json!({
                "token": {"name": body["name"], "expires_at": "2026-11-02T08:00:00Z"},
                "secret": "ppat_new"
            })),
        )
            .into_response(),
        ("GET", "/api/v1/accounts") => Json(json!([
            {"id": "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b", "username": "ops", "roles": ["operator"],
             "disabled": false, "locked": false}
        ]))
        .into_response(),
        ("POST", "/api/v1/accounts") => (
            StatusCode::CREATED,
            Json(json!({"id": "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a99",
                        "username": body["username"], "service": body["service"]})),
        )
            .into_response(),
        ("POST", "/api/v1/accounts/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b/tokens") => (
            StatusCode::CREATED,
            Json(json!({"secret": "ppat_issued",
                        "token": {"name": body["name"], "expires_at": "2026-10-10T00:00:00Z"}})),
        )
            .into_response(),
        ("PUT", "/api/v1/workload-identities/shop") => Json(body.clone()).into_response(),
        ("POST", "/api/v1/accounts/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b/grants") => {
            let mut grant = body.clone();
            grant["id"] = json!("0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a88");
            grant["created_by"] = json!("root");
            (StatusCode::CREATED, Json(grant)).into_response()
        }
        ("GET", "/api/v1/accounts/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b/grants") => Json(json!([{
            "id": "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a88", "role": "operator",
            "scope": {"kind": "site_group", "group": "shop"},
            "conditions": {"networks": ["10.0.0.0/8"], "windows": []},
            "created_at": "2026-10-03T00:00:00Z", "created_by": "root"
        }]))
        .into_response(),
        ("POST", "/api/v1/auth/workload") => (
            StatusCode::CREATED,
            Json(json!({"secret": "session-secret", "expires_at": "2026-10-03T09:15:00Z",
                        "account": "deployer"})),
        )
            .into_response(),
        ("PATCH", "/api/v1/accounts/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b") => {
            Json(json!({"username": "ops", "disabled": body["disabled"]})).into_response()
        }
        ("PUT", "/api/v1/tls-profiles/edge") => Json(body.clone()).into_response(),
        ("PUT", "/api/v1/security-policies/office") => Json(body.clone()).into_response(),
        ("PUT", "/api/v1/identity-providers/corp") => Json(body.clone()).into_response(),
        ("PUT", "/api/v1/sign-in-policy") => Json(body.clone()).into_response(),
        ("GET", "/api/v1/sign-in-policy") => {
            Json(json!({"password_sign_in": "break_glass_only"})).into_response()
        }
        ("GET", "/api/v1/identity-providers") => Json(json!([
            {"id": "corp", "display_name": "Corporate", "issuer": "https://id.example",
             "client_id": "panel", "has_client_secret": true, "scopes": ["profile"],
             "claims": {"username": "preferred_username", "display_name": "name",
                        "email": "email", "groups": "groups"},
             "group_roles": [{"group": "ops", "role": "operator"}],
             "create_accounts": true, "enabled": true,
             "created_at": "2026-10-03T00:00:00.000Z", "updated_at": "2026-10-03T00:00:00.000Z"}
        ]))
        .into_response(),
        ("PUT", "/api/v1/listeners/edge") => Json(body.clone()).into_response(),
        ("GET", "/api/v1/gateway/file-checks") => Json(json!({
            "checked_at": "2026-10-03T00:00:00.000Z", "active_revision_id": 7,
            "private_keys": [{"file": "shop.key", "tls_profile_ids": ["shop"], "mode": "0644",
                              "owner_only": false, "error": null}],
            "static_roots": [{"id": "docs", "root": "docs", "inside": true,
                              "escaping_links": [{"path": "old", "target": "/etc"}],
                              "entries_checked": 3, "truncated": false, "error": null}]
        }))
        .into_response(),
        ("GET", "/api/v1/security-policies") => Json(json!([
            {"id": "office", "allowed_cidrs": ["10.0.0.0/8"], "basic_auth": {"realm": "Staff", "users_secret_id": "staff.htpasswd"},
             "rate_limits": [{"key": {"kind": "client_address"}, "requests": 10, "per_seconds": 1}],
             "used_by": ["0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"], "etag": "\"p1\""}
        ]))
        .into_response(),
        ("POST", "/api/v1/tls-checks") => Json(json!({
            "listener": body["listener"], "address": "127.0.0.1:8443", "host": body["host"],
            "protocol": "TLSv1.3", "cipher_suite": "TLS13_AES_256_GCM_SHA384", "alpn": "h2",
            "handshake_ms": 3,
            "versions": [{"version": "TLSv1.2", "accepted": false},
                         {"version": "TLSv1.3", "accepted": true}],
            "certificate": {"subject": "CN=shop.example", "names": ["shop.example"],
                            "not_after": "2027-01-01T00:00:00Z"},
            "certificate_status": "valid", "covers_host": true, "http_status": null,
            "strict_transport_security": "max-age=31536000"
        }))
        .into_response(),
        ("POST", "/api/v1/sites") => (
            StatusCode::CREATED,
            Json(json!({"id": "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a60", "name": body["name"]})),
        )
            .into_response(),
        ("GET", "/api/v1/certificates") => Json(json!([
            {"id": "example.com", "names": ["example.com", "*.example.com"], "status": "expiring",
             "not_after": "2026-10-20T00:00:00Z", "issuer": "CN=Example CA", "source": "uploaded"}
        ]))
        .into_response(),
        ("POST", "/api/v1/certificates") => (
            StatusCode::CREATED,
            [("etag", "\"1\"")],
            Json(json!({"id": body["id"], "source": body["source"], "status": "valid"})),
        )
            .into_response(),
        ("GET", "/api/v1/certificates/example.com") => with_etag(
            "3",
            json!({"id": "example.com", "source": "uploaded", "status": "valid",
                   "names": ["example.com"], "fingerprint": "0a1bff", "key_algorithm": "ecdsa_p256",
                   "key_bits": 256, "version": 3}),
        ),
        ("PUT", "/api/v1/certificates/example.com") => {
            with_etag("4", json!({"id": "example.com", "version": 4}))
        }
        ("DELETE", "/api/v1/certificates/example.com") => StatusCode::NO_CONTENT.into_response(),
        ("GET", "/api/v1/certificates/example.com/coverage") => Json(json!({
            "status": "valid", "not_after": "2027-01-01T00:00:00Z",
            "hosts": [{"host": "www.example.com", "covered": true},
                      {"host": "example.org", "covered": false}]
        }))
        .into_response(),
        ("GET", "/api/v1/acme-accounts") => Json(json!([
            {"id": "letsencrypt", "directory": "https://acme-v02.api.letsencrypt.org/directory",
             "contact": ["ops@example.com"], "url": "https://acme.example/acct/1"}
        ]))
        .into_response(),
        ("POST", "/api/v1/acme-accounts") => (
            StatusCode::CREATED,
            [("etag", "\"1\"")],
            Json(json!({"id": body["id"], "directory": body["directory"], "version": 1})),
        )
            .into_response(),
        ("GET", "/api/v1/acme-accounts/letsencrypt") => with_etag(
            "1",
            json!({"id": "letsencrypt", "directory": "https://acme-v02.api.letsencrypt.org/directory",
                   "contact": ["ops@example.com"], "ca_bundle": null, "version": 1}),
        ),
        ("DELETE", "/api/v1/acme-accounts/letsencrypt") => StatusCode::NO_CONTENT.into_response(),
        ("GET", "/api/v1/acme-certificates") => Json(json!([
            {"id": "example.com", "names": ["example.com", "www.example.com"], "state": "failing",
             "challenge": "http-01", "account": "letsencrypt",
             "renew_after": "2026-10-04T00:00:00Z", "failures": 2}
        ]))
        .into_response(),
        ("POST", "/api/v1/acme-certificates") => (
            StatusCode::CREATED,
            [("etag", "\"1\"")],
            Json(json!({"id": body["id"], "state": "pending"})),
        )
            .into_response(),
        ("GET", "/api/v1/acme-certificates/example.com") => with_etag(
            "4",
            json!({"id": "example.com", "state": "failing", "failures": 2,
                   "last_error": {"code": "VALIDATION_FAILED",
                                  "message": "the CA refused (connection): no answer",
                                  "at": "2026-10-03T00:00:00Z"}, "version": 4}),
        ),
        ("POST", "/api/v1/acme-certificates/example.com/renewals") => (
            StatusCode::ACCEPTED,
            Json(json!({"id": "example.com", "state": "failing"})),
        )
            .into_response(),
        ("DELETE", "/api/v1/acme-certificates/example.com") => {
            StatusCode::NO_CONTENT.into_response()
        }
        ("GET", "/api/v1/dns-providers") => Json(json!([
            {"id": "primary-ns", "kind": "rfc2136",
             "rfc2136": {"server": "ns1.example.com:53", "zones": ["example.com"],
                         "key_name": "acme-update", "algorithm": "hmac-sha256"}}
        ]))
        .into_response(),
        ("POST", "/api/v1/dns-providers") => (
            StatusCode::CREATED,
            [("etag", "\"1\"")],
            Json(json!({"id": body["id"], "kind": "rfc2136", "version": 1})),
        )
            .into_response(),
        ("GET", "/api/v1/dns-providers/primary-ns") => with_etag(
            "2",
            json!({"id": "primary-ns", "kind": "rfc2136", "propagation_seconds": 30,
                   "rfc2136": {"server": "ns1.example.com:53", "zones": ["example.com"],
                               "key_name": "acme-update", "algorithm": "hmac-sha256"},
                   "version": 2}),
        ),
        ("PUT", "/api/v1/dns-providers/primary-ns") => {
            with_etag("3", json!({"id": "primary-ns", "version": 3}))
        }
        ("DELETE", "/api/v1/dns-providers/primary-ns") => StatusCode::NO_CONTENT.into_response(),
        ("POST", "/api/v1/certificate-inspections") => Json(json!({
            "status": "valid", "key_matches": !body["key"].is_null(), "names": ["example.com"],
            "fingerprint": "0a1bff"
        }))
        .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Follows twice: the first tail falls behind and the second ends because
/// the log store is unavailable.
async fn tail(State(log): State<Log>, uri: Uri, upgrade: WebSocketUpgrade) -> Response {
    let follow = {
        let mut log = log.lock().unwrap();
        log.push(Request {
            method: Method::GET,
            path: uri.path().to_owned(),
            query: uri.query().unwrap_or_default().to_owned(),
            if_match: None,
            authorization: None,
            body: Value::Null,
        });
        log.iter()
            .filter(|request| request.path == uri.path())
            .count()
    };
    upgrade.on_upgrade(move |mut socket| async move {
        let (line, cursor, code, message) = if follow == 1 {
            (
                "first line",
                "2026-10-04T10:00:01Z",
                "RESOURCE_EXHAUSTED",
                "the tail fell behind",
            )
        } else {
            (
                "second line",
                "2026-10-04T10:00:02Z",
                "UNAVAILABLE",
                "the log store is unavailable",
            )
        };
        for message in [
            json!({"records": [{"line": format!("{line}\n")}], "cursor": cursor, "error": null}),
            json!({"records": [], "cursor": cursor, "error": {"code": code, "message": message}}),
        ] {
            let _ = socket
                .send(ws::Message::Text(message.to_string().into()))
                .await;
        }
        let _ = socket
            .send(ws::Message::Close(Some(ws::CloseFrame {
                code: 1013,
                reason: "the tail ended".into(),
            })))
            .await;
    })
}

struct Stub {
    base: String,
    log: Log,
    /// Where the command line keeps sessions, apart from the user's own.
    config: tempfile::TempDir,
}

impl Stub {
    fn start() -> Self {
        let log = Log::default();
        let router = Router::new()
            .route("/api/v1/logs/tail", get(tail))
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
            config: tempfile::tempdir().unwrap(),
        }
    }

    fn command(&self, arguments: &[&str]) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ppanel"));
        command
            .args(["--api", &self.base])
            .args(arguments)
            .env("PPANEL_CONFIG_DIR", self.config.path())
            .env_remove("PPANEL_TOKEN");
        command
    }

    fn ppanel(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().unwrap()
    }

    fn ppanel_with_input(&self, arguments: &[&str], input: &str) -> Output {
        let mut child = self
            .command(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
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
fn traffic_is_summarized_and_charted() {
    let stub = Stub::start();
    let summary = stub.ppanel(&[
        "traffic", "summary", "--site", "shop", "--route", "checkout", "--window", "15m",
    ]);
    let printed = stdout(&summary);
    for expected in [
        "15m",
        "120",
        "110 / 0 / 0 / 10",
        "12 ms / - / 250 ms / -",
        "2.0 KiB / 1.5 MiB",
        "7 (activated 2026-10-04T09:00:00Z)",
        "12.5%",
        "checkout",
        "*.shop.example",
        "10.0.0.7:8080",
        "connect_refused",
        "87.5%",
    ] {
        assert!(printed.contains(expected), "{expected} in\n{printed}");
    }
    assert_eq!(
        stub.requests("GET", "/api/v1/traffic")[0].query,
        "window=900&site=shop&route=checkout"
    );
    let series = stub.ppanel(&["traffic", "series", "--window", "1d", "--step", "5m"]);
    assert!(stdout(&series).contains("2026-10-04T09:59:00Z"));
    assert_eq!(
        stub.requests("GET", "/api/v1/traffic/series")[0].query,
        "window=86400&step=300"
    );
    let routeless = stub.ppanel(&["traffic", "summary", "--route", "checkout"]);
    assert_eq!(routeless.status.code(), Some(2));
}

#[test]
fn logs_are_searched_followed_downloaded_and_deleted() {
    let stub = Stub::start();
    let search = stub.ppanel(&[
        "logs",
        "search",
        "--site",
        "shop",
        "--status",
        "5xx",
        "--since",
        "2026-10-04T09:00:00Z",
        "--limit",
        "20",
    ]);
    assert!(search.status.success(), "{}", stderr(&search));
    let printed = stdout(&search);
    for expected in ["502", "/cart", "req-9", "checkout"] {
        assert!(printed.contains(expected), "{expected} in\n{printed}");
    }
    assert!(stderr(&search).contains("pass --until 2026-10-04T10:00:00.5Z"));
    assert_eq!(
        stub.requests("GET", "/api/v1/logs")[0].query,
        "site=shop&status=5xx&since=2026-10-04T09%3A00%3A00Z&limit=20"
    );

    let tail = stub.ppanel(&["logs", "tail", "--kind", "error"]);
    assert_eq!(stdout(&tail), "first line\nsecond line\n");
    assert_eq!(tail.status.code(), Some(6), "{}", stderr(&tail));
    let complaint = stderr(&tail);
    assert!(
        complaint.contains("the log store is unavailable"),
        "{complaint}"
    );
    assert!(
        complaint.contains("resume with --after 2026-10-04T10:00:02Z"),
        "{complaint}"
    );
    let tails = stub.requests("GET", "/api/v1/logs/tail");
    assert_eq!(tails[0].query, "kind=error");
    assert_eq!(tails[1].query, "kind=error&after=2026-10-04T10%3A00%3A01Z");

    let file = stub.config.path().join("gateway.log");
    let download = stub.ppanel(&[
        "logs",
        "download",
        "--site",
        "shop",
        "--file",
        file.to_str().unwrap(),
    ]);
    assert!(download.status.success(), "{}", stderr(&download));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "newest\noldest\n");
    assert!(stdout(&download).contains("Wrote 2 records"));

    let unconfirmed = stub.ppanel(&["logs", "delete", "--site", "shop"]);
    assert_eq!(unconfirmed.status.code(), Some(2));
    assert!(stub.requests("POST", "/api/v1/logs/deletions").is_empty());
    let delete = stub.ppanel(&["logs", "delete", "--site", "shop", "--yes"]);
    assert!(delete.status.success(), "{}", stderr(&delete));
    assert!(
        stdout(&delete).contains("Asked to delete shop's records up to 2026-10-04T10:00:00Z"),
        "{}",
        stdout(&delete)
    );
    assert_eq!(
        stub.requests("POST", "/api/v1/logs/deletions")[0].body,
        json!({"site": "shop", "since": null})
    );
    let printed = stdout(&stub.ppanel(&["logs", "deletions"]));
    assert!(printed.contains("every site"), "{printed}");
    assert!(printed.contains("applied"), "{printed}");
}

#[test]
fn the_host_is_summarized_with_its_fullest_filesystems() {
    let stub = Stub::start();
    let host = stub.ppanel(&["host"]);
    assert!(host.status.success(), "{}", stderr(&host));
    let printed = stdout(&host);
    for expected in [
        "web-1",
        "Ubuntu 24.04.2 LTS · 6.8.0 · x86_64",
        "25.0% of 4 cores",
        "6.0 GiB of 8.0 GiB used",
        "1day 1h",
        "/var",
        "critical",
        "2.0 KiB",
    ] {
        assert!(printed.contains(expected), "{expected} in\n{printed}");
    }
}

#[test]
fn the_host_agent_and_the_panels_directories_are_shown() {
    let stub = Stub::start();
    let agent = stub.ppanel(&["host", "agent"]);
    assert!(agent.status.success(), "{}", stderr(&agent));
    let printed = stdout(&agent);
    for expected in [
        "connected",
        "0.1.0",
        "web-1",
        "directories",
        "available",
        "denied",
        "grant CAP_DAC_READ_SEARCH",
    ] {
        assert!(printed.contains(expected), "{expected} in\n{printed}");
    }

    let directories = stub.ppanel(&["host", "directories"]);
    assert!(directories.status.success(), "{}", stderr(&directories));
    let printed = stdout(&directories);
    for expected in [
        "/var/lib/pingora-panel",
        "1.0 MiB",
        "partial, 3 unreadable",
        "certificates",
        "missing",
    ] {
        assert!(printed.contains(expected), "{expected} in\n{printed}");
    }
}

#[test]
fn alert_rules_and_channels_are_set_from_the_command_line() {
    let stub = Stub::start();
    let url_file = stub.config.path().join("hook-url");
    std::fs::write(&url_file, "https://hooks.example/T0/secret\n").unwrap();
    let created = stub.ppanel(&[
        "alert",
        "channel",
        "create",
        "ops",
        "--url-file",
        url_file.to_str().unwrap(),
    ]);
    assert!(created.status.success(), "{}", stderr(&created));
    let printed = stdout(&created);
    assert!(printed.contains("https://hooks.example"), "{printed}");
    assert!(printed.contains("whsec_c2VjcmV0"), "{printed}");
    assert_eq!(
        stub.requests("POST", "/api/v1/alert-channels")[0].body,
        json!({"id": "ops", "kind": "webhook", "url": "https://hooks.example/T0/secret"})
    );
    let rotated = stub.ppanel_with_input(
        &["alert", "channel", "rotate", "ops", "--url-file", "-"],
        "https://other.example/x\n",
    );
    assert!(rotated.status.success(), "{}", stderr(&rotated));
    let rotation = &stub.requests("POST", "/api/v1/alert-channels/ops/rotate")[0];
    assert_eq!(rotation.if_match.as_deref(), Some("\"1\""));
    assert_eq!(rotation.body, json!({"url": "https://other.example/x"}));

    let replaced = stub.ppanel(&[
        "alert",
        "rule",
        "set",
        "shop-errors",
        "--measure",
        "server-error-ratio",
        "--above",
        "0.05",
        "--pending",
        "5m",
        "--site",
        "shop",
        "--severity",
        "critical",
        "--channel",
        "ops",
    ]);
    assert!(replaced.status.success(), "{}", stderr(&replaced));
    assert!(stdout(&replaced).contains("Replaced alert rule shop-errors"));
    let replacing = &stub.requests("PUT", "/api/v1/alert-rules/shop-errors")[0];
    assert_eq!(replacing.if_match.as_deref(), Some("\"3\""));
    assert_eq!(
        replacing.body,
        json!({
            "name": "shop-errors", "description": "", "measure": "server_error_ratio",
            "comparison": "above", "threshold": 0.05, "pending_seconds": 300, "site": "shop",
            "route": null, "upstream": null, "severity": "critical", "enabled": true,
            "channels": ["ops"]
        })
    );
    let created = stub.ppanel(&[
        "alert",
        "rule",
        "set",
        "slow",
        "--measure",
        "latency-p95",
        "--above",
        "1.5",
    ]);
    assert!(created.status.success(), "{}", stderr(&created));
    assert_eq!(
        stub.requests("PUT", "/api/v1/alert-rules/slow")[0].if_match,
        None
    );
    let undirected = stub.ppanel(&["alert", "rule", "set", "x", "--measure", "request-rate"]);
    assert_eq!(undirected.status.code(), Some(2));

    let listed = stdout(&stub.ppanel(&["alert", "rule", "list"]));
    for expected in ["firing", "server_error_ratio > 0.05", "shop", "ops"] {
        assert!(listed.contains(expected), "{expected} in\n{listed}");
    }
    let deleted = stub.ppanel(&["alert", "rule", "delete", "shop-errors"]);
    assert!(deleted.status.success(), "{}", stderr(&deleted));
    assert_eq!(
        stub.requests("DELETE", "/api/v1/alert-rules/shop-errors")[0]
            .if_match
            .as_deref(),
        Some("\"3\"")
    );

    let tested = stub.ppanel(&["alert", "channel", "test", "ops"]);
    assert_eq!(tested.status.code(), Some(1));
    assert!(stderr(&tested).contains("the receiver answered 503"));
    let notifications = stub.ppanel(&["alert", "notifications", "--rule", "shop-errors"]);
    assert!(stdout(&notifications).contains("abandoned"));
    assert_eq!(
        stub.requests("GET", "/api/v1/alert-notifications")[0].query,
        "limit=50&rule=shop-errors"
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

#[test]
fn logging_in_keeps_a_session_for_later_commands() {
    let stub = Stub::start();
    let refused = stub.ppanel(&["whoami"]);
    assert_eq!(refused.status.code(), Some(7), "{}", stderr(&refused));
    assert!(stderr(&refused).contains("ppanel login"));

    let login = stub.ppanel_with_input(
        &["login", "--username", "ops", "--password-stdin"],
        "a long enough passphrase\n",
    );
    assert!(login.status.success(), "{}", stderr(&login));
    assert!(stdout(&login).starts_with("Logged in as ops until 2026-10-04T08:00:00Z"));
    let sent = &stub.requests("POST", "/api/v1/session")[0];
    assert_eq!(
        sent.body,
        json!({"username": "ops", "password": "a long enough passphrase", "transport": "bearer"})
    );
    assert_eq!(sent.authorization, None);
    let kept = std::fs::read_to_string(stub.config.path().join("credentials.json")).unwrap();
    assert!(kept.contains("session-secret"));

    let whoami = stub.ppanel(&["whoami"]);
    assert!(whoami.status.success(), "{}", stderr(&whoami));
    assert!(stdout(&whoami).contains("config.read,config.write"));
    assert_eq!(
        stub.requests("GET", "/api/v1/session")[1]
            .authorization
            .as_deref(),
        Some("Bearer session-secret")
    );
    stub.ppanel(&["--token", "ppat_given", "whoami"]);
    assert_eq!(
        stub.requests("GET", "/api/v1/session")[2]
            .authorization
            .as_deref(),
        Some("Bearer ppat_given")
    );

    let token = stub.ppanel(&[
        "token",
        "create",
        "ci",
        "--permission",
        "config.read",
        "--days",
        "30",
    ]);
    assert!(token.status.success(), "{}", stderr(&token));
    assert!(stdout(&token).starts_with("ppat_new\n"));
    assert_eq!(
        stub.requests("POST", "/api/v1/account/tokens")[0].body,
        json!({"name": "ci", "permissions": ["config.read"], "expires_in_days": 30})
    );

    let disabled = stub.ppanel(&["account", "update", "OPS", "--disable"]);
    assert!(disabled.status.success(), "{}", stderr(&disabled));
    assert_eq!(
        stub.requests(
            "PATCH",
            "/api/v1/accounts/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"
        )[0]
        .body,
        json!({"disabled": true})
    );

    let logout = stub.ppanel(&["logout"]);
    assert!(logout.status.success(), "{}", stderr(&logout));
    assert_eq!(stub.requests("DELETE", "/api/v1/session").len(), 1);
    let again = stub.ppanel(&["whoami"]);
    assert_eq!(again.status.code(), Some(7));
}

#[test]
fn roles_rotation_and_logging_out_everywhere() {
    let stub = Stub::start();
    let role = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "role",
        "create",
        "deployer",
        "--name",
        "Deployer",
        "--permission",
        "config.read",
        "--permission",
        "config.apply",
    ]);
    assert!(role.status.success(), "{}", stderr(&role));
    assert_eq!(
        stub.requests("POST", "/api/v1/roles")[0].body,
        json!({"id": "deployer", "name": "Deployer", "description": "",
               "permissions": ["config.read", "config.apply"]})
    );
    let missing = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "role",
        "create",
        "x",
        "--name",
        "X",
    ]);
    assert_eq!(missing.status.code(), Some(2));

    let rotated = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "token",
        "rotate",
        "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c",
    ]);
    assert!(rotated.status.success(), "{}", stderr(&rotated));
    assert!(stdout(&rotated).starts_with("ppat_rotated\n"));

    let logout = stub.ppanel(&["--token", "ppat_admin", "logout", "--everywhere"]);
    assert!(logout.status.success(), "{}", stderr(&logout));
    assert_eq!(stub.requests("DELETE", "/api/v1/account/sessions").len(), 1);
    assert_eq!(stub.requests("DELETE", "/api/v1/session").len(), 1);
}

#[test]
fn certificates_are_uploaded_generated_checked_and_replaced() {
    let stub = Stub::start();
    let files = tempfile::tempdir().unwrap();
    let chain = files.path().join("chain.pem");
    let key = files.path().join("key.pem");
    std::fs::write(&chain, "CHAIN").unwrap();
    std::fs::write(&key, "KEY").unwrap();
    let (chain, key) = (chain.to_str().unwrap(), key.to_str().unwrap());
    let run = |arguments: &[&str]| {
        let output = stub.ppanel(&[&["--token", "ppat_admin", "certificate"], arguments].concat());
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            stderr(&output)
        );
        stdout(&output)
    };

    let listed = run(&["list"]);
    assert!(listed.contains("example.com,*.example.com") && listed.contains("expiring"));
    run(&["upload", "example.com", "--chain", chain, "--key", key]);
    run(&[
        "generate",
        "internal",
        "--name",
        "intranet.example",
        "--name",
        "10.0.0.1",
        "--days",
        "30",
    ]);
    let created: Vec<Value> = stub
        .requests("POST", "/api/v1/certificates")
        .into_iter()
        .map(|request| request.body)
        .collect();
    assert_eq!(
        created,
        [
            json!({"source": "upload", "id": "example.com", "chain": "CHAIN", "key": "KEY"}),
            json!({"source": "self_signed", "id": "internal",
                   "names": ["intranet.example", "10.0.0.1"], "days": 30}),
        ]
    );

    assert!(run(&["show", "example.com"]).contains("0A:1B:FF"));
    run(&["replace", "example.com", "--chain", chain, "--key", key]);
    let replaced = &stub.requests("PUT", "/api/v1/certificates/example.com")[0];
    assert_eq!(replaced.if_match.as_deref(), Some("\"3\""));
    assert_eq!(replaced.body, json!({"chain": "CHAIN", "key": "KEY"}));
    run(&["delete", "example.com"]);
    assert_eq!(
        stub.requests("DELETE", "/api/v1/certificates/example.com")[0]
            .if_match
            .as_deref(),
        Some("\"3\"")
    );

    let checked = run(&[
        "check",
        "example.com",
        "--host",
        "www.example.com",
        "--host",
        "example.org",
    ]);
    assert_eq!(
        stub.requests("GET", "/api/v1/certificates/example.com/coverage")[0].query,
        "hosts=www.example.com%2Cexample.org"
    );
    assert!(checked.contains("example.org      false"), "{checked}");
    assert!(run(&["inspect", "--chain", chain]).contains("false"));
    assert_eq!(
        stub.requests("POST", "/api/v1/certificate-inspections")[0].body,
        json!({"chain": "CHAIN"})
    );

    let unreadable = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "certificate",
        "upload",
        "x",
        "--chain",
        "/nonexistent",
        "--key",
        key,
    ]);
    assert!(!unreadable.status.success());
    assert!(stderr(&unreadable).contains("cannot read /nonexistent"));
}

#[test]
fn acme_accounts_register_and_certificates_renew() {
    let stub = Stub::start();
    let files = tempfile::tempdir().unwrap();
    let mac_key = files.path().join("eab.key");
    std::fs::write(&mac_key, "c2VjcmV0\n").unwrap();
    let run = |arguments: &[&str]| {
        let output = stub.ppanel(&[&["--token", "ppat_admin", "acme"], arguments].concat());
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            stderr(&output)
        );
        stdout(&output)
    };

    assert!(run(&["account", "list"]).contains("ops@example.com"));
    run(&[
        "account",
        "register",
        "zerossl",
        "--directory",
        "zerossl",
        "--email",
        "ops@example.com",
        "--agree-tos",
        "--eab-key-id",
        "kid-1",
        "--eab-mac-key-file",
        mac_key.to_str().unwrap(),
    ]);
    assert_eq!(
        stub.requests("POST", "/api/v1/acme-accounts")[0].body,
        json!({"id": "zerossl", "directory": "https://acme.zerossl.com/v2/DV90",
               "contact": ["ops@example.com"], "terms_of_service_agreed": true,
               "external_account": {"key_id": "kid-1", "mac_key": "c2VjcmV0"}})
    );
    let refused = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "acme",
        "account",
        "register",
        "letsencrypt",
        "--directory",
        "letsencrypt",
    ]);
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("--agree-tos"));
    assert_eq!(stub.requests("POST", "/api/v1/acme-accounts").len(), 1);
    assert!(run(&["account", "show", "letsencrypt"]).contains("false"));
    run(&["account", "delete", "letsencrypt"]);
    assert_eq!(
        stub.requests("DELETE", "/api/v1/acme-accounts/letsencrypt")[0]
            .if_match
            .as_deref(),
        Some("\"1\"")
    );

    let listed = run(&["certificate", "list"]);
    assert!(listed.contains("example.com,www.example.com") && listed.contains("failing"));
    run(&[
        "certificate",
        "request",
        "example.com",
        "--account",
        "letsencrypt",
        "--name",
        "example.com",
        "--name",
        "www.example.com",
    ]);
    assert_eq!(
        stub.requests("POST", "/api/v1/acme-certificates")[0].body,
        json!({"id": "example.com", "account": "letsencrypt",
               "names": ["example.com", "www.example.com"], "challenge": "http-01"})
    );
    assert!(run(&["certificate", "show", "example.com"]).contains("no answer"));
    run(&["certificate", "renew", "example.com"]);
    assert_eq!(
        stub.requests("POST", "/api/v1/acme-certificates/example.com/renewals")
            .len(),
        1
    );
    run(&["certificate", "delete", "example.com"]);
    assert_eq!(
        stub.requests("DELETE", "/api/v1/acme-certificates/example.com")[0]
            .if_match
            .as_deref(),
        Some("\"4\"")
    );

    let secret = files.path().join("tsig.key");
    std::fs::write(&secret, "c2VjcmV0\n").unwrap();
    let secret = secret.to_str().unwrap();
    assert!(run(&["dns-provider", "list"]).contains("ns1.example.com:53"));
    run(&[
        "dns-provider",
        "add",
        "primary-ns",
        "--server",
        "ns1.example.com:53",
        "--zone",
        "example.com",
        "--key-name",
        "acme-update",
        "--secret-file",
        secret,
        "--propagation",
        "45",
    ]);
    assert_eq!(
        stub.requests("POST", "/api/v1/dns-providers")[0].body,
        json!({"id": "primary-ns", "kind": "rfc2136", "secret": "c2VjcmV0",
               "propagation_seconds": 45,
               "rfc2136": {"server": "ns1.example.com:53", "zones": ["example.com"],
                           "key_name": "acme-update", "algorithm": "hmac-sha256"}})
    );
    assert!(run(&["dns-provider", "show", "primary-ns"]).contains("acme-update"));
    run(&[
        "dns-provider",
        "update",
        "primary-ns",
        "--server",
        "ns2.example.com:53",
        "--zone",
        "example.com",
        "--key-name",
        "acme-update",
        "--algorithm",
        "hmac-sha512",
    ]);
    let updated = &stub.requests("PUT", "/api/v1/dns-providers/primary-ns")[0];
    assert_eq!(updated.if_match.as_deref(), Some("\"2\""));
    assert!(updated.body.get("secret").is_none(), "the secret stays");
    assert_eq!(updated.body["rfc2136"]["algorithm"], "hmac-sha512");
    run(&["dns-provider", "delete", "primary-ns"]);
    run(&[
        "certificate",
        "request",
        "wild.example.com",
        "--account",
        "letsencrypt",
        "--name",
        "*.example.com",
        "--challenge",
        "dns-01",
        "--dns-provider",
        "primary-ns",
    ]);
    assert_eq!(
        stub.requests("POST", "/api/v1/acme-certificates")[1].body,
        json!({"id": "wild.example.com", "account": "letsencrypt", "names": ["*.example.com"],
               "challenge": "dns-01", "dns_provider": "primary-ns"})
    );
}

#[test]
fn sites_are_created_serving_https() {
    let stub = Stub::start();
    let created = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "site",
        "create",
        "--name",
        "shop",
        "--domain",
        "shop.example",
        "--proxy",
        "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
        "--https-redirect",
        "--tls-profile",
        "edge",
    ]);
    assert!(created.status.success(), "{}", stderr(&created));
    let body = &stub.requests("POST", "/api/v1/sites")[0].body;
    assert_eq!(body["tls_profile_id"], "edge");
    assert_eq!(body["https_redirect"], true);
    assert_eq!(body["hsts"], Value::Null);

    let strict = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "site",
        "create",
        "--name",
        "strict",
        "--proxy",
        "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
        "--hsts-max-age",
        "31536000",
        "--hsts-include-subdomains",
    ]);
    assert!(strict.status.success(), "{}", stderr(&strict));
    assert_eq!(
        stub.requests("POST", "/api/v1/sites")[1].body["hsts"],
        json!({"max_age_seconds": 31_536_000, "include_subdomains": true, "preload": false})
    );
    let orphan = stub.ppanel(&["site", "create", "--name", "x", "--hsts-preload"]);
    assert_eq!(orphan.status.code(), Some(2));
}

#[test]
fn tls_profiles_narrow_handshakes() {
    let stub = Stub::start();
    let saved = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "tls-profile",
        "set",
        "edge",
        "--certificate-id",
        "example.com",
        "--max-protocol",
        "TLSv1.2",
        "--cipher",
        "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
        "--no-session-resumption",
    ]);
    assert!(saved.status.success(), "{}", stderr(&saved));
    let body = &stub.requests("PUT", "/api/v1/tls-profiles/edge")[0].body;
    assert_eq!(body["certificate_id"], "example.com");
    assert_eq!(body["max_protocol"], "TLSv1.2");
    assert_eq!(
        body["cipher_suites"],
        json!(["TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256"])
    );
    assert_eq!(body["session_resumption"], false);
    assert_eq!(body["ocsp_stapling"], false);
}

#[test]
fn security_policies_are_set_from_flags() {
    let stub = Stub::start();
    let saved = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "security-policy",
        "set",
        "office",
        "--allow",
        "10.0.0.0/8",
        "--method",
        "GET",
        "--basic-auth",
        "staff.htpasswd",
        "--max-body-size",
        "10m",
        "--rate-limit",
        "300r/m burst=50 key=header:X-Api-Key",
        "--limited-status",
        "503",
    ]);
    assert!(saved.status.success(), "{}", stderr(&saved));
    let body = &stub.requests("PUT", "/api/v1/security-policies/office")[0].body;
    assert_eq!(body["allowed_cidrs"], json!(["10.0.0.0/8"]));
    assert_eq!(body["allowed_methods"], json!(["GET"]));
    assert_eq!(
        body["basic_auth"],
        json!({"realm": "Restricted", "users_secret_id": "staff.htpasswd"})
    );
    assert_eq!(body["max_body_bytes"], 10 << 20);
    assert_eq!(
        body["rate_limits"][0],
        json!({"key": {"kind": "header", "name": "X-Api-Key"}, "requests": 300, "per_seconds": 60, "burst": 50})
    );
    assert_eq!(body["limited_response"]["status"], 503);
    assert!(body["referer"].is_null());

    let listed = stub.ppanel(&["--token", "ppat_admin", "security-policy", "list"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let table = String::from_utf8_lossy(&listed.stdout);
    assert!(
        table.contains("office") && table.contains("networks, password, rates"),
        "{table}"
    );
    let invalid = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "security-policy",
        "set",
        "office",
        "--rate-limit",
        "10 per second",
    ]);
    assert_eq!(invalid.status.code(), Some(2));
}

#[test]
fn listeners_name_trusted_proxies_and_head_deadlines() {
    let stub = Stub::start();
    let saved = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "listener",
        "set",
        "edge",
        "--address",
        "0.0.0.0:80",
        "--trusted-proxy",
        "10.0.0.0/8",
        "--trusted-proxy",
        "192.0.2.7",
        "--real-ip-header",
        "X-Real-IP",
        "--request-head-timeout",
        "15",
    ]);
    assert!(saved.status.success(), "{}", stderr(&saved));
    let body = &stub.requests("PUT", "/api/v1/listeners/edge")[0].body;
    assert_eq!(body["trusted_proxies"], json!(["10.0.0.0/8", "192.0.2.7"]));
    assert_eq!(body["real_ip_header"], "x-real-ip");
    assert_eq!(body["request_head_timeout_seconds"], 15);
}

#[test]
fn gateway_files_are_checked() {
    let stub = Stub::start();
    let checked = stub.ppanel(&["--token", "ppat_admin", "gateway", "files"]);
    assert!(checked.status.success(), "{}", stderr(&checked));
    let table = String::from_utf8_lossy(&checked.stdout);
    for expected in ["shop.key", "0644", "old -> /etc", "STATIC ROOT"] {
        assert!(table.contains(expected), "{expected}: {table}");
    }
    assert!(table
        .lines()
        .any(|line| line.starts_with("shop.key") && line.contains(" no ")));
}

#[test]
fn listeners_are_checked_as_clients_see_them() {
    let stub = Stub::start();
    let checked = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "listener",
        "check",
        "https",
        "--host",
        "shop.example",
    ]);
    assert!(checked.status.success(), "{}", stderr(&checked));
    assert_eq!(
        stub.requests("POST", "/api/v1/tls-checks")[0].body,
        json!({"listener": "https", "host": "shop.example"})
    );
    let shown = stdout(&checked);
    for expected in [
        "TLS13_AES_256_GCM_SHA384",
        "TLSv1.2 no, TLSv1.3 yes",
        "max-age=31536000",
        "3 ms",
    ] {
        assert!(shown.contains(expected), "{expected}: {shown}");
    }
}

#[test]
fn identity_providers_take_their_secret_from_a_file() {
    let stub = Stub::start();
    let secret = std::env::temp_dir().join(format!("ppanel-oidc-{}", std::process::id()));
    std::fs::write(&secret, "s3cret\n").unwrap();
    let saved = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "identity-provider",
        "set",
        "corp",
        "--name",
        "Corporate",
        "--issuer",
        "https://id.example",
        "--client-id",
        "panel",
        "--client-secret-file",
        secret.to_str().unwrap(),
        "--group-role",
        "ops=operator",
        "--create-accounts",
        "--groups-claim",
        "roles",
    ]);
    std::fs::remove_file(&secret).unwrap();
    assert!(saved.status.success(), "{}", stderr(&saved));
    let body = &stub.requests("PUT", "/api/v1/identity-providers/corp")[0].body;
    assert_eq!(body["client_secret"], "s3cret");
    assert_eq!(
        body["group_roles"],
        json!([{"group": "ops", "role": "operator"}])
    );
    assert_eq!(body["claims"]["groups"], "roles");
    assert_eq!(body["claims"]["username"], "preferred_username");
    assert_eq!(
        (body["create_accounts"].clone(), body["enabled"].clone()),
        (json!(true), json!(true))
    );
    assert!(
        body.get("scopes").is_none(),
        "the server's default scopes apply"
    );

    let public = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "identity-provider",
        "set",
        "corp",
        "--name",
        "Corporate",
        "--issuer",
        "https://id.example",
        "--client-id",
        "panel",
        "--public-client",
        "--disabled",
    ]);
    assert!(public.status.success(), "{}", stderr(&public));
    let body = &stub.requests("PUT", "/api/v1/identity-providers/corp")[1].body;
    assert!(body["client_secret"].is_null() && body.get("client_secret").is_some());
    assert_eq!(body["enabled"], false);

    let listed = stub.ppanel(&["--token", "ppat_admin", "identity-provider", "list"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let table = String::from_utf8_lossy(&listed.stdout);
    assert!(
        table.contains("ops=operator") && table.contains("enabled"),
        "{table}"
    );
    let invalid = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "identity-provider",
        "set",
        "corp",
        "--name",
        "C",
        "--issuer",
        "https://id.example",
        "--client-id",
        "panel",
        "--group-role",
        "ops",
    ]);
    assert_eq!(invalid.status.code(), Some(2));
}

#[test]
fn password_sign_in_can_be_limited_to_break_glass_accounts() {
    let stub = Stub::start();
    let marked = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "account",
        "update",
        "ops",
        "--break-glass",
    ]);
    assert!(marked.status.success(), "{}", stderr(&marked));
    let patches = stub.requests(
        "PATCH",
        "/api/v1/accounts/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
    );
    assert_eq!(patches[0].body, json!({"break_glass": true}));

    let limited = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "sign-in-policy",
        "set",
        "--password",
        "break-glass-only",
    ]);
    assert!(limited.status.success(), "{}", stderr(&limited));
    assert_eq!(
        stub.requests("PUT", "/api/v1/sign-in-policy")[0].body,
        json!({"password_sign_in": "break_glass_only"})
    );
    let shown = stub.ppanel(&["--token", "ppat_admin", "sign-in-policy", "show"]);
    assert!(shown.status.success(), "{}", stderr(&shown));
    assert!(String::from_utf8_lossy(&shown.stdout).contains("break-glass-only"));
    let both = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "account",
        "update",
        "ops",
        "--break-glass",
        "--no-break-glass",
    ]);
    assert_eq!(both.status.code(), Some(2));
}

fn approval_request(state: &str) -> Value {
    json!({
        "id": "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a77", "state": state, "draft_version": 6,
        "content_hash": "sha256:cc", "requested_by": "ops",
        "requested_at": "2026-10-03T09:00:00Z", "expires_at": "2026-10-04T09:00:00Z",
        "note": "covered", "risk": "high", "policies": [{"id": "prod", "version": 1}],
        "required": 1, "valid_minutes": 60,
        "changes": [{"resource": "sites/shop", "change": "changed"}],
        "approvals": if state == "approved" {
            json!([{"approver": "root", "approved_at": "2026-10-03T09:05:00Z",
                    "valid_until": "2026-10-03T10:05:00Z", "revoked_at": null}])
        } else {
            json!([])
        },
        "closed_by": null, "closed_at": null, "reason": null, "revision": null
    })
}

#[test]
fn covered_changes_wait_for_approval_from_the_command_line() {
    let stub = Stub::start();
    let waiting = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "config",
        "apply",
        "--note",
        "covered",
    ]);
    assert!(waiting.status.success(), "{}", stderr(&waiting));
    let said = String::from_utf8_lossy(&waiting.stdout);
    assert!(
        said.contains("Waiting for approval: request 0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a77")
            && said.contains("prod@v1"),
        "{said}"
    );
    let bypassed = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "config",
        "apply",
        "--bypass-reason",
        "checkout is down for everyone",
        "--incident",
        "INC-7",
    ]);
    assert!(bypassed.status.success(), "{}", stderr(&bypassed));
    assert_eq!(
        stub.requests("POST", "/api/v1/config/apply")[1].body["bypass"],
        json!({"reason": "checkout is down for everyone", "incident": "INC-7"})
    );
    let half = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "config",
        "apply",
        "--bypass-reason",
        "x",
    ]);
    assert_eq!(half.status.code(), Some(2), "a bypass needs an incident");

    let listed = stub.ppanel(&["--token", "ppat_admin", "approval", "list"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let table = String::from_utf8_lossy(&listed.stdout);
    assert!(
        table.contains("pending") && table.contains("0/1"),
        "{table}"
    );
    let approved = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "approval",
        "approve",
        "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a77",
    ]);
    assert!(approved.status.success(), "{}", stderr(&approved));
    assert!(String::from_utf8_lossy(&approved.stdout).contains("it is now approved"));

    let set = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "approval-policy",
        "set",
        "prod",
        "--site-tag",
        "prod",
        "--min-risk",
        "high",
        "--window",
        "mon,tue 09:00-18:00 Europe/Berlin",
        "--approvals",
        "2",
    ]);
    assert!(set.status.success(), "{}", stderr(&set));
    let body = &stub.requests("PUT", "/api/v1/approval-policies/prod")[0].body;
    let recurrence = body["windows"][0]["recurrence"].as_str().unwrap();
    assert!(
        recurrence.starts_with("DTSTART;TZID=Europe/Berlin:")
            && recurrence.ends_with("T090000\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU"),
        "{recurrence}"
    );
    assert_eq!(body["windows"][0]["minutes"], 540);
    assert_eq!(
        (body["min_risk"].clone(), body["approvals"].clone()),
        (json!("high"), json!(2))
    );
    assert_eq!(body["enabled"], true);
    let bad = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "approval-policy",
        "set",
        "prod",
        "--window",
        "9to5",
    ]);
    assert_eq!(bad.status.code(), Some(2));
}

#[test]
fn programs_get_service_accounts_and_workload_identities() {
    let stub = Stub::start();
    let created = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "account",
        "create",
        "deployer",
        "--service",
        "--role",
        "operator",
    ]);
    assert!(created.status.success(), "{}", stderr(&created));
    let body = &stub.requests("POST", "/api/v1/accounts")[0].body;
    assert_eq!(
        (body["service"].clone(), body["password"].clone()),
        (json!(true), Value::Null)
    );
    let both = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "account",
        "create",
        "x",
        "--service",
        "--with-password",
    ]);
    assert_eq!(both.status.code(), Some(2));

    let issued = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "account",
        "issue-token",
        "ops",
        "--name",
        "nightly",
        "--days",
        "7",
    ]);
    assert!(issued.status.success(), "{}", stderr(&issued));
    assert!(String::from_utf8_lossy(&issued.stdout).contains("ppat_issued"));
    assert_eq!(
        stub.requests(
            "POST",
            "/api/v1/accounts/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b/tokens"
        )[0]
        .body,
        json!({"name": "nightly", "permissions": null, "expires_in_days": 7})
    );

    let set = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "workload-identity",
        "set",
        "shop",
        "--account",
        "ops",
        "--issuer",
        "https://token.actions.githubusercontent.com",
        "--audience",
        "pingora-panel",
        "--subject",
        "repo:shop/site:*",
        "--claim",
        "repository=shop/site",
    ]);
    assert!(set.status.success(), "{}", stderr(&set));
    let body = &stub.requests("PUT", "/api/v1/workload-identities/shop")[0].body;
    assert_eq!(body["account_id"], "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b");
    assert_eq!(body["claims"], json!({"repository": "shop/site"}));
    assert_eq!(
        (body["session_minutes"].clone(), body["enabled"].clone()),
        (json!(15), json!(true))
    );

    let token = std::env::temp_dir().join(format!("ppanel-workload-{}", std::process::id()));
    std::fs::write(&token, "header.payload.signature\n").unwrap();
    let exchanged = stub.ppanel(&[
        "workload-identity",
        "exchange",
        "--token-file",
        token.to_str().unwrap(),
    ]);
    std::fs::remove_file(&token).unwrap();
    assert!(exchanged.status.success(), "{}", stderr(&exchanged));
    assert_eq!(
        String::from_utf8_lossy(&exchanged.stdout).trim(),
        "session-secret"
    );
    assert_eq!(
        stub.requests("POST", "/api/v1/auth/workload")[0].body,
        json!({"token": "header.payload.signature"})
    );
}

#[test]
fn accounts_are_granted_roles_for_site_groups_under_conditions() {
    let stub = Stub::start();
    let granted = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "account",
        "grant",
        "ops",
        "--role",
        "operator",
        "--site-group",
        "shop",
        "--network",
        "10.0.0.0/8",
        "--window",
        "mon,tue 09:00-18:00",
        "--until",
        "2026-12-31T00:00:00Z",
    ]);
    assert!(granted.status.success(), "{}", stderr(&granted));
    let body = &stub.requests(
        "POST",
        "/api/v1/accounts/0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b/grants",
    )[0]
    .body;
    assert_eq!(
        body["scope"],
        json!({"kind": "site_group", "group": "shop"})
    );
    assert_eq!(body["conditions"]["networks"], json!(["10.0.0.0/8"]));
    let recurrence = body["conditions"]["windows"][0]["recurrence"]
        .as_str()
        .unwrap();
    assert!(
        recurrence.starts_with("DTSTART:")
            && recurrence.ends_with("T090000Z\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU"),
        "{recurrence}"
    );
    assert_eq!(body["conditions"]["not_after"], "2026-12-31T00:00:00Z");
    assert!(String::from_utf8_lossy(&granted.stdout).contains("group shop"));
    let listed = stub.ppanel(&["--token", "ppat_admin", "account", "grants", "ops"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let table = String::from_utf8_lossy(&listed.stdout);
    assert!(
        table.contains("group shop") && table.contains("from 10.0.0.0/8"),
        "{table}"
    );
    let both = stub.ppanel(&[
        "--token",
        "ppat_admin",
        "account",
        "grant",
        "ops",
        "--role",
        "operator",
        "--site-group",
        "shop",
        "--site",
        "s-1",
    ]);
    assert_eq!(both.status.code(), Some(2));
}
