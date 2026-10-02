//! Disposable databases for integration tests.
//!
//! Tests run against the server named by [`DATABASE_URL_ENV`], connecting as
//! a role allowed to create databases and roles. Without it they are skipped,
//! unless [`REQUIRE_ENV`] is set, in which case a missing server fails the
//! test so CI cannot silently lose coverage.

use crate::{
    storage_error, DatabaseBootstrap, RoleSecret, ScramVerifier, ServiceDatabase,
    ServiceDatabaseConfig, ServiceRole, SqlIdentifier,
};
use sqlx::{
    postgres::{PgConnectOptions, PgConnection},
    AssertSqlSafe, ConnectOptions,
};
use std::str::FromStr;

pub const DATABASE_URL_ENV: &str = "PANEL_TEST_DATABASE_URL";
pub const REQUIRE_ENV: &str = "PANEL_REQUIRE_INTEGRATION_SERVICES";

/// A freshly created database plus the roles bootstrapped into it.
pub struct TestDatabase {
    admin: PgConnectOptions,
    name: SqlIdentifier,
    prefix: String,
    roles: Vec<SqlIdentifier>,
}

impl TestDatabase {
    pub async fn create() -> Option<Self> {
        let Ok(url) = std::env::var(DATABASE_URL_ENV) else {
            assert!(
                std::env::var_os(REQUIRE_ENV).is_none(),
                "{REQUIRE_ENV} is set but {DATABASE_URL_ENV} is not"
            );
            eprintln!("skipping: {DATABASE_URL_ENV} is not set");
            return None;
        };
        let admin = PgConnectOptions::from_str(&url).expect("test database URL is valid");
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = SqlIdentifier::new(format!("panel_test_{suffix}")).unwrap();
        let mut connection = admin.connect().await.expect("test server is reachable");
        sqlx::raw_sql(AssertSqlSafe(format!("CREATE DATABASE {}", name.quoted())))
            .execute(&mut connection)
            .await
            .expect("test database can be created");
        Some(Self {
            admin,
            name,
            prefix: format!("t{}", &suffix[..10]),
            roles: Vec::new(),
        })
    }

    /// A cluster-unique role name; roles outlive databases, so tests must
    /// not share them.
    pub fn role_name(&self, service: &str) -> SqlIdentifier {
        SqlIdentifier::new(format!("{}_{service}", self.prefix)).unwrap()
    }

    pub async fn admin_connection(&self) -> PgConnection {
        self.admin
            .clone()
            .database(self.name.as_str())
            .connect()
            .await
            .expect("test database accepts the administrator")
    }

    /// Bootstraps one role and schema per service and returns their secrets.
    pub async fn bootstrap(&mut self, services: &[(&str, &str)]) -> Vec<RoleSecret> {
        let mut bootstrap = DatabaseBootstrap::new();
        let mut secrets = Vec::new();
        for (service, schema) in services {
            let role = self.role_name(service);
            let secret = RoleSecret::generate().unwrap();
            bootstrap = bootstrap.with_service(ServiceRole::new(
                role.clone(),
                SqlIdentifier::new(*schema).unwrap(),
                ScramVerifier::derive(&secret).unwrap(),
            ));
            self.roles.push(role);
            secrets.push(secret);
        }
        bootstrap
            .apply(&mut self.admin_connection().await)
            .await
            .expect("bootstrap succeeds");
        secrets
    }

    /// The URL of a bootstrapped service role, without its password.
    pub fn service_url(&self, service: &str) -> String {
        format!(
            "postgres://{}@{}:{}/{}",
            self.role_name(service),
            self.admin.get_host(),
            self.admin.get_port(),
            self.name
        )
    }

    /// Connection settings for a bootstrapped service role.
    pub fn service_config(
        &self,
        service: &str,
        schema: &str,
        secret: &RoleSecret,
    ) -> ServiceDatabaseConfig {
        let url = self.service_url(service);
        ServiceDatabaseConfig::new(&url, service, SqlIdentifier::new(schema).unwrap())
            .unwrap()
            .with_secret(secret)
            .with_max_connections(4)
    }

    pub async fn connect_service(
        &self,
        service: &str,
        schema: &str,
        secret: &RoleSecret,
    ) -> ServiceDatabase {
        ServiceDatabase::connect(self.service_config(service, schema, secret))
            .await
            .expect("service role can connect")
    }

    /// Drops the database and every role created for it.
    pub async fn drop(self) {
        let mut connection = self
            .admin
            .connect()
            .await
            .expect("test server is reachable");
        let mut statements = vec![format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name.quoted()
        )];
        statements.extend(
            self.roles
                .iter()
                .map(|role| format!("DROP ROLE IF EXISTS {}", role.quoted())),
        );
        for statement in statements {
            sqlx::raw_sql(AssertSqlSafe(statement))
                .execute(&mut connection)
                .await
                .map_err(storage_error)
                .expect("test cleanup succeeds");
        }
    }
}
