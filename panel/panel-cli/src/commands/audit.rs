//! The audit trail: every change and every refused or failed attempt.

use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Format, Output},
};
use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum AuditCommand {
    /// Audit events, newest first.
    List {
        /// Who acted: an account's username.
        #[arg(long)]
        actor_id: Option<String>,
        /// An event type, or a prefix ending in `.` such as `config.`.
        #[arg(long = "type")]
        event_type: Option<String>,
        /// What was acted on, such as `configuration/draft`.
        #[arg(long)]
        subject: Option<String>,
        /// Every event of one request.
        #[arg(long)]
        correlation_id: Option<String>,
        /// Events at or after this RFC 3339 time.
        #[arg(long)]
        since: Option<String>,
        /// Events before this RFC 3339 time.
        #[arg(long)]
        until: Option<String>,
        /// Only events older than this sequence.
        #[arg(long)]
        before: Option<u64>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// One audit event with its data.
    Show { sequence: u64 },
    /// Checks that no audit event was changed or removed.
    Verify {
        #[arg(long)]
        from: Option<u64>,
        #[arg(long)]
        to: Option<u64>,
    },
}

const EVENTS: &[Column] = &[
    ("SEQ", |event| text(&event["sequence"])),
    ("TIME", |event| text(&event["occurred_at"])),
    ("ACTOR", |event| text(&event["actor_id"])),
    ("TYPE", |event| text(&event["event_type"])),
    ("SUBJECT", |event| text(&event["subject"])),
    ("CORRELATION", |event| text(&event["correlation_id"])),
];

const EVENT: &[Column] = &[
    ("Sequence", |event| text(&event["sequence"])),
    ("Type", |event| {
        format!(
            "{} v{}",
            text(&event["event_type"]),
            text(&event["event_version"])
        )
    }),
    ("Subject", |event| text(&event["subject"])),
    ("Occurred", |event| text(&event["occurred_at"])),
    ("Recorded", |event| text(&event["recorded_at"])),
    ("Actor", |event| {
        format!(
            "{} ({})",
            text(&event["actor_id"]),
            text(&event["actor_type"])
        )
    }),
    ("Source", |event| text(&event["source"])),
    ("Correlation", |event| text(&event["correlation_id"])),
    ("Causation", |event| text(&event["causation_id"])),
    ("Idempotency key", |event| text(&event["idempotency_key"])),
    ("Hash", |event| text(&event["hash"])),
    ("Previous hash", |event| text(&event["previous_hash"])),
];

pub async fn run(api: &Api, output: &Output, command: AuditCommand) -> Result<()> {
    match command {
        AuditCommand::List {
            actor_id,
            event_type,
            subject,
            correlation_id,
            since,
            until,
            before,
            limit,
        } => {
            let mut query = vec![("limit", limit.to_string())];
            for (name, value) in [
                ("actor", actor_id),
                ("type", event_type),
                ("subject", subject),
                ("correlation_id", correlation_id),
                ("since", since),
                ("until", until),
                ("before", before.map(|before| before.to_string())),
            ] {
                if let Some(value) = value {
                    query.push((name, value));
                }
            }
            let page = api.get("/api/v1/audit-events", &query).await?.body;
            if output.format == Format::Json {
                output.json(&page);
            } else {
                output.list(&page["items"], EVENTS);
            }
        }
        AuditCommand::Show { sequence } => {
            let event = api
                .get(&format!("/api/v1/audit-events/{sequence}"), &[])
                .await?
                .body;
            if output.format == Format::Json {
                output.json(&event);
            } else if !output.quiet {
                output.item(&event, EVENT);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&event["data"]).expect("JSON values serialize")
                );
            }
        }
        AuditCommand::Verify { from, to } => {
            let mut query = Vec::new();
            if let Some(from) = from {
                query.push(("from", from.to_string()));
            }
            if let Some(to) = to {
                query.push(("to", to.to_string()));
            }
            let verified = api.get("/api/v1/audit-events/verify", &query).await?.body;
            if verified["intact"] != true {
                if output.format == Format::Json {
                    output.json(&verified);
                }
                return Err(CliError::Failed(format!(
                    "the audit trail was changed: event {} does not match its hash",
                    text(&verified["first_mismatch"])
                )));
            }
            output.done(
                &format!(
                    "The audit trail is intact: {} events checked, head {} {}",
                    text(&verified["checked"]),
                    text(&verified["head_sequence"]),
                    text(&verified["head_hash"])
                ),
                &verified,
            );
        }
    }
    Ok(())
}
