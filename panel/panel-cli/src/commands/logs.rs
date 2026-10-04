//! The gateway's access and error logs.

use super::time;
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Format, Output},
};
use clap::{Args, Subcommand};
use futures_util::StreamExt;
use reqwest::Method;
use serde_json::{json, Value};
use std::{
    io::{ErrorKind, Write},
    path::PathBuf,
    time::Duration,
};
use tokio_tungstenite::tungstenite::Message;

/// How long a download may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// The pause before following again after falling behind.
const RESUME_DELAY: Duration = Duration::from_millis(250);

#[derive(Args)]
pub(crate) struct Filter {
    /// Only access records, or only error records.
    #[arg(long, value_parser = ["access", "error"])]
    kind: Option<String>,
    /// Only the records of this site.
    #[arg(long)]
    site: Option<String>,
    /// Only the records of this route.
    #[arg(long)]
    route: Option<String>,
    /// A status code such as `502`, or a class such as `5xx`.
    #[arg(long)]
    status: Option<String>,
    /// A client address, or a CIDR block of them.
    #[arg(long)]
    client: Option<String>,
    /// Requests whose path starts with this.
    #[arg(long)]
    path: Option<String>,
    /// The records of one request.
    #[arg(long)]
    request_id: Option<String>,
    /// Records that contain this text, ignoring case.
    #[arg(long)]
    text: Option<String>,
}

impl Filter {
    fn query(self) -> Vec<(&'static str, String)> {
        [
            ("kind", self.kind),
            ("site", self.site),
            ("route", self.route),
            ("status", self.status),
            ("client", self.client),
            ("path", self.path),
            ("request_id", self.request_id),
            ("text", self.text),
        ]
        .into_iter()
        .filter_map(|(name, value)| Some((name, value?)))
        .collect()
    }
}

#[derive(Args)]
pub(crate) struct Window {
    /// The oldest records to include, in RFC 3339 or as how long ago such
    /// as `1h`; an hour before `--until` by default.
    #[arg(long, value_parser = time)]
    since: Option<String>,
    /// Only records before this, in RFC 3339 or as how long ago; now by
    /// default.
    #[arg(long, value_parser = time)]
    until: Option<String>,
}

impl Window {
    fn add_to(self, query: &mut Vec<(&'static str, String)>) {
        query.extend(self.since.map(|since| ("since", since)));
        query.extend(self.until.map(|until| ("until", until)));
    }
}

#[derive(Subcommand)]
pub(crate) enum LogsCommand {
    /// Records that match, newest first.
    Search {
        #[command(flatten)]
        filter: Filter,
        #[command(flatten)]
        window: Window,
        /// How many records to show.
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=500))]
        limit: u32,
    },
    /// Follows records as they arrive, until interrupted; prints their lines,
    /// or one JSON object per line with `--output json`.
    Tail {
        #[command(flatten)]
        filter: Filter,
        /// Start after this time rather than now, in RFC 3339 or as how long
        /// ago such as `5m`.
        #[arg(long, value_parser = time)]
        after: Option<String>,
    },
    /// Writes the lines of up to 100,000 matching records, newest first.
    Download {
        #[command(flatten)]
        filter: Filter,
        #[command(flatten)]
        window: Window,
        /// The file to write; standard output by default.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Deletes the records of one site, or of every site, up to now.
    Delete {
        /// The site whose records go; every site's by default.
        #[arg(long)]
        site: Option<String>,
        /// Only records from this time on, in RFC 3339 or as how long ago;
        /// every older record too by default.
        #[arg(long, value_parser = time)]
        since: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Deletions asked for, pending or applied, newest first.
    Deletions,
}

const RECORDS: &[Column] = &[
    ("TIME", |record| text(&record["time"])),
    ("KIND", |record| text(&record["kind"])),
    ("SITE", |record| text(&record["site"])),
    ("ROUTE", |record| text(&record["route"])),
    ("STATUS", |record| text(&record["status"])),
    ("METHOD", |record| text(&record["method"])),
    ("PATH", |record| text(&record["path"])),
    ("CLIENT", |record| text(&record["client"])),
    ("REQUEST", |record| text(&record["request_id"])),
];

const DELETIONS: &[Column] = &[
    ("SITE", |deletion| match &deletion["site"] {
        Value::Null => "every site".into(),
        site => text(site),
    }),
    ("SINCE", |deletion| text(&deletion["since"])),
    ("UNTIL", |deletion| text(&deletion["until"])),
    ("REQUESTED", |deletion| text(&deletion["requested_at"])),
    ("STATE", |deletion| text(&deletion["state"])),
];

/// Prints records as the gateway wrote them, or as JSON lines; false once
/// standard output is closed.
fn print(output: &Output, records: &[Value]) -> Result<bool> {
    if output.quiet {
        return Ok(true);
    }
    let mut stdout = std::io::stdout().lock();
    for record in records {
        let written = match output.format {
            Format::Json => writeln!(stdout, "{record}"),
            Format::Table => writeln!(
                stdout,
                "{}",
                record["line"].as_str().unwrap_or_default().trim_end()
            ),
        };
        match written {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::BrokenPipe => return Ok(false),
            Err(error) => return Err(CliError::Failed(format!("cannot print: {error}"))),
        }
    }
    match stdout.flush() {
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(false),
        _ => Ok(true),
    }
}

/// Follows records until interrupted, the API ends the tail or standard
/// output closes; picks up from the last record after falling behind.
async fn tail(
    api: &Api,
    output: &Output,
    filter: Vec<(&'static str, String)>,
    mut after: Option<String>,
) -> Result<()> {
    loop {
        let mut query = filter.clone();
        query.extend(after.clone().map(|after| ("after", after)));
        let mut socket = api.websocket("/api/v1/logs/tail", &query).await?;
        let ended = loop {
            let message = tokio::select! {
                message = socket.next() => message,
                _ = tokio::signal::ctrl_c() => {
                    let _ = socket.close(None).await;
                    return Ok(());
                }
            };
            let message = match message {
                None | Some(Ok(Message::Close(_))) => return Ok(()),
                Some(Err(error)) => return Err(CliError::transport(error)),
                Some(Ok(Message::Text(message))) => serde_json::from_str::<Value>(&message)
                    .map_err(|error| {
                        CliError::Transport(format!("the API sent an unreadable record: {error}"))
                    })?,
                Some(Ok(_)) => continue,
            };
            let records = message["records"].as_array().map_or(&[][..], Vec::as_slice);
            if !print(output, records)? {
                let _ = socket.close(None).await;
                return Ok(());
            }
            if let Some(cursor) = message["cursor"].as_str() {
                after = Some(cursor.to_owned());
            }
            if !message["error"].is_null() {
                break message["error"].clone();
            }
        };
        let code = ended["code"].as_str().unwrap_or_default().to_owned();
        if code == "RESOURCE_EXHAUSTED" {
            tokio::time::sleep(RESUME_DELAY).await;
            continue;
        }
        let mut message = ended["message"]
            .as_str()
            .unwrap_or("the tail ended")
            .to_owned();
        if let Some(after) = &after {
            message.push_str(&format!("\n  resume with --after {after}"));
        }
        return Err(CliError::Ended { code, message });
    }
}

/// Writes a download to `file`, or to standard output; returns the number
/// of records.
async fn download(mut response: reqwest::Response, file: Option<&PathBuf>) -> Result<usize> {
    let mut sink: Box<dyn Write> = match file {
        Some(path) => Box::new(std::io::BufWriter::new(
            std::fs::File::create(path).map_err(|error| {
                CliError::Failed(format!("cannot create {}: {error}", path.display()))
            })?,
        )),
        None => Box::new(std::io::stdout().lock()),
    };
    let mut records = 0;
    let written: Result<()> = async {
        while let Some(chunk) = response.chunk().await.map_err(CliError::transport)? {
            records += chunk.iter().filter(|byte| **byte == b'\n').count();
            sink.write_all(&chunk)
                .map_err(|error| CliError::Failed(format!("cannot write: {error}")))?;
        }
        sink.flush()
            .map_err(|error| CliError::Failed(format!("cannot write: {error}")))
    }
    .await;
    if let (Err(_), Some(path)) = (&written, file) {
        drop(sink);
        let _ = std::fs::remove_file(path);
    }
    written.map(|()| records)
}

pub async fn run(api: &Api, output: &Output, command: LogsCommand) -> Result<()> {
    match command {
        LogsCommand::Search {
            filter,
            window,
            limit,
        } => {
            let mut query = filter.query();
            window.add_to(&mut query);
            query.push(("limit", limit.to_string()));
            let page = api.get("/api/v1/logs", &query).await?.body;
            if output.format == Format::Json {
                output.json(&page);
            } else {
                output.list(&page["records"], RECORDS);
                match page["next_until"].as_str() {
                    Some(until) if !output.quiet => {
                        eprintln!("older records: pass --until {until}");
                    }
                    _ => {}
                }
            }
        }
        LogsCommand::Tail { filter, after } => tail(api, output, filter.query(), after).await?,
        LogsCommand::Download {
            filter,
            window,
            file,
        } => {
            let mut query = filter.query();
            window.add_to(&mut query);
            let response = api
                .stream("/api/v1/logs/download", &query, DOWNLOAD_TIMEOUT)
                .await?;
            let records = download(response, file.as_ref()).await?;
            if let Some(file) = file {
                output.done(
                    &format!("Wrote {records} records to {}", file.display()),
                    &json!({"file": file, "records": records}),
                );
            }
        }
        LogsCommand::Delete { site, since, yes } => {
            if !yes {
                return Err(CliError::Usage(
                    "deleted records cannot be restored; pass --yes to confirm".into(),
                ));
            }
            let body = json!({"site": site, "since": since});
            let deletion = api
                .change(Method::POST, "/api/v1/logs/deletions", Some(&body), None)
                .await?
                .body;
            let whose = site.map_or_else(|| "every site's".to_owned(), |site| format!("{site}'s"));
            output.done(
                &format!(
                    "Asked to delete {whose} records up to {}; the log store deletes them \
                     once the request can no longer be cancelled",
                    text(&deletion["until"])
                ),
                &deletion,
            );
        }
        LogsCommand::Deletions => {
            let deletions = api.get("/api/v1/logs/deletions", &[]).await?.body;
            output.list(&deletions["deletions"], DELETIONS);
        }
    }
    Ok(())
}
