//! DNS providers that publish DNS-01 records for automatic certificates.
//! Their settings are public; their secrets are sealed with the master keys
//! and never returned.

use crate::{acme::slug, certificates::Cause};
use chrono::{DateTime, Utc};
use dns_rfc2136::{Algorithm, Rfc2136, Rfc2136Settings};
use panel_acme::{Dns01, DnsProvider};
use panel_errors::{PanelError, Result};
use panel_postgres::{storage_error, EventLog, PgOutbox, ServiceDatabase};
use panel_secrets::{Sealed, SecretVault};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, PgConnection, PgPool, Row};
use std::{sync::Arc, time::Duration};
use zeroize::Zeroizing;

const AGGREGATE: &str = "dns_provider";
const MAX_PROPAGATION_SECONDS: u32 = 3600;

fn default_propagation() -> u32 {
    30
}

/// Where and how an RFC 2136 provider sends updates.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rfc2136Config {
    /// The primary server as `host:port`.
    pub server: String,
    /// The zones the key may update.
    pub zones: Vec<String>,
    pub key_name: String,
    /// `hmac-sha256` or `hmac-sha512`.
    pub algorithm: String,
    #[serde(default)]
    pub ttl: Option<u32>,
}

/// A DNS provider as kept: its kind and settings, without its secret.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DnsProviderRecord {
    pub id: String,
    pub kind: String,
    pub rfc2136: Rfc2136Config,
    /// Seconds to wait for a record to reach every authoritative server.
    pub propagation_seconds: u32,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DnsProviderRecord {
    pub fn etag(&self) -> String {
        format!("\"{}\"", self.version)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewDnsProvider {
    pub id: String,
    /// Only `rfc2136` so far.
    pub kind: String,
    pub rfc2136: Rfc2136Config,
    /// The TSIG secret, base64-encoded.
    pub secret: Zeroizing<String>,
    #[serde(default = "default_propagation")]
    pub propagation_seconds: u32,
}

/// New settings; the secret stays unless a new one is given.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnsProviderChange {
    pub rfc2136: Rfc2136Config,
    #[serde(default)]
    pub secret: Option<Zeroizing<String>>,
    #[serde(default = "default_propagation")]
    pub propagation_seconds: u32,
}

/// Builds the provider a stored kind and settings describe.
pub trait DnsProviderFactory: Send + Sync {
    fn build(&self, kind: &str, settings: &Value, secret: &str) -> Result<Arc<dyn DnsProvider>>;
}

/// The providers this build includes.
#[derive(Clone, Copy, Debug, Default)]
pub struct StandardDnsProviders;

impl DnsProviderFactory for StandardDnsProviders {
    fn build(&self, kind: &str, settings: &Value, secret: &str) -> Result<Arc<dyn DnsProvider>> {
        match kind {
            "rfc2136" => {
                let config: Rfc2136Config =
                    serde_json::from_value(settings.clone()).map_err(|error| {
                        PanelError::invalid_argument(format!("invalid RFC 2136 settings: {error}"))
                    })?;
                Ok(Arc::new(Rfc2136::new(Rfc2136Settings {
                    server: config.server,
                    zones: config.zones,
                    key_name: config.key_name,
                    algorithm: Algorithm::parse(&config.algorithm)?,
                    secret: Zeroizing::new(secret.to_owned()),
                    ttl: config.ttl,
                })?))
            }
            other => Err(PanelError::invalid_argument(format!(
                "{other:?} is not a DNS provider kind: use rfc2136"
            ))),
        }
    }
}

macro_rules! columns {
    () => {
        "provider_id, kind, settings::text AS settings, propagation_seconds, version, created_at, \
         updated_at"
    };
}

fn corrupt(what: &str) -> PanelError {
    PanelError::corrupt_state(format!("a stored DNS provider has an invalid {what}"))
}

fn record(row: &PgRow) -> Result<DnsProviderRecord> {
    let settings: String = row.try_get("settings").map_err(storage_error)?;
    let propagation: i32 = row.try_get("propagation_seconds").map_err(storage_error)?;
    let version: i64 = row.try_get("version").map_err(storage_error)?;
    Ok(DnsProviderRecord {
        id: row.try_get("provider_id").map_err(storage_error)?,
        kind: row.try_get("kind").map_err(storage_error)?,
        rfc2136: serde_json::from_str(&settings).map_err(|_| corrupt("settings"))?,
        propagation_seconds: u32::try_from(propagation).map_err(|_| corrupt("propagation"))?,
        version: u64::try_from(version).map_err(|_| corrupt("version"))?,
        created_at: row.try_get("created_at").map_err(storage_error)?,
        updated_at: row.try_get("updated_at").map_err(storage_error)?,
    })
}

fn owner(id: &str) -> String {
    format!("dns-provider/{id}/secret")
}

fn check_propagation(seconds: u32) -> Result<()> {
    if seconds > MAX_PROPAGATION_SECONDS {
        return Err(PanelError::invalid_argument(format!(
            "records propagate within at most {MAX_PROPAGATION_SECONDS} seconds"
        )));
    }
    Ok(())
}

/// DNS providers in the service schema. Changes write their
/// `tls.acme.dns_provider.*` events in the same transaction.
#[derive(Clone)]
pub struct DnsProviders {
    pool: PgPool,
    events: EventLog,
    vault: Option<Arc<dyn SecretVault>>,
    factory: Arc<dyn DnsProviderFactory>,
}

impl DnsProviders {
    pub fn new(
        database: &ServiceDatabase,
        events: EventLog,
        vault: Option<Arc<dyn SecretVault>>,
        factory: Arc<dyn DnsProviderFactory>,
    ) -> Self {
        Self {
            pool: database.pool().clone(),
            events,
            vault,
            factory,
        }
    }

    fn vault(&self) -> Result<&dyn SecretVault> {
        self.vault.as_deref().ok_or_else(|| {
            PanelError::unavailable("DNS providers cannot be kept until master keys are configured")
        })
    }

    pub async fn list(&self) -> Result<Vec<DnsProviderRecord>> {
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM dns_providers ORDER BY provider_id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?
        .iter()
        .map(record)
        .collect()
    }

    pub async fn get(&self, id: &str) -> Result<DnsProviderRecord> {
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM dns_providers WHERE provider_id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .map(|row| record(&row))
        .transpose()?
        .ok_or_else(|| PanelError::not_found(format!("there is no DNS provider {id}")))
    }

    pub async fn create(
        &self,
        cause: Cause<'_>,
        body: NewDnsProvider,
    ) -> Result<DnsProviderRecord> {
        let id = body.id.clone();
        let result = async {
            let id = slug(&body.id, "a DNS provider ID")?;
            check_propagation(body.propagation_seconds)?;
            let settings = serde_json::to_value(&body.rfc2136)
                .map_err(|_| PanelError::internal("DNS provider settings do not serialize"))?;
            self.factory.build(&body.kind, &settings, &body.secret)?;
            let sealed = self.vault()?.seal(&owner(&id), body.secret.as_bytes()).await?;
            let now = Utc::now();
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            let inserted = sqlx::query(
                "INSERT INTO dns_providers (provider_id, kind, settings, sealed_secret, \
                 propagation_seconds, version, created_at, updated_at) \
                 VALUES ($1, $2, $3::jsonb, $4, $5, 1, $6, $6) ON CONFLICT (provider_id) DO NOTHING",
            )
            .bind(&id)
            .bind(&body.kind)
            .bind(settings.to_string())
            .bind(sealed.as_str())
            .bind(i32::try_from(body.propagation_seconds).unwrap_or(0))
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?
            .rows_affected();
            if inserted == 0 {
                return Err(PanelError::conflict(format!("DNS provider {id} already exists")));
            }
            let created = DnsProviderRecord {
                id: id.clone(),
                kind: body.kind,
                rfc2136: body.rfc2136,
                propagation_seconds: body.propagation_seconds,
                version: 1,
                created_at: now,
                updated_at: now,
            };
            self.publish(&mut transaction, cause, "tls.acme.dns_provider.created", &created)
                .await?;
            transaction.commit().await.map_err(storage_error)?;
            Ok(created)
        }
        .await;
        self.refused(cause, &id, "create", result).await
    }

    pub async fn update(
        &self,
        cause: Cause<'_>,
        id: &str,
        expected: Option<u64>,
        body: DnsProviderChange,
    ) -> Result<DnsProviderRecord> {
        let result = async {
            check_propagation(body.propagation_seconds)?;
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            let row = sqlx::query(concat!(
                "SELECT ",
                columns!(),
                ", sealed_secret FROM dns_providers WHERE provider_id = $1 FOR UPDATE"
            ))
            .bind(id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| PanelError::not_found(format!("there is no DNS provider {id}")))?;
            let current = record(&row)?;
            if let Some(expected) = expected.filter(|expected| *expected != current.version) {
                return Err(PanelError::precondition_failed(format!(
                    "DNS provider {id} has changed; it is at version {}, not {expected}",
                    current.version
                )));
            }
            let vault = self.vault()?;
            let secret = match body.secret {
                Some(secret) => secret,
                None => {
                    let sealed = Sealed::new(
                        row.try_get::<String, _>("sealed_secret")
                            .map_err(storage_error)?,
                    );
                    let opened = vault.open(&owner(id), &sealed).await?;
                    Zeroizing::new(
                        String::from_utf8(opened.to_vec()).map_err(|_| corrupt("secret"))?,
                    )
                }
            };
            let settings = serde_json::to_value(&body.rfc2136)
                .map_err(|_| PanelError::internal("DNS provider settings do not serialize"))?;
            self.factory.build(&current.kind, &settings, &secret)?;
            let sealed = vault.seal(&owner(id), secret.as_bytes()).await?;
            let updated = DnsProviderRecord {
                rfc2136: body.rfc2136,
                propagation_seconds: body.propagation_seconds,
                version: current.version + 1,
                updated_at: Utc::now(),
                ..current
            };
            sqlx::query(
                "UPDATE dns_providers SET settings = $2::jsonb, sealed_secret = $3, \
                 propagation_seconds = $4, version = $5, updated_at = $6 WHERE provider_id = $1",
            )
            .bind(id)
            .bind(settings.to_string())
            .bind(sealed.as_str())
            .bind(i32::try_from(updated.propagation_seconds).unwrap_or(0))
            .bind(i64::try_from(updated.version).unwrap_or(i64::MAX))
            .bind(updated.updated_at)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            self.publish(
                &mut transaction,
                cause,
                "tls.acme.dns_provider.updated",
                &updated,
            )
            .await?;
            transaction.commit().await.map_err(storage_error)?;
            Ok(updated)
        }
        .await;
        self.refused(cause, id, "update", result).await
    }

    /// Forgets a provider no automatic certificate uses.
    pub async fn delete(&self, cause: Cause<'_>, id: &str, expected: Option<u64>) -> Result<()> {
        let result = async {
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            let version: Option<i64> = sqlx::query_scalar(
                "SELECT version FROM dns_providers WHERE provider_id = $1 FOR UPDATE",
            )
            .bind(id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage_error)?;
            let version = version
                .and_then(|version| u64::try_from(version).ok())
                .ok_or_else(|| PanelError::not_found(format!("there is no DNS provider {id}")))?;
            if let Some(expected) = expected.filter(|expected| *expected != version) {
                return Err(PanelError::precondition_failed(format!(
                    "DNS provider {id} has changed; it is at version {version}, not {expected}"
                )));
            }
            let users: Vec<String> = sqlx::query_scalar(
                "SELECT certificate_id FROM acme_certificates WHERE dns_provider_id = $1 \
                 ORDER BY certificate_id",
            )
            .bind(id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(storage_error)?;
            if !users.is_empty() {
                return Err(PanelError::conflict(format!(
                    "DNS provider {id} still publishes records for {}",
                    users.join(", ")
                )));
            }
            sqlx::query("DELETE FROM dns_providers WHERE provider_id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
            let event = self.events.event_named_by(
                "tls.acme.dns_provider.deleted",
                (AGGREGATE, id),
                cause.scope,
                cause.principal,
                &json!({ "id": id }),
            )?;
            PgOutbox::append(&mut transaction, &event).await?;
            transaction.commit().await.map_err(storage_error)
        }
        .await;
        self.refused(cause, id, "delete", result).await
    }

    /// The DNS-01 solver of a provider.
    pub(crate) async fn solver(&self, id: &str) -> Result<Dns01> {
        let row = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            ", sealed_secret FROM dns_providers WHERE provider_id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| PanelError::not_found(format!("there is no DNS provider {id}")))?;
        let current = record(&row)?;
        let sealed = Sealed::new(
            row.try_get::<String, _>("sealed_secret")
                .map_err(storage_error)?,
        );
        let opened = self.vault()?.open(&owner(id), &sealed).await?;
        let secret =
            Zeroizing::new(String::from_utf8(opened.to_vec()).map_err(|_| corrupt("secret"))?);
        let settings = serde_json::to_value(&current.rfc2136)
            .map_err(|_| PanelError::internal("DNS provider settings do not serialize"))?;
        let provider = self.factory.build(&current.kind, &settings, &secret)?;
        Ok(Dns01::new(
            provider,
            Duration::from_secs(u64::from(current.propagation_seconds)),
        ))
    }

    async fn publish(
        &self,
        connection: &mut PgConnection,
        cause: Cause<'_>,
        event_type: &str,
        provider: &DnsProviderRecord,
    ) -> Result<()> {
        let event = self.events.event_named_by(
            event_type,
            (AGGREGATE, &provider.id),
            cause.scope,
            cause.principal,
            &json!({
                "id": provider.id,
                "kind": provider.kind,
                "server": provider.rfc2136.server,
                "zones": provider.rfc2136.zones,
                "key_name": provider.rfc2136.key_name,
                "version": provider.version,
            }),
        )?;
        PgOutbox::append(connection, &event).await
    }

    async fn refused<T>(
        &self,
        cause: Cause<'_>,
        id: &str,
        operation: &str,
        result: Result<T>,
    ) -> Result<T> {
        if let Err(error) = &result {
            self.events
                .record_named_by(
                    "tls.acme.dns_provider.refused",
                    (AGGREGATE, id),
                    cause.scope,
                    cause.principal,
                    &json!({
                        "id": id,
                        "operation": operation,
                        "code": error.code.as_str(),
                        "message": error.message,
                    }),
                )
                .await;
        }
        result
    }
}
