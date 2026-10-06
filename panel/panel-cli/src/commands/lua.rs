//! Lua scripts (ADR 0039): checked with the configuration they belong to,
//! listed with where they run and their versions, and tested on a request
//! described on the command line.

use super::config::{draft_files, print_diagnostics, rejected};
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Format, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};
use std::path::PathBuf;

const PHASES: [&str; 10] = [
    "set",
    "server_rewrite",
    "rewrite",
    "access",
    "precontent",
    "content",
    "balancer",
    "header_filter",
    "body_filter",
    "log",
];

#[derive(Subcommand)]
pub(crate) enum LuaCommand {
    /// Checks the Lua of configuration files, or of the draft: scripts that
    /// do not compile, modules require cannot load, functions the gateway
    /// does not provide or a phase does not allow, and globals scripts
    /// write.
    Check {
        /// A directory of `.conf` and `.lua` files, or a file read as
        /// `main.conf`; the draft by default.
        path: Option<PathBuf>,
    },
    /// The scripts of the draft or of a revision, with where each runs and
    /// the SHA-256 that names its version.
    Scripts {
        #[arg(long)]
        revision: Option<u64>,
    },
    /// Runs the draft's Lua handlers a request reaches, or one script in
    /// place of a phase's handler, on a request described here; nothing is
    /// proxied or changed.
    Test {
        #[command(flatten)]
        test: Box<TestArgs>,
    },
}

/// The request a test runs on, and the script it runs instead of the draft's
/// handler.
#[derive(clap::Args)]
pub(crate) struct TestArgs {
    /// The host the request names.
    #[arg(long)]
    host: String,
    /// The request target, such as /api/items?tag=new.
    #[arg(long, default_value = "/")]
    target: String,
    #[arg(long, default_value = "GET")]
    method: String,
    /// A header line, NAME: VALUE; repeat it for more.
    #[arg(long = "header", short = 'H', value_name = "NAME: VALUE")]
    headers: Vec<String>,
    /// The request body, or @FILE to read it from a file.
    #[arg(long)]
    body: Option<String>,
    /// The client's address, after trusted proxies.
    #[arg(long)]
    client: Option<String>,
    /// The request arrives over TLS.
    #[arg(long)]
    tls: bool,
    /// The listener the request arrives on.
    #[arg(long)]
    listener: Option<String>,
    /// Runs this file's code in place of the draft's handler of --phase.
    #[arg(long, requires = "phase")]
    script: Option<PathBuf>,
    #[arg(long, value_parser = PHASES)]
    phase: Option<String>,
    /// Grants the script what lua_allow grants; repeat for more.
    #[arg(long, value_parser = ["body", "upstream", "network"])]
    allow: Vec<String>,
    /// The status the upstream answers a proxied request with.
    #[arg(long, default_value_t = 200)]
    upstream_status: u16,
    /// The body the upstream answers with, or @FILE.
    #[arg(long)]
    upstream_body: Option<String>,
}

const SCRIPTS: &[Column] = &[
    ("SCRIPT", |script| text(&script["id"])),
    ("MODULE", |script| text(&script["module"])),
    ("RUNS", |script| {
        let uses: Vec<String> = script["uses"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|place| format!("{} in {}", text(&place["phase"]), text(&place["label"])))
            .collect();
        if uses.is_empty() {
            "-".into()
        } else {
            uses.join("; ")
        }
    }),
    ("LINES", |script| text(&script["lines"])),
    ("VERSION", |script| {
        text(&script["sha256"]).chars().take(12).collect()
    }),
];

const RUNS: &[Column] = &[
    ("PHASE", |run| text(&run["phase"])),
    ("SCRIPT", |run| text(&run["script"])),
    ("OUTCOME", |run| match run["failure"]["message"].as_str() {
        Some(message) => format!("{} ({})", text(&run["outcome"]), message),
        None => text(&run["outcome"]),
    }),
    ("TIME", |run| {
        run["duration_us"].as_u64().map_or_else(
            || "-".into(),
            |micros| format!("{:.3} ms", micros as f64 / 1000.0),
        )
    }),
];

/// Whether `diagnostic` is about Lua.
fn about_lua(diagnostic: &Value) -> bool {
    diagnostic["code"] == "DSL_LUA"
        || diagnostic["source_span"]
            .as_str()
            .is_some_and(|span| span.starts_with("lua/"))
        || text(&diagnostic["message"])
            .to_ascii_lowercase()
            .contains("lua")
}

/// The text of `value`, or of the file `@path` names.
fn content(value: Option<String>) -> Result<Option<String>> {
    match value {
        Some(value) => match value.strip_prefix('@') {
            Some(path) => std::fs::read_to_string(path)
                .map(Some)
                .map_err(|error| CliError::Usage(format!("cannot read {path}: {error}"))),
            None => Ok(Some(value)),
        },
        None => Ok(None),
    }
}

fn header_lines(headers: &[String]) -> Result<Vec<Value>> {
    headers
        .iter()
        .map(|line| {
            let (name, value) = line.split_once(':').ok_or_else(|| {
                CliError::Usage(format!("{line:?} is not a header line NAME: VALUE"))
            })?;
            Ok(json!({ "name": name.trim(), "value": value.trim() }))
        })
        .collect()
}

pub(crate) async fn run(api: &Api, output: &Output, command: LuaCommand) -> Result<()> {
    match command {
        LuaCommand::Check { path } => {
            let files = draft_files(api, path).await?;
            let result = api
                .post_read("/api/v1/config/check", &json!({ "files": files }))
                .await?
                .body;
            let found: Vec<Value> = result["diagnostics"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|diagnostic| about_lua(diagnostic))
                .cloned()
                .collect();
            let failed = found.iter().any(|diagnostic| {
                diagnostic["severity"]
                    .as_str()
                    .is_some_and(|severity| severity.eq_ignore_ascii_case("error"))
            });
            if output.format == Format::Json {
                output.json(&json!({ "valid": !failed, "diagnostics": found }));
            } else {
                print_diagnostics(&Value::Array(found.clone()));
            }
            if failed {
                return Err(rejected("the Lua scripts have errors"));
            }
            if output.format != Format::Json {
                output.done(
                    &format!("The Lua scripts check, with {} warnings", found.len()),
                    &result,
                );
            }
        }
        LuaCommand::Scripts { revision } => {
            let query: Vec<(&str, String)> = revision
                .map(|revision| ("revision", revision.to_string()))
                .into_iter()
                .collect();
            let library = api.get("/api/v1/config/lua", &query).await?.body;
            if output.format == Format::Json {
                output.json(&library);
                return Ok(());
            }
            output.list(&library["scripts"], SCRIPTS);
            print_diagnostics(&library["diagnostics"]);
            if library["disabled"] == true && !output.quiet {
                println!("lua off: the scripts are kept, but none runs");
            }
        }
        LuaCommand::Test { test } => {
            let TestArgs {
                host,
                target,
                method,
                headers,
                body,
                client,
                tls,
                listener,
                script,
                phase,
                allow,
                upstream_status,
                upstream_body,
            } = *test;
            let script = match script {
                Some(path) => {
                    let code = std::fs::read_to_string(&path).map_err(|error| {
                        CliError::Usage(format!("cannot read {}: {error}", path.display()))
                    })?;
                    let mut granted = json!({});
                    for permission in &allow {
                        granted[permission] = json!(true);
                    }
                    Some(json!({ "code": code, "phase": phase, "allow": granted }))
                }
                None => None,
            };
            let test = json!({
                "request": {
                    "method": method, "host": host, "target": target,
                    "headers": header_lines(&headers)?, "body": content(body)?,
                    "client": client, "tls": tls, "listener": listener,
                },
                "upstream": { "status": upstream_status, "body": content(upstream_body)? },
                "script": script,
            });
            let result = api
                .change(Method::POST, "/api/v1/config/lua/test", Some(&test), None)
                .await?
                .body;
            if output.format == Format::Json {
                output.json(&result);
                return Ok(());
            }
            output.list(&result["runs"], RUNS);
            if output.quiet {
                return Ok(());
            }
            for run in result["runs"].as_array().into_iter().flatten() {
                for log in run["logs"].as_array().into_iter().flatten() {
                    println!(
                        "[{}] {}: {}",
                        text(&log["level"]),
                        text(&run["phase"]),
                        text(&log["message"])
                    );
                }
            }
            if result["aborted"] == true {
                println!("the connection closes without an answer");
            } else if let Some(status) = result["response"]["status"].as_u64() {
                println!("answers {status}");
                for header in result["response"]["headers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    println!("{}: {}", text(&header["name"]), text(&header["value"]));
                }
                let body = text(&result["response"]["body"]);
                if !body.is_empty() && body != "-" {
                    println!("\n{body}");
                }
            }
            if let Some(peer) = result["peer"].as_str() {
                println!("the balancer chose {peer}");
            }
        }
    }
    Ok(())
}
