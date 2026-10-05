//! `panel-control restore ARCHIVE`: installs the databases a backup holds
//! while the control plane is stopped (ADR 0035). Every member is checked
//! against the archive's manifest and every database with SQLite's
//! integrity check before anything is replaced; a database written by a
//! newer release is refused; the files replaced are kept beside the new ones.

use chrono::Utc;
use panel_errors::{PanelError, Result};
use panel_sqlite::SchemaMigration;
use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
use std::{
    fs, io,
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

const RESTORING: &str = "restoring";

/// A database installed from an archive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Installed {
    /// The module it belongs to, such as `config`.
    pub module: String,
    /// Where the file it replaced was kept; none when there was none.
    pub replaced: Option<PathBuf>,
}

/// Installs every database `archive` holds into `data_directory`, for the
/// modules `known` lists with their migrations.
pub async fn restore(
    archive: &Path,
    data_directory: &Path,
    known: &[(&str, &[SchemaMigration])],
) -> Result<Vec<Installed>> {
    let manifest = panel_backup::verify(archive)?;
    let databases: Vec<(&str, &str)> = manifest
        .members
        .iter()
        .filter_map(|member| {
            let module = member
                .path
                .strip_prefix("databases/")?
                .strip_suffix(".db")?;
            Some((member.path.as_str(), module))
        })
        .collect();
    if databases.is_empty() {
        return Err(PanelError::invalid_argument(
            "the archive holds no databases",
        ));
    }
    let mut copies = Vec::new();
    let checked = async {
        for &(path, module) in &databases {
            let migrations = known
                .iter()
                .find(|(name, _)| *name == module)
                .map(|(_, migrations)| *migrations)
                .ok_or_else(|| {
                    PanelError::invalid_argument(format!(
                        "the archive holds a database of {module}, which this release does not run"
                    ))
                })?;
            let copy = data_directory.join(format!("{module}.db.{RESTORING}"));
            remove(&copy)?;
            panel_backup::copy_member(archive, path, &copy)?;
            copies.push(copy.clone());
            check(&copy, module, migrations).await?;
        }
        Ok::<_, PanelError>(())
    }
    .await;
    if let Err(error) = checked {
        for copy in &copies {
            let _ = remove(copy);
        }
        return Err(error);
    }
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut installed = Vec::new();
    for (&(_, module), copy) in databases.iter().zip(&copies) {
        let current = data_directory.join(format!("{module}.db"));
        let replaced = current
            .exists()
            .then(|| data_directory.join(format!("{module}.db.replaced-{stamp}")));
        if let Some(kept) = &replaced {
            fs::rename(&current, kept).map_err(failure)?;
        }
        // A write-ahead log belongs to the file it was written for.
        for suffix in ["-wal", "-shm"] {
            let log = data_directory.join(format!("{module}.db{suffix}"));
            if log.exists() {
                fs::rename(
                    &log,
                    data_directory.join(format!("{module}.db{suffix}.replaced-{stamp}")),
                )
                .map_err(failure)?;
            }
        }
        fs::rename(copy, &current).map_err(failure)?;
        installed.push(Installed {
            module: module.to_owned(),
            replaced,
        });
    }
    Ok(installed)
}

/// Refuses a copy SQLite finds damaged, or one migrated further than this
/// release knows.
async fn check(copy: &Path, module: &str, migrations: &[SchemaMigration]) -> Result<()> {
    let options = SqliteConnectOptions::new()
        .filename(copy)
        .read_only(true)
        .immutable(true);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(|error| damaged(module, error))?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await
        .map_err(|error| damaged(module, error))?;
    if integrity != "ok" {
        return Err(damaged(module, integrity));
    }
    let applied: Option<i64> =
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
            .fetch_one(&mut connection)
            .await
            .unwrap_or(None);
    connection
        .close()
        .await
        .map_err(|error| damaged(module, error))?;
    let latest = SchemaMigration::latest(migrations);
    match applied {
        Some(applied) if applied > latest => Err(PanelError::validation_failed(format!(
            "the {module} database is at schema {applied}, newer than this release's {latest}; restore it with the release that took it"
        ))),
        _ => Ok(()),
    }
}

/// Fails when any of `addresses` answers: the control plane is running.
pub fn stopped(addresses: impl IntoIterator<Item = SocketAddr>) -> Result<()> {
    for address in addresses {
        if TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok() {
            return Err(PanelError::conflict(format!(
                "the control plane answers on {address}; stop it before restoring databases"
            )));
        }
    }
    Ok(())
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(failure(error)),
        _ => Ok(()),
    }
}

fn failure(error: io::Error) -> PanelError {
    PanelError::storage_unavailable(format!("cannot install the databases: {error}"))
}

fn damaged(module: &str, why: impl std::fmt::Display) -> PanelError {
    PanelError::validation_failed(format!("the archive's {module} database is damaged: {why}"))
}
