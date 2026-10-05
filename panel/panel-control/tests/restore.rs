#![forbid(unsafe_code)]

use chrono::Utc;
use panel_control::restore::{restore, stopped, Installed};
use panel_sqlite::SchemaMigration;
use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
use std::{fs, path::Path};

const KNOWN: &[(&str, &[SchemaMigration])] = &[("config", &[]), ("audit", &[])];

/// A database at schema `schema` holding `marker`.
async fn database(path: &Path, marker: &str, schema: i64) {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
    for statement in [
        "CREATE TABLE _sqlx_migrations (version BIGINT PRIMARY KEY, success BOOLEAN NOT NULL)",
        "CREATE TABLE marker (name TEXT NOT NULL)",
    ] {
        sqlx::query(statement)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO _sqlx_migrations VALUES (?1, TRUE)")
        .bind(schema)
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO marker VALUES (?1)")
        .bind(marker)
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
}

async fn marker(path: &Path) -> String {
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
            .await
            .unwrap();
    let marker = sqlx::query_scalar("SELECT name FROM marker")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    marker
}

/// An archive of the databases written below `staging`.
fn archive(staging: &Path, out: &Path) {
    panel_backup::write(out, staging, "1.2.3", &["databases"], Utc::now()).unwrap();
}

#[tokio::test]
async fn databases_are_installed_and_the_files_they_replace_kept() {
    let data = tempfile::tempdir().unwrap();
    database(&data.path().join("config.db"), "before", 1).await;
    fs::write(data.path().join("config.db-wal"), b"a log of the old file").unwrap();
    let staging = tempfile::tempdir().unwrap();
    fs::create_dir(staging.path().join("databases")).unwrap();
    database(&staging.path().join("databases/config.db"), "backed up", 1).await;
    database(&staging.path().join("databases/audit.db"), "audit", 1).await;
    let out = tempfile::tempdir().unwrap();
    let backup = out.path().join("backup.tar.zst");
    archive(staging.path(), &backup);

    let installed = restore(&backup, data.path(), KNOWN).await.unwrap();
    let modules: Vec<&str> = installed
        .iter()
        .map(|database| database.module.as_str())
        .collect();
    assert_eq!(modules, ["audit", "config"]);
    let Installed {
        replaced: Some(kept),
        ..
    } = &installed[1]
    else {
        panic!("the configuration's database replaced one");
    };
    assert_eq!(marker(&data.path().join("config.db")).await, "backed up");
    assert_eq!(marker(kept).await, "before");
    assert!(
        installed[0].replaced.is_none(),
        "there was no audit database"
    );
    assert!(
        !data.path().join("config.db-wal").exists(),
        "the old file's log does not follow the new file"
    );
    assert!(fs::read_dir(data.path()).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".restoring")));
}

#[tokio::test]
async fn newer_damaged_or_unknown_databases_replace_nothing() {
    let data = tempfile::tempdir().unwrap();
    database(&data.path().join("config.db"), "before", 1).await;
    let out = tempfile::tempdir().unwrap();

    let newer = tempfile::tempdir().unwrap();
    fs::create_dir(newer.path().join("databases")).unwrap();
    database(
        &newer.path().join("databases/config.db"),
        "later",
        9_999_999,
    )
    .await;
    archive(newer.path(), &out.path().join("newer.tar.zst"));
    let refused = restore(&out.path().join("newer.tar.zst"), data.path(), KNOWN)
        .await
        .unwrap_err();
    assert!(
        refused.message.contains("newer than this release"),
        "{refused}"
    );

    let damaged = tempfile::tempdir().unwrap();
    fs::create_dir(damaged.path().join("databases")).unwrap();
    fs::write(
        damaged.path().join("databases/config.db"),
        b"not a database",
    )
    .unwrap();
    archive(damaged.path(), &out.path().join("damaged.tar.zst"));
    let refused = restore(&out.path().join("damaged.tar.zst"), data.path(), KNOWN)
        .await
        .unwrap_err();
    assert!(refused.message.contains("damaged"), "{refused}");

    let unknown = tempfile::tempdir().unwrap();
    fs::create_dir(unknown.path().join("databases")).unwrap();
    database(&unknown.path().join("databases/billing.db"), "billing", 1).await;
    archive(unknown.path(), &out.path().join("unknown.tar.zst"));
    let refused = restore(&out.path().join("unknown.tar.zst"), data.path(), KNOWN)
        .await
        .unwrap_err();
    assert!(refused.message.contains("does not run"), "{refused}");

    assert_eq!(marker(&data.path().join("config.db")).await, "before");
    let names: Vec<String> = fs::read_dir(data.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["config.db"], "nothing is left behind");
}

#[test]
fn databases_are_restored_only_while_the_control_plane_is_stopped() {
    let listening = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let running = stopped([listening.local_addr().unwrap()]).unwrap_err();
    assert_eq!(running.code.as_str(), "CONFLICT");
    let address = listening.local_addr().unwrap();
    drop(listening);
    assert!(stopped([address]).is_ok());
}
