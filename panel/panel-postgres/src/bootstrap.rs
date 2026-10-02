use crate::{storage_error, ScramVerifier, SqlIdentifier};
use panel_errors::{PanelError, Result};
use sqlx::{postgres::PgConnection, AssertSqlSafe, Connection};
use std::collections::BTreeSet;

/// One service's database identity: a login role that owns one schema.
#[derive(Clone, Debug)]
pub struct ServiceRole {
    role: SqlIdentifier,
    schema: SqlIdentifier,
    verifier: ScramVerifier,
    connection_limit: u16,
}

impl ServiceRole {
    pub fn new(role: SqlIdentifier, schema: SqlIdentifier, verifier: ScramVerifier) -> Self {
        Self {
            role,
            schema,
            verifier,
            connection_limit: 32,
        }
    }

    pub fn with_connection_limit(mut self, limit: u16) -> Self {
        self.connection_limit = limit;
        self
    }

    pub fn role(&self) -> &SqlIdentifier {
        &self.role
    }

    pub fn schema(&self) -> &SqlIdentifier {
        &self.schema
    }
}

/// Idempotent ownership setup run once per installation or upgrade by a
/// privileged administrator connected to the product database.
///
/// The administrator must own the database; a service role owning it would
/// make every schema usage pattern insecure.
#[derive(Clone, Debug, Default)]
pub struct DatabaseBootstrap {
    services: Vec<ServiceRole>,
}

impl DatabaseBootstrap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_service(mut self, service: ServiceRole) -> Self {
        self.services.push(service);
        self
    }

    pub async fn apply(&self, admin: &mut PgConnection) -> Result<()> {
        let roles = self
            .services
            .iter()
            .map(|service| service.role.as_str())
            .collect::<BTreeSet<_>>();
        let schemas = self
            .services
            .iter()
            .map(|service| service.schema.as_str())
            .collect::<BTreeSet<_>>();
        if roles.len() != self.services.len() || schemas.len() != self.services.len() {
            return Err(PanelError::invalid_argument(
                "each service needs its own role and schema",
            ));
        }
        let mut transaction = admin.begin().await.map_err(storage_error)?;
        // Concurrent bootstraps of the same database serialize here.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('pingora-panel.bootstrap', 0))")
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        let (database, owner): (String, String) = sqlx::query_as(
            "SELECT current_database()::text, pg_get_userbyid(datdba)::text \
             FROM pg_database WHERE datname = current_database()",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if roles.contains(owner.as_str()) {
            return Err(PanelError::precondition_failed(
                "a service role must not own the product database",
            ));
        }
        let database = SqlIdentifier::new(database)?;
        let mut statements = vec![
            format!("REVOKE ALL ON DATABASE {} FROM PUBLIC", database.quoted()),
            "REVOKE ALL ON SCHEMA public FROM PUBLIC".to_owned(),
        ];
        for service in &self.services {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1)")
                    .bind(service.role.as_str())
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(storage_error)?;
            let role = service.role.quoted();
            let schema = service.schema.quoted();
            if !exists {
                statements.push(format!("CREATE ROLE {role}"));
            }
            statements.extend([
                format!(
                    "ALTER ROLE {role} WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT \
                     NOREPLICATION NOBYPASSRLS CONNECTION LIMIT {} PASSWORD '{}'",
                    service.connection_limit,
                    service.verifier.as_str()
                ),
                format!("GRANT CONNECT ON DATABASE {} TO {role}", database.quoted()),
                format!("CREATE SCHEMA IF NOT EXISTS {schema} AUTHORIZATION {role}"),
                format!("ALTER SCHEMA {schema} OWNER TO {role}"),
                format!("REVOKE ALL ON SCHEMA {schema} FROM PUBLIC"),
                format!(
                    "ALTER ROLE {role} IN DATABASE {} SET search_path = {schema}",
                    database.quoted()
                ),
            ]);
        }
        for statement in statements {
            sqlx::raw_sql(AssertSqlSafe(statement))
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
        }
        transaction.commit().await.map_err(storage_error)
    }
}
