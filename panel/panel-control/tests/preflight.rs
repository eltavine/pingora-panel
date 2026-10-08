#![forbid(unsafe_code)]

use panel_control::preflight::{databases, Verdict};
use panel_sqlite::{SchemaMigration, ServiceDatabase, ServiceDatabaseConfig};
use std::path::Path;

const CREATE: SchemaMigration = SchemaMigration::new(
    10_001,
    "items",
    "CREATE TABLE items (id INTEGER PRIMARY KEY, note TEXT); INSERT INTO items (note) VALUES ('kept')",
);
const RELEASED: &[SchemaMigration] = &[CREATE];
const EXPANDING: &[SchemaMigration] = &[
    CREATE,
    SchemaMigration::new(10_002, "sizes", "ALTER TABLE items ADD COLUMN size INTEGER"),
];
const CONTRACTING: &[SchemaMigration] = &[
    CREATE,
    SchemaMigration::new(10_002, "no notes", "ALTER TABLE items DROP COLUMN note"),
];
const FAILING: &[SchemaMigration] = &[
    CREATE,
    SchemaMigration::new(
        10_002,
        "unique notes",
        "CREATE UNIQUE INDEX one ON items (id, nope)",
    ),
];

async fn released(directory: &Path, module: &str, migrations: &[SchemaMigration]) {
    let database =
        ServiceDatabase::open(ServiceDatabaseConfig::new(directory, module).unwrap()).unwrap();
    database.migrate(migrations).await.unwrap();
    database.close().await;
}

async fn schema_of(directory: &Path, module: &str) -> i64 {
    let database =
        ServiceDatabase::open(ServiceDatabaseConfig::new(directory, module).unwrap()).unwrap();
    let applied = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
        .fetch_one(database.pool())
        .await
        .unwrap();
    database.close().await;
    applied
}

#[tokio::test]
async fn migrations_run_on_copies_and_report_what_they_contract() {
    let data = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    for module in ["expanding", "contracting", "failing", "current"] {
        released(data.path(), module, RELEASED).await;
    }
    released(data.path(), "newer", EXPANDING).await;

    let (findings, contracts) = databases(
        data.path(),
        scratch.path(),
        &[
            ("expanding", EXPANDING),
            ("contracting", CONTRACTING),
            ("failing", FAILING),
            ("current", RELEASED),
            ("newer", RELEASED),
            ("absent", RELEASED),
        ],
    )
    .await;
    let lines: Vec<String> = findings.iter().map(ToString::to_string).collect();
    assert!(contracts, "{lines:#?}");
    assert_eq!(
        findings
            .iter()
            .map(|finding| finding.verdict)
            .collect::<Vec<_>>(),
        [
            Verdict::Pass,
            Verdict::Warn,
            Verdict::Fail,
            Verdict::Pass,
            Verdict::Fail,
            Verdict::Pass
        ],
        "{lines:#?}"
    );
    assert_eq!(
        lines[0],
        "pass expanding: migrates from schema 10001 to 10002, expanding only"
    );
    assert!(
        lines[1].contains("drops column items.note") && lines[1].contains("needs the backup"),
        "{}",
        lines[1]
    );
    assert!(
        lines[2].contains("migrating a copy of it fails"),
        "{}",
        lines[2]
    );
    assert_eq!(lines[3], "pass current: at schema 10001 already");
    assert!(
        lines[4].contains("newer than this release's 10001"),
        "{}",
        lines[4]
    );
    assert!(lines[5].contains("no database yet"), "{}", lines[5]);

    for module in ["expanding", "contracting", "failing"] {
        assert_eq!(schema_of(data.path(), module).await, 10_001, "{module}");
    }
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
}
