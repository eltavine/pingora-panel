use crate::{storage_error, RoleSecret, SqlIdentifier};
use panel_errors::{PanelError, Result};
use sqlx::{
    migrate::{Migration, MigrationType, Migrator},
    postgres::{PgConnectOptions, PgPool, PgPoolOptions},
    SqlSafeStr,
};
use std::{borrow::Cow, collections::BTreeSet, str::FromStr, time::Duration};

/// One forward-only schema migration. Platform migrations occupy versions
/// below [`SchemaMigration::SERVICE_VERSION_FLOOR`]; service migrations use
/// versions at or above it so both sets apply in one ordered history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaMigration {
    version: i64,
    description: &'static str,
    sql: &'static str,
}

impl SchemaMigration {
    pub const SERVICE_VERSION_FLOOR: i64 = 10_000;

    pub const fn new(version: i64, description: &'static str, sql: &'static str) -> Self {
        Self {
            version,
            description,
            sql,
        }
    }

    pub const fn version(&self) -> i64 {
        self.version
    }

    fn to_sqlx(self) -> Migration {
        Migration::new(
            self.version,
            Cow::Borrowed(self.description),
            MigrationType::Simple,
            self.sql.into_sql_str(),
            false,
        )
    }
}

/// Tables every service schema carries, such as the outbox and inbox.
const PLATFORM_MIGRATIONS: &[SchemaMigration] = &[SchemaMigration::new(
    1,
    "transactional outbox",
    include_str!("../migrations/0001_outbox.sql"),
)];

/// Connection settings for one service role.
#[derive(Clone, Debug)]
pub struct ServiceDatabaseConfig {
    connect: PgConnectOptions,
    schema: SqlIdentifier,
    max_connections: u32,
    acquire_timeout: Duration,
    statement_timeout: Duration,
    idle_in_transaction_timeout: Duration,
}

impl ServiceDatabaseConfig {
    /// `url` names the host, database and service role; the password is
    /// supplied separately so it can come from a secret store.
    pub fn new(url: &str, application: &str, schema: SqlIdentifier) -> Result<Self> {
        let connect = PgConnectOptions::from_str(url)
            .map_err(|_| PanelError::invalid_argument("database URL is not a PostgreSQL URL"))?
            .application_name(application);
        Ok(Self {
            connect,
            schema,
            max_connections: 16,
            acquire_timeout: Duration::from_secs(5),
            statement_timeout: Duration::from_secs(30),
            idle_in_transaction_timeout: Duration::from_secs(60),
        })
    }

    pub fn with_secret(mut self, secret: &RoleSecret) -> Self {
        self.connect = self.connect.password(secret.expose());
        self
    }

    pub fn with_max_connections(mut self, max_connections: u32) -> Self {
        self.max_connections = max_connections.max(1);
        self
    }

    pub fn with_acquire_timeout(mut self, timeout: Duration) -> Self {
        self.acquire_timeout = timeout;
        self
    }

    pub fn with_statement_timeout(mut self, timeout: Duration) -> Self {
        self.statement_timeout = timeout;
        self
    }

    pub fn with_idle_in_transaction_timeout(mut self, timeout: Duration) -> Self {
        self.idle_in_transaction_timeout = timeout;
        self
    }
}

/// A connection pool bound to one service role and schema.
#[derive(Clone, Debug)]
pub struct ServiceDatabase {
    pool: PgPool,
    schema: SqlIdentifier,
}

impl ServiceDatabase {
    /// Connects and verifies the session resolves unqualified names to the
    /// service schema only.
    pub async fn connect(config: ServiceDatabaseConfig) -> Result<Self> {
        let schema = config.schema.clone();
        let connect = config.connect.options([
            ("search_path", schema.quoted()),
            (
                "statement_timeout",
                config.statement_timeout.as_millis().to_string(),
            ),
            (
                "idle_in_transaction_session_timeout",
                config.idle_in_transaction_timeout.as_millis().to_string(),
            ),
        ]);
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(config.acquire_timeout)
            .connect_with(connect)
            .await
            .map_err(storage_error)?;
        let current: Option<String> = sqlx::query_scalar("SELECT current_schema()::text")
            .fetch_one(&pool)
            .await
            .map_err(storage_error)?;
        if current.as_deref() != Some(schema.as_str()) {
            pool.close().await;
            return Err(PanelError::precondition_failed(format!(
                "service schema {schema} is not available to this role"
            )));
        }
        Ok(Self { pool, schema })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn schema(&self) -> &SqlIdentifier {
        &self.schema
    }

    /// Applies platform and service migrations in version order. History is
    /// recorded in the service schema, so services migrate independently.
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
        let mut migrator = Migrator::with_migrations(
            PLATFORM_MIGRATIONS
                .iter()
                .chain(service)
                .map(|migration| migration.to_sqlx())
                .collect(),
        );
        migrator.dangerous_set_table_name(format!("{}._sqlx_migrations", self.schema.quoted()));
        migrator
            .run(&self.pool)
            .await
            .map_err(|error| PanelError::internal(format!("migration failed: {error}")))
    }

    /// Closes the pool once every checked-out connection, including relay
    /// leadership and commit listeners, has been returned.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
