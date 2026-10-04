#![forbid(unsafe_code)]

//! Idempotent provisioning of an installation: one login role and schema
//! per service in PostgreSQL, and the event streams and service registry in
//! JetStream. Rerunning it applies the current role passwords, so rotating
//! a secret is a rerun. Its `pki` mode creates the internal certificate
//! authority and keeps every service's mutual TLS credentials current.

mod pki;

pub use pki::{
    PkiPlan, CERTIFICATE_LIFETIME_MS_ENV, PKI_CHECK_INTERVAL_MS_ENV, PKI_CREDENTIALS_ENV,
    PKI_DIR_ENV,
};

use panel_control_runtime::NATS_URL_ENV;
use panel_errors::Result;
use panel_jetstream::{ensure_streams, JetStreamServiceRegistry, JetStreamSettings};
use panel_postgres::{DatabaseBootstrap, RoleSecret, ScramVerifier, ServiceRole, SqlIdentifier};
use panel_service::Environment;

/// The administrator connection; the role must own the product database.
pub const ADMIN_DATABASE_URL_ENV: &str = "PINGORA_PANEL_ADMIN_DATABASE_URL";
/// Also read from the file named by `PINGORA_PANEL_ADMIN_DATABASE_PASSWORD_FILE`.
pub const ADMIN_DATABASE_PASSWORD_ENV: &str = "PINGORA_PANEL_ADMIN_DATABASE_PASSWORD";
const DEFAULT_NATS_URL: &str = "nats://127.0.0.1:4222";

/// The schema of every service and the variable naming its role password.
pub const SERVICE_SCHEMAS: &[(&str, &str)] = &[
    ("identity", "PINGORA_PANEL_IDENTITY_DATABASE_PASSWORD"),
    ("config", "PINGORA_PANEL_CONFIG_DATABASE_PASSWORD"),
];

/// Prefix of the login role that owns each schema, as in `panel_config`.
pub const DEFAULT_ROLE_PREFIX: &str = "panel_";

/// What to provision.
pub struct Plan {
    admin_url: String,
    admin_password: Option<RoleSecret>,
    secrets: Vec<(&'static str, RoleSecret)>,
    role_prefix: String,
    nats_url: String,
    jetstream: JetStreamSettings,
}

impl Plan {
    /// Every service role password is required, directly or as a file.
    pub fn read(env: &mut Environment<'_>) -> Result<Self> {
        let mut secrets = Vec::with_capacity(SERVICE_SCHEMAS.len());
        for (schema, password_env) in SERVICE_SCHEMAS {
            let secret = env.secret(password_env)?.ok_or_else(|| {
                panel_errors::PanelError::invalid_argument(format!(
                    "{password_env} or {password_env}_FILE is required"
                ))
            })?;
            secrets.push((*schema, RoleSecret::new(secret)?));
        }
        Ok(Self {
            admin_url: env.required(ADMIN_DATABASE_URL_ENV)?,
            admin_password: env
                .secret(ADMIN_DATABASE_PASSWORD_ENV)?
                .map(RoleSecret::new)
                .transpose()?,
            secrets,
            role_prefix: DEFAULT_ROLE_PREFIX.into(),
            nats_url: env
                .string(NATS_URL_ENV)?
                .unwrap_or_else(|| DEFAULT_NATS_URL.into()),
            jetstream: JetStreamSettings::default(),
        })
    }

    pub fn with_jetstream_settings(mut self, settings: JetStreamSettings) -> Self {
        self.jetstream = settings;
        self
    }

    /// Distinguishes installations sharing one PostgreSQL cluster, where
    /// role names are global.
    pub fn with_role_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.role_prefix = prefix.into();
        self
    }

    /// The login role that owns `schema`.
    pub fn role_name(&self, schema: &str) -> Result<SqlIdentifier> {
        SqlIdentifier::new(format!("{}{schema}", self.role_prefix))
    }

    pub async fn apply(&self) -> Result<()> {
        let mut database = DatabaseBootstrap::new();
        for (schema, secret) in &self.secrets {
            database = database.with_service(ServiceRole::new(
                self.role_name(schema)?,
                SqlIdentifier::new(*schema)?,
                ScramVerifier::derive(secret)?,
            ));
        }
        database
            .apply_at(&self.admin_url, self.admin_password.as_ref())
            .await?;
        tracing::info!(
            schemas = SERVICE_SCHEMAS.len(),
            "database roles and schemas are provisioned"
        );
        let client = async_nats::connect(self.nats_url.as_str())
            .await
            .map_err(|error| {
                panel_errors::PanelError::unavailable(format!("broker connection failed: {error}"))
            })?;
        let context = async_nats::jetstream::new(client);
        ensure_streams(&context, &self.jetstream).await?;
        JetStreamServiceRegistry::provision(
            &context,
            &self.jetstream,
            JetStreamServiceRegistry::DEFAULT_TTL,
        )
        .await?;
        tracing::info!("event streams and the service registry are provisioned");
        Ok(())
    }
}
