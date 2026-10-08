//! `panel-control preflight`: what upgrading to this release would do to
//! the live databases, and whether it speaks the protocols the running
//! release does (ADR 0047). Nothing live changes: each database is copied
//! as one consistent snapshot, and the copy is migrated.

use panel_contracts::ProtocolRevisions;
use panel_errors::{PanelError, Result};
use panel_sqlite::{SchemaMigration, ServiceDatabase, ServiceDatabaseConfig};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{sqlite::SqliteConnectOptions, Connection, Row, SqliteConnection, SqlitePool};
use std::{collections::BTreeMap, fmt, fs, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
}

/// What one check found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding {
    pub verdict: Verdict,
    /// A module, such as `config`, or a protocol package.
    pub subject: String,
    pub detail: String,
}

impl Finding {
    pub fn new(verdict: Verdict, subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            verdict,
            subject: subject.into(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verdict = match self.verdict {
            Verdict::Pass => "pass",
            Verdict::Warn => "warn",
            Verdict::Fail => "fail",
        };
        write!(formatter, "{verdict} {}: {}", self.subject, self.detail)
    }
}

/// The revisions of one protocol package, as releases and the versions
/// report name them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Revisions {
    pub name: String,
    pub min_revision: u32,
    pub max_revision: u32,
}

impl From<&ProtocolRevisions> for Revisions {
    fn from(value: &ProtocolRevisions) -> Self {
        Self {
            name: value.package.to_owned(),
            min_revision: value.min,
            max_revision: value.max,
        }
    }
}

/// The revisions the running installation speaks, with who speaks them:
/// `document` is what `panel-control protocols` of its image prints, or the
/// versions report `GET /api/v1/system/versions` answers.
pub fn peers(document: &str) -> Result<Vec<(String, Revisions)>> {
    let unreadable = |error: serde_json::Error| {
        PanelError::invalid_argument(format!("the peers cannot be read: {error}"))
    };
    let document: Value = serde_json::from_str(document).map_err(unreadable)?;
    if document.is_array() {
        let revisions: Vec<Revisions> = serde_json::from_value(document).map_err(unreadable)?;
        return Ok(revisions
            .into_iter()
            .map(|revisions| ("the running release".to_owned(), revisions))
            .collect());
    }
    let mut peers = Vec::new();
    for module in document["modules"].as_array().into_iter().flatten() {
        let service = module["service"].as_str().unwrap_or("a module").to_owned();
        let revisions: Vec<Revisions> =
            serde_json::from_value(module["protocols"].clone()).map_err(unreadable)?;
        peers.extend(
            revisions
                .into_iter()
                .map(|revisions| (service.clone(), revisions)),
        );
    }
    Ok(peers)
}

/// Whether `ours` overlaps every package `peers` speak: a peer replaced
/// later in the upgrade must still be understood until it is.
pub fn protocol_findings(
    ours: &[ProtocolRevisions],
    peers: &[(String, Revisions)],
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    for (owner, peer) in peers {
        let finding = match ours.iter().find(|revisions| revisions.package == peer.name) {
            None => Finding::new(
                Verdict::Warn,
                &peer.name,
                format!(
                    "this release no longer speaks it; {owner} speaks {}..{}",
                    peer.min_revision, peer.max_revision
                ),
            ),
            Some(revisions)
                if revisions.min.max(peer.min_revision)
                    <= revisions.max.min(peer.max_revision) =>
            {
                Finding::new(
                    Verdict::Pass,
                    &peer.name,
                    format!(
                        "{}..{} overlaps {}..{} of {owner}",
                        revisions.min, revisions.max, peer.min_revision, peer.max_revision
                    ),
                )
            }
            Some(revisions) => Finding::new(
                Verdict::Fail,
                &peer.name,
                format!(
                    "this release speaks {}..{} and {owner} {}..{}; upgrade through a release that speaks both",
                    revisions.min, revisions.max, peer.min_revision, peer.max_revision
                ),
            ),
        };
        if !findings.contains(&finding) {
            findings.push(finding);
        }
    }
    findings
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Column {
    kind: String,
    not_null: bool,
    default: Option<String>,
}

/// Each table's columns by name.
type Schema = BTreeMap<String, BTreeMap<String, Column>>;

/// What migrating from `before` to `after` takes away: a table or column,
/// a column's type, or rows that may hold no value.
fn contractions(before: &Schema, after: &Schema) -> Vec<String> {
    let mut found = Vec::new();
    for (table, columns) in before {
        let Some(kept) = after.get(table) else {
            found.push(format!("drops table {table}"));
            continue;
        };
        for (name, column) in columns {
            match kept.get(name) {
                None => found.push(format!("drops column {table}.{name}")),
                Some(now) if now.kind != column.kind => found.push(format!(
                    "retypes column {table}.{name} from {} to {}",
                    column.kind, now.kind
                )),
                Some(now) if now.not_null && !column.not_null => {
                    found.push(format!("requires column {table}.{name}"));
                }
                Some(_) => {}
            }
        }
        for (name, column) in kept {
            if !columns.contains_key(name) && column.not_null && column.default.is_none() {
                found.push(format!("requires new column {table}.{name}"));
            }
        }
    }
    found
}

async fn schema(pool: &SqlitePool) -> Result<Schema> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .map_err(unreadable)?;
    let mut schema = Schema::new();
    for table in tables {
        let rows =
            sqlx::query("SELECT name, type, \"notnull\", dflt_value FROM pragma_table_info(?1)")
                .bind(&table)
                .fetch_all(pool)
                .await
                .map_err(unreadable)?;
        let columns = rows
            .iter()
            .map(|row| {
                Ok((
                    row.try_get::<String, _>(0).map_err(unreadable)?,
                    Column {
                        kind: row
                            .try_get::<String, _>(1)
                            .map_err(unreadable)?
                            .to_uppercase(),
                        not_null: row.try_get::<i64, _>(2).map_err(unreadable)? != 0,
                        default: row.try_get::<Option<String>, _>(3).map_err(unreadable)?,
                    },
                ))
            })
            .collect::<Result<_>>()?;
        schema.insert(table, columns);
    }
    Ok(schema)
}

async fn applied(pool: &SqlitePool) -> Option<i64> {
    sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
        .fetch_one(pool)
        .await
        .unwrap_or(None)
}

/// Copies `live` to `copy` as one consistent snapshot, whatever the
/// running release writes meanwhile.
async fn snapshot(live: &Path, copy: &Path) -> Result<()> {
    let options = SqliteConnectOptions::new().filename(live).read_only(true);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(unreadable)?;
    sqlx::query("VACUUM INTO ?1")
        .bind(copy.to_string_lossy().into_owned())
        .execute(&mut connection)
        .await
        .map_err(unreadable)?;
    connection.close().await.map_err(unreadable)
}

/// What migrating each database of `known` in `data_directory` to this
/// release does, working on copies in `scratch`; and whether any
/// migration contracts a schema.
pub async fn databases(
    data_directory: &Path,
    scratch: &Path,
    known: &[(&str, &[SchemaMigration])],
) -> (Vec<Finding>, bool) {
    let mut findings = Vec::new();
    let mut contracts = false;
    for &(module, migrations) in known {
        let live = data_directory.join(format!("{module}.db"));
        let latest = SchemaMigration::latest(migrations);
        if !live.exists() {
            findings.push(Finding::new(
                Verdict::Pass,
                module,
                format!("no database yet; it starts at schema {latest}"),
            ));
            continue;
        }
        let copy = scratch.join(format!("{module}.db"));
        discard(scratch, module);
        let finding = match migrated(&live, &copy, migrations).await {
            Ok((from, found)) => {
                let from = from.unwrap_or_default();
                if from > latest {
                    Finding::new(
                        Verdict::Fail,
                        module,
                        format!(
                            "schema {from} is newer than this release's {latest}; a later release wrote it"
                        ),
                    )
                } else if from == latest {
                    Finding::new(Verdict::Pass, module, format!("at schema {latest} already"))
                } else if found.is_empty() {
                    Finding::new(
                        Verdict::Pass,
                        module,
                        format!("migrates from schema {from} to {latest}, expanding only"),
                    )
                } else {
                    contracts = true;
                    Finding::new(
                        Verdict::Warn,
                        module,
                        format!(
                            "migrates from schema {from} to {latest} and {}; rolling back then needs the backup",
                            found.join(", ")
                        ),
                    )
                }
            }
            Err(error) => Finding::new(Verdict::Fail, module, error.message),
        };
        findings.push(finding);
        discard(scratch, module);
    }
    (findings, contracts)
}

fn discard(scratch: &Path, module: &str) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(scratch.join(format!("{module}.db{suffix}")));
    }
}

/// The schema `live` was at, and what migrating a copy of it contracts.
async fn migrated(
    live: &Path,
    copy: &Path,
    migrations: &[SchemaMigration],
) -> Result<(Option<i64>, Vec<String>)> {
    snapshot(live, copy).await?;
    let database = ServiceDatabase::open(ServiceDatabaseConfig::at(copy))?;
    let result = async {
        let from = applied(database.pool()).await;
        if from.is_some_and(|from| from >= SchemaMigration::latest(migrations)) {
            return Ok((from, Vec::new()));
        }
        let before = schema(database.pool()).await?;
        database.migrate(migrations).await.map_err(|error| {
            PanelError::validation_failed(format!(
                "migrating a copy of it fails: {}",
                error.message
            ))
        })?;
        let after = schema(database.pool()).await?;
        Ok((from, contractions(&before, &after)))
    }
    .await;
    database.close().await;
    result
}

fn unreadable(error: impl fmt::Display) -> PanelError {
    PanelError::storage_unavailable(format!("the database cannot be read: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(kind: &str, not_null: bool, default: Option<&str>) -> Column {
        Column {
            kind: kind.into(),
            not_null,
            default: default.map(str::to_owned),
        }
    }

    fn schema_of(tables: &[(&str, &[(&str, Column)])]) -> Schema {
        tables
            .iter()
            .map(|(table, columns)| {
                (
                    (*table).to_owned(),
                    columns
                        .iter()
                        .map(|(name, column)| ((*name).to_owned(), column.clone()))
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn contractions_are_what_an_older_release_would_miss() {
        let before = schema_of(&[
            ("gone", &[("id", column("INTEGER", true, None))]),
            (
                "kept",
                &[
                    ("id", column("INTEGER", true, None)),
                    ("note", column("TEXT", false, None)),
                    ("size", column("INTEGER", false, None)),
                    ("old", column("TEXT", false, None)),
                ],
            ),
        ]);
        let after = schema_of(&[(
            "kept",
            &[
                ("id", column("INTEGER", true, None)),
                ("note", column("TEXT", true, None)),
                ("size", column("TEXT", false, None)),
                ("added", column("TEXT", true, Some("''"))),
                ("required", column("TEXT", true, None)),
            ],
        )]);
        assert_eq!(
            contractions(&before, &after),
            [
                "drops table gone",
                "requires column kept.note",
                "drops column kept.old",
                "retypes column kept.size from INTEGER to TEXT",
                "requires new column kept.required",
            ]
        );
        assert!(contractions(&before, &before).is_empty());
    }

    #[test]
    fn peers_read_from_a_release_or_from_its_versions_report() {
        let release =
            peers(r#"[{"name":"pingora.panel.config.v1","min_revision":1,"max_revision":2}]"#)
                .unwrap();
        assert_eq!(release[0].0, "the running release");
        assert_eq!(release[0].1.max_revision, 2);
        let report = peers(
            r#"{"modules":[{"service":"config-service","protocols":[
                {"name":"pingora.panel.config.v1","min_revision":1,"max_revision":1}]}]}"#,
        )
        .unwrap();
        assert_eq!(report[0].0, "config-service");
        assert!(peers("not json").is_err());
    }

    #[test]
    fn protocols_must_overlap_what_the_running_release_speaks() {
        let ours = [ProtocolRevisions {
            package: "pingora.panel.gateway.v1",
            min: 2,
            max: 3,
        }];
        let speaking = |min, max| {
            vec![(
                "the running release".to_owned(),
                Revisions {
                    name: "pingora.panel.gateway.v1".into(),
                    min_revision: min,
                    max_revision: max,
                },
            )]
        };
        assert_eq!(
            protocol_findings(&ours, &speaking(1, 2))[0].verdict,
            Verdict::Pass
        );
        let disjoint = &protocol_findings(&ours, &speaking(1, 1))[0];
        assert_eq!(disjoint.verdict, Verdict::Fail);
        assert!(disjoint
            .detail
            .contains("upgrade through a release that speaks both"));
        let dropped = protocol_findings(
            &ours,
            &[(
                "audit-service".to_owned(),
                Revisions {
                    name: "pingora.panel.audit.v1".into(),
                    min_revision: 1,
                    max_revision: 1,
                },
            )],
        );
        assert_eq!(dropped[0].verdict, Verdict::Warn);
    }
}
