//! Lua tests (ADR 0039): the draft compiled as an apply would compile it,
//! and a request described in full run on the handlers it reaches, or on
//! one script in place of a phase's handler, with the gateway's runtime and
//! limits.

use bytes::Bytes;
use panel_config_api::{
    HeaderLine, LuaRunOutcome, LuaTest, LuaTestFailure, LuaTestLog, LuaTestReply,
    LuaTestRequestState, LuaTestResult, LuaTestRun,
};
use panel_config_model::{compile, ConfigModel};
use panel_domain::RevisionId;
use panel_errors::{PanelError, Result};
use panel_ir::LuaHandler;
use panel_lua::{LogEntry, Phase, Source};
use panel_lua_ir::{try_request, RunOutcome, Trial, TrialRequest, TrialResponse, TrialScript};
use std::net::IpAddr;

/// The phase a test names.
fn phase(name: &str) -> Result<Phase> {
    Ok(match name {
        "set" => Phase::Set,
        "server_rewrite" => Phase::ServerRewrite,
        "rewrite" => Phase::Rewrite,
        "access" => Phase::Access,
        "precontent" => Phase::Precontent,
        "content" => Phase::Content,
        "balancer" => Phase::Balancer,
        "header_filter" => Phase::HeaderFilter,
        "body_filter" => Phase::BodyFilter,
        "log" => Phase::Log,
        other => {
            return Err(PanelError::invalid_argument(format!(
                "{other:?} is not a phase a script runs in: use set, server_rewrite, rewrite, access, precontent, content, balancer, header_filter, body_filter or log"
            )))
        }
    })
}

/// The host as routing compares it: lowercase, without its port or a
/// trailing dot.
fn host(host: &str) -> String {
    let host = host.trim();
    let name = match host.strip_prefix('[') {
        Some(literal) => literal.split(']').next().unwrap_or(literal),
        None => match host.rsplit_once(':') {
            Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
            _ => host,
        },
    };
    name.trim_end_matches('.').to_ascii_lowercase()
}

fn lines(lines: Vec<HeaderLine>) -> Vec<(String, String)> {
    lines
        .into_iter()
        .map(|line| (line.name, line.value))
        .collect()
}

fn logs(entries: Vec<LogEntry>) -> Vec<LuaTestLog> {
    entries
        .into_iter()
        .map(|entry| LuaTestLog {
            level: entry.level.name().to_owned(),
            message: entry.message,
        })
        .collect()
}

fn header_lines(headers: &http::HeaderMap) -> Vec<HeaderLine> {
    headers
        .iter()
        .map(|(name, value)| HeaderLine {
            name: name.as_str().to_owned(),
            value: String::from_utf8_lossy(value.as_bytes()).into_owned(),
        })
        .collect()
}

/// What `test` does on `model`, the draft at `version`.
pub(crate) async fn run(
    model: &ConfigModel,
    version: u64,
    test: LuaTest,
    request_id: &str,
) -> Result<LuaTestResult> {
    let snapshot = compile(model, RevisionId::new(version)).map_err(|diagnostics| {
        PanelError::validation_failed("the draft does not compile").with_diagnostics(diagnostics)
    })?;
    let wanted = test.request;
    let method = wanted.method.unwrap_or_else(|| "GET".to_owned());
    if method.is_empty()
        || !method
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte == b'-')
    {
        return Err(PanelError::invalid_argument(format!(
            "{method:?} is not a request method"
        )));
    }
    let client = wanted
        .client
        .as_deref()
        .map(|client| {
            client.trim().parse::<IpAddr>().map_err(|_| {
                PanelError::invalid_argument(format!("{client:?} is not an IP address"))
            })
        })
        .transpose()?;
    let request = TrialRequest {
        method,
        host: host(&wanted.host),
        target: wanted.target.unwrap_or_else(|| "/".to_owned()),
        headers: lines(wanted.headers),
        body: wanted.body.map(Bytes::from),
        client,
        tls: wanted.tls,
        listener: wanted.listener,
        request_id: request_id.to_owned(),
    };
    let upstream = test
        .upstream
        .map_or_else(TrialResponse::default, |upstream| TrialResponse {
            status: upstream.status,
            headers: lines(upstream.headers),
            body: upstream.body.map(Bytes::from).unwrap_or_default(),
        });
    let script = match test.script {
        None => TrialScript::Configured,
        Some(script) => {
            let mut terms = LuaHandler::new("editor");
            terms.allow = script.allow;
            terms.time_limit_ms = script.time_limit_ms.unwrap_or(0);
            terms.work_limit = script.work_limit.unwrap_or(0);
            if terms.time_limit_ms > panel_engine::MOST_LUA_TIME_MS
                || terms.work_limit > panel_engine::MOST_LUA_WORK
            {
                return Err(PanelError::invalid_argument(
                    "the script's limits are beyond what the gateway allows",
                ));
            }
            TrialScript::Script {
                source: Source::new("editor", script.code, 1),
                terms,
                phase: phase(&script.phase)?,
            }
        }
    };
    let trial = try_request(&snapshot, script, request, upstream).await?;
    Ok(result(trial, version))
}

fn result(trial: Trial, version: u64) -> LuaTestResult {
    LuaTestResult {
        draft_version: version,
        site_id: trial.site,
        route_id: trial.route,
        init_logs: logs(trial.init_logs),
        runs: trial
            .runs
            .into_iter()
            .map(|run| {
                let (outcome, failure) = match run.outcome {
                    RunOutcome::Continue => (LuaRunOutcome::Continue, None),
                    RunOutcome::Respond => (LuaRunOutcome::Respond, None),
                    RunOutcome::Abort => (LuaRunOutcome::Abort, None),
                    RunOutcome::Failed { kind, message } => (
                        LuaRunOutcome::Failed,
                        Some(LuaTestFailure {
                            kind: kind.name().to_owned(),
                            message,
                        }),
                    ),
                };
                LuaTestRun {
                    phase: run.phase.name().to_owned(),
                    script: run.script,
                    outcome,
                    failure,
                    duration_us: u64::try_from(run.duration.as_micros()).unwrap_or(u64::MAX),
                    logs: logs(run.logs),
                }
            })
            .collect(),
        request: trial.request.map(|request| LuaTestRequestState {
            method: request.method,
            uri: request.uri,
            args: request.args,
            headers: header_lines(&request.headers),
        }),
        response: trial.response.map(|(status, headers, body)| LuaTestReply {
            status,
            headers: header_lines(&headers),
            body: String::from_utf8_lossy(&body).into_owned(),
        }),
        aborted: trial.aborted,
        peer: trial
            .peer
            .map(|peer| format!("{}:{}", peer.host, peer.port)),
    }
}
