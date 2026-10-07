use crate::storage_error;
use panel_errors::{PanelError, Result};
use sqlx::{
    migrate::{Migration, MigrationType, Migrator},
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
    },
    SqlSafeStr, Sqlite, Transaction,
};
use std::{
    borrow::Cow,
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Notify;

/// One forward-only schema migration. Platform migrations occupy versions
/// below [`SchemaMigration::SERVICE_VERSION_FLOOR`]; service migrations use
/// versions at or above it so both sets apply in one ordered history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaMigration {
    version: i64,
    description: &'static str,
    sql: &'static str,
    in_transaction: bool,
}

impl SchemaMigration {
    pub const SERVICE_VERSION_FLOOR: i64 = 10_000;

    pub const fn new(version: i64, description: &'static str, sql: &'static str) -> Self {
        Self {
            version,
            description,
            sql,
            in_transaction: true,
        }
    }

    /// A migration that rebuilds a table other tables reference, as SQLite
    /// changes what `ALTER TABLE` cannot: it runs outside a transaction, so
    /// that it can turn foreign key enforcement off and the old table's
    /// rows are not deleted or cascaded when it is dropped, and its SQL
    /// brackets the rebuild in a transaction of its own.
    pub const fn rebuilding(version: i64, description: &'static str, sql: &'static str) -> Self {
        Self {
            version,
            description,
            sql,
            in_transaction: false,
        }
    }

    pub const fn version(&self) -> i64 {
        self.version
    }

    /// The schema version a service reaches after applying the platform
    /// migrations and `service`.
    pub fn latest(service: &[SchemaMigration]) -> i64 {
        PLATFORM_MIGRATIONS
            .iter()
            .chain(service)
            .map(|migration| migration.version)
            .max()
            .unwrap_or_default()
    }

    fn to_sqlx(self) -> Migration {
        Migration::new(
            self.version,
            Cow::Borrowed(self.description),
            MigrationType::Simple,
            self.sql.into_sql_str(),
            !self.in_transaction,
        )
    }
}

/// Tables every module's database carries, such as the outbox and inbox.
const PLATFORM_MIGRATIONS: &[SchemaMigration] = &[
    SchemaMigration::new(
        1,
        "transactional outbox",
        include_str!("../migrations/0001_outbox.sql"),
    ),
    SchemaMigration::new(
        2,
        "processed events",
        include_str!("../migrations/0002_processed_events.sql"),
    ),
];

/// Where a module's database file lives and how its pool behaves.
#[derive(Clone, Debug)]
pub struct ServiceDatabaseConfig {
    path: PathBuf,
    max_connections: u32,
    acquire_timeout: Duration,
    busy_timeout: Duration,
}

impl ServiceDatabaseConfig {
    /// The database of `module` in `directory`, the file `<module>.db`.
    pub fn new(directory: impl AsRef<Path>, module: &str) -> Result<Self> {
        let valid = !module.is_empty()
            && module.len() <= 63
            && module
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !valid {
            return Err(PanelError::invalid_argument(format!(
                "{module:?} is not a module name: use lowercase letters, digits and hyphens"
            )));
        }
        Ok(Self::at(directory.as_ref().join(format!("{module}.db"))))
    }

    /// The database in the file at `path`.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_connections: 8,
            acquire_timeout: Duration::from_secs(5),
            busy_timeout: Duration::from_secs(5),
        }
    }

    pub fn with_max_connections(mut self, max_connections: u32) -> Self {
        self.max_connections = max_connections.max(1);
        self
    }

    pub fn with_acquire_timeout(mut self, timeout: Duration) -> Self {
        self.acquire_timeout = timeout;
        self
    }

    /// How long a statement waits for another connection's lock on the file.
    pub fn with_busy_timeout(mut self, timeout: Duration) -> Self {
        self.busy_timeout = timeout;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// One module's database file and its connection pool.
#[derive(Clone, Debug)]
pub struct ServiceDatabase {
    pool: SqlitePool,
    path: PathBuf,
    committed: Arc<Notify>,
}

impl ServiceDatabase {
    /// Creates the file, readable and writable by this user alone, and a
    /// pool that connects on first use. The write-ahead log and its index
    /// take the file's permissions.
    pub fn open(config: ServiceDatabaseConfig) -> Result<Self> {
        prepare(&config.path)?;
        let options = SqliteConnectOptions::new()
            .filename(&config.path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(config.busy_timeout)
            .optimize_on_close(true, None);
        let pool = SqlitePoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(config.acquire_timeout)
            .connect_lazy_with(options);
        Ok(Self {
            pool,
            path: config.path,
            committed: Arc::new(Notify::new()),
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Starts a transaction that writes. It takes the file's write lock at
    /// once, waiting for another writer up to the busy timeout, so it cannot
    /// fail later because a writer committed after it read.
    pub async fn begin(&self) -> Result<Transaction<'static, Sqlite>> {
        self.pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)
    }

    /// Wakes the outbox relay after a transaction that appended events
    /// commits; otherwise the relay finds them on its next look.
    pub fn committed(&self) {
        self.committed.notify_one();
    }

    pub(crate) fn commits(&self) -> Arc<Notify> {
        self.committed.clone()
    }

    /// Applies platform and service migrations in version order.
    pub async fn migrate(&self, service: &[SchemaMigration]) -> Result<()> {
        let mut versions = BTreeSet::new();
        for migration in PLATFORM_MIGRATIONS.iter().chain(service) {
            if !versions.insert(migration.version) {
                return Err(PanelError::invalid_argument(format!(
                    "migration version {} is declared twice",
                    migration.version
                )));
            }
        }
        if let Some(migration) = service
            .iter()
            .find(|migration| migration.version < SchemaMigration::SERVICE_VERSION_FLOOR)
        {
            return Err(PanelError::invalid_argument(format!(
                "service migration {} uses a platform version",
                migration.version
            )));
        }
        Migrator::with_migrations(
            PLATFORM_MIGRATIONS
                .iter()
                .chain(service)
                .map(|migration| migration.to_sqlx())
                .collect(),
        )
        .run(&self.pool)
        .await
        .map_err(|error| PanelError::internal(format!("migration failed: {error}")))
    }

    /// Closes the pool once every checked-out connection has been returned.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Creates the file's directory and the file itself for this user alone.
fn prepare(path: &Path) -> Result<()> {
    let unavailable = |error: std::io::Error| {
        PanelError::storage_unavailable(format!("cannot prepare {}: {error}", path.display()))
    };
    if let Some(directory) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(directory).map_err(unavailable)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path).map(drop).map_err(unavailable)
}
