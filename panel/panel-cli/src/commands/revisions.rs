//! Revisions: every configuration applied or attempted, with its files.

use super::config::print_changes;
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Format, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::json;

#[derive(Subcommand)]
pub(crate) enum RevisionCommand {
    /// Revisions, newest first.
    List {
        /// Only revisions older than this one.
        #[arg(long)]
        before: Option<u64>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// A revision and the names of its files.
    Show {
        id: u64,
        /// Print this file of the revision instead.
        #[arg(long)]
        file: Option<String>,
    },
    /// What a revision changed.
    Diff {
        id: u64,
        /// `previous`, `active`, `draft` or a revision number.
        #[arg(long, default_value = "previous")]
        against: String,
    },
    /// Copies a revision's files into the draft, to review and apply.
    Restore { id: u64 },
    /// Sets a revision's note; an empty note removes it.
    Note { id: u64, note: String },
}

const REVISIONS: &[Column] = &[
    ("ID", |revision| text(&revision["id"])),
    ("OUTCOME", |revision| text(&revision["outcome"])),
    ("CREATED", |revision| text(&revision["created_at"])),
    ("AUTHOR", |revision| text(&revision["author"])),
    ("DRAFT", |revision| text(&revision["draft_version"])),
    ("NOTE", |revision| text(&revision["note"])),
];

const DETAIL: &[Column] = &[
    ("Revision", |detail| text(&detail["revision"]["id"])),
    ("Outcome", |detail| text(&detail["revision"]["outcome"])),
    ("Created", |detail| text(&detail["revision"]["created_at"])),
    ("Author", |detail| text(&detail["revision"]["author"])),
    ("Note", |detail| text(&detail["revision"]["note"])),
    ("Draft version", |detail| {
        text(&detail["revision"]["draft_version"])
    }),
    ("Language", |detail| {
        text(&detail["revision"]["language_version"])
    }),
    ("Content hash", |detail| {
        text(&detail["revision"]["content_hash"])
    }),
    ("Snapshot hash", |detail| {
        text(&detail["revision"]["snapshot_hash"])
    }),
    ("Gateway revision", |detail| {
        text(&detail["revision"]["gateway_revision"])
    }),
    ("Settled", |detail| text(&detail["revision"]["outcome_at"])),
    ("Files", |detail| {
        detail["files"]
            .as_object()
            .map(|files| files.keys().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    }),
];

pub async fn run(api: &Api, output: &Output, command: RevisionCommand) -> Result<()> {
    match command {
        RevisionCommand::List { before, limit } => {
            let mut query = vec![("limit", limit.to_string())];
            if let Some(before) = before {
                query.push(("before", before.to_string()));
            }
            let page = api.get("/api/v1/revisions", &query).await?.body;
            if output.format == Format::Json {
                output.json(&page);
            } else {
                output.list(&page["items"], REVISIONS);
            }
        }
        RevisionCommand::Show { id, file } => {
            let detail = api.get(&format!("/api/v1/revisions/{id}"), &[]).await?.body;
            match file {
                Some(name) => {
                    let Some(text) = detail["files"][&name].as_str() else {
                        return Err(CliError::Usage(format!(
                            "revision {id} has no file {name:?}"
                        )));
                    };
                    if !output.quiet {
                        print!("{text}");
                    }
                }
                None => output.item(&detail, DETAIL),
            }
        }
        RevisionCommand::Diff { id, against } => {
            let changes = api
                .get(
                    &format!("/api/v1/revisions/{id}/diff"),
                    &[("against", against)],
                )
                .await?
                .body;
            print_changes(output, &changes);
        }
        RevisionCommand::Restore { id } => {
            let source = api
                .change(
                    Method::POST,
                    &format!("/api/v1/revisions/{id}/restore"),
                    None,
                    None,
                )
                .await?
                .body;
            output.done(
                &format!(
                    "Restored revision {id} into the draft; review it with `ppanel config plan`"
                ),
                &source,
            );
        }
        RevisionCommand::Note { id, note } => {
            let revision = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/revisions/{id}/note"),
                    Some(&json!({ "note": note })),
                    None,
                )
                .await?
                .body;
            output.done(
                &if note.is_empty() {
                    format!("Removed the note of revision {id}")
                } else {
                    format!("Noted revision {id}")
                },
                &revision,
            );
        }
    }
    Ok(())
}
