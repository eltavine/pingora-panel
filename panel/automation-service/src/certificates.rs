//! The certificate inventory.

use crate::delivery::{Delivery, SecretDirectory};
use chrono::{DateTime, Duration, Utc};
use panel_certificates::{
    accept, self_signed, Accepted, Certificate, CertificateDetails, CertificateId,
    CertificateSource,
};
use panel_errors::{PanelError, Result};
use panel_events::{Principal, RequestScope};
use panel_postgres::{storage_error, EventLog, PgOutbox, ServiceDatabase};
use panel_secrets::{Sealed, SecretVault};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, PgConnection, PgPool, Row};
use std::sync::Arc;
use zeroize::Zeroizing;

const AGGREGATE: &str = "certificate";

/// The columns read into a [`Certificate`], as a literal so queries stay static.
macro_rules! columns {
    () => {
        "certificate_id, source, details::text AS details, chain, version, created_at, updated_at"
    };
}

/// Days before its end at which a certificate is announced as expiring.
const REMINDERS: [i64; 5] = [30, 14, 7, 3, 1];

/// Who asked for a change.
#[derive(Clone, Copy)]
pub struct Cause<'a> {
    pub scope: &'a RequestScope,
    pub principal: &'a Principal,
}

/// The smallest reminder threshold, in days, that `left` is within; 0 once
/// it has run out, `None` while no threshold is reached.
fn reminder(left: Duration) -> Option<i64> {
    if left <= Duration::zero() {
        return Some(0);
    }
    REMINDERS
        .iter()
        .rev()
        .copied()
        .find(|days| left <= Duration::days(*days))
}

/// Certificates with their chains and sealed keys in the service schema.
///
/// Every change writes its `tls.certificate.*` event in the same
/// transaction and is then delivered to the gateway's secret directory;
/// refused changes are recorded as `tls.certificate.refused`.
#[derive(Clone)]
pub struct CertificateInventory {
    pool: PgPool,
    events: EventLog,
    vault: Option<Arc<dyn SecretVault>>,
    directory: Option<SecretDirectory>,
}

/// The owner a certificate's key is sealed for.
fn owner(id: &CertificateId) -> String {
    format!("certificate/{id}/key")
}

fn stored_id(row: &PgRow) -> Result<CertificateId> {
    let id: String = row.try_get("certificate_id").map_err(storage_error)?;
    CertificateId::new(id)
        .map_err(|_| PanelError::corrupt_state("a stored certificate has an invalid id"))
}

fn certificate(row: &PgRow) -> Result<Certificate> {
    let source: String = row.try_get("source").map_err(storage_error)?;
    let details: String = row.try_get("details").map_err(storage_error)?;
    let version: i64 = row.try_get("version").map_err(storage_error)?;
    let corrupt = |what: &str| {
        PanelError::corrupt_state(format!("a stored certificate has an invalid {what}"))
    };
    Ok(Certificate {
        id: stored_id(row)?,
        source: serde_json::from_value(Value::String(source)).map_err(|_| corrupt("source"))?,
        details: serde_json::from_str::<CertificateDetails>(&details)
            .map_err(|_| corrupt("description"))?,
        chain: row.try_get("chain").map_err(storage_error)?,
        version: u64::try_from(version).map_err(|_| corrupt("version"))?,
        created_at: row.try_get("created_at").map_err(storage_error)?,
        updated_at: row.try_get("updated_at").map_err(storage_error)?,
    })
}

fn details(details: &CertificateDetails) -> Result<String> {
    serde_json::to_string(details)
        .map_err(|_| PanelError::internal("a certificate description does not serialize"))
}

fn source_name(source: CertificateSource) -> Result<String> {
    match serde_json::to_value(source) {
        Ok(Value::String(name)) => Ok(name),
        _ => Err(PanelError::internal("a certificate source has no name")),
    }
}

fn summary(certificate: &Certificate) -> Value {
    json!({
        "id": certificate.id,
        "source": certificate.source,
        "names": certificate.details.names,
        "not_after": certificate.details.not_after,
        "fingerprint": certificate.details.fingerprint,
        "version": certificate.version,
    })
}

/// Fails unless `expected`, an entity tag, names the current version.
fn check_version(id: &CertificateId, current: u64, expected: Option<u64>) -> Result<()> {
    match expected {
        Some(expected) if expected != current => Err(PanelError::precondition_failed(format!(
            "certificate {id} has changed; it is at version {current}"
        ))),
        _ => Ok(()),
    }
}

impl CertificateInventory {
    pub fn new(
        database: &ServiceDatabase,
        events: EventLog,
        vault: Option<Arc<dyn SecretVault>>,
        directory: Option<SecretDirectory>,
    ) -> Self {
        Self {
            pool: database.pool().clone(),
            events,
            vault,
            directory,
        }
    }

    fn vault(&self) -> Result<&dyn SecretVault> {
        self.vault.as_deref().ok_or_else(|| {
            PanelError::unavailable(
                "certificates cannot be stored until master keys are configured",
            )
        })
    }

    pub async fn list(&self) -> Result<Vec<Certificate>> {
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM certificates ORDER BY certificate_id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?
        .iter()
        .map(certificate)
        .collect()
    }

    pub async fn get(&self, id: &CertificateId) -> Result<Certificate> {
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM certificates WHERE certificate_id = $1"
        ))
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .map(|row| certificate(&row))
        .transpose()?
        .ok_or_else(|| PanelError::not_found(format!("there is no certificate {id}")))
    }

    /// Adds an uploaded certificate. Uploading the same certificate under
    /// the same ID again returns the stored one, so retries are safe.
    pub async fn upload(
        &self,
        cause: Cause<'_>,
        id: CertificateId,
        chain: &str,
        key: &str,
    ) -> Result<Certificate> {
        let result = async {
            let accepted = accept(chain, key, Utc::now())?;
            if let Ok(existing) = self.get(&id).await {
                if existing.source == CertificateSource::Uploaded
                    && existing.details.fingerprint == accepted.details.fingerprint
                {
                    return Ok(existing);
                }
            }
            self.create(cause, id.clone(), CertificateSource::Uploaded, accepted)
                .await
        }
        .await;
        self.refused(cause, "upload", &id, result).await
    }

    /// Adds a self-signed certificate for `names`, valid for `days`.
    pub async fn generate(
        &self,
        cause: Cause<'_>,
        id: CertificateId,
        names: &[String],
        days: u32,
    ) -> Result<Certificate> {
        let result = async {
            let accepted = self_signed(names, days, Utc::now())?;
            self.create(cause, id.clone(), CertificateSource::SelfSigned, accepted)
                .await
        }
        .await;
        self.refused(cause, "generate", &id, result).await
    }

    /// Replaces the chain and key of a certificate, for example with a
    /// renewed one; the certificate keeps its ID and references.
    pub async fn replace(
        &self,
        cause: Cause<'_>,
        id: CertificateId,
        expected: Option<u64>,
        chain: &str,
        key: &str,
    ) -> Result<Certificate> {
        let result = async {
            let accepted = accept(chain, key, Utc::now())?;
            self.store_replacement(cause, &id, expected, CertificateSource::Uploaded, accepted)
                .await
        }
        .await;
        self.refused(cause, "replace", &id, result).await
    }

    /// Stores a certificate an ACME CA issued under `id`, creating it or
    /// replacing what is there.
    pub async fn store_issued(
        &self,
        cause: Cause<'_>,
        id: &CertificateId,
        accepted: Accepted,
    ) -> Result<Certificate> {
        match self.get(id).await {
            Ok(_) => {
                self.store_replacement(cause, id, None, CertificateSource::Acme, accepted)
                    .await
            }
            Err(error) if error.code.as_str() == "NOT_FOUND" => {
                self.create(cause, id.clone(), CertificateSource::Acme, accepted)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    /// The chain and source of a certificate, if there is one.
    pub async fn chain(&self, id: &CertificateId) -> Result<Option<(CertificateSource, String)>> {
        match self.get(id).await {
            Ok(certificate) => Ok(Some((certificate.source, certificate.chain))),
            Err(error) if error.code.as_str() == "NOT_FOUND" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Announces certificates that came within a reminder threshold of
    /// their end, or ran out, as `tls.certificate.expiring`, once per
    /// threshold and version; returns how many were announced.
    pub async fn remind_expiring(&self, cause: Cause<'_>, now: DateTime<Utc>) -> Result<usize> {
        let horizon = now + Duration::days(REMINDERS[0]);
        let rows = sqlx::query(
            "SELECT certificate_id, source, not_after, reminded_days FROM certificates \
             WHERE not_after <= $1 ORDER BY certificate_id",
        )
        .bind(horizon)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        let mut announced = 0;
        for row in rows {
            let id = stored_id(&row)?;
            let not_after: DateTime<Utc> = row.try_get("not_after").map_err(storage_error)?;
            let reminded: Option<i32> = row.try_get("reminded_days").map_err(storage_error)?;
            let source: String = row.try_get("source").map_err(storage_error)?;
            let Some(days) = reminder(not_after - now) else {
                continue;
            };
            if reminded.is_some_and(|reminded| i64::from(reminded) <= days) {
                continue;
            }
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            let claimed = sqlx::query(
                "UPDATE certificates SET reminded_days = $2 WHERE certificate_id = $1 \
                 AND not_after = $3 AND (reminded_days IS NULL OR reminded_days > $2)",
            )
            .bind(id.as_str())
            .bind(i32::try_from(days).unwrap_or(0))
            .bind(not_after)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?
            .rows_affected();
            if claimed == 0 {
                continue;
            }
            self.publish(
                &mut transaction,
                cause,
                "tls.certificate.expiring",
                &id,
                &json!({
                    "id": id,
                    "source": source,
                    "not_after": not_after,
                    "within_days": days,
                    "expired": days == 0,
                }),
            )
            .await?;
            transaction.commit().await.map_err(storage_error)?;
            announced += 1;
        }
        Ok(announced)
    }

    pub async fn delete(
        &self,
        cause: Cause<'_>,
        id: CertificateId,
        expected: Option<u64>,
    ) -> Result<()> {
        let result = async {
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            let current = self.lock(&mut transaction, &id).await?;
            check_version(&id, current.version, expected)?;
            sqlx::query("DELETE FROM certificates WHERE certificate_id = $1")
                .bind(id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
            self.publish(
                &mut transaction,
                cause,
                "tls.certificate.deleted",
                &id,
                &json!({ "id": id, "fingerprint": current.details.fingerprint }),
            )
            .await?;
            transaction.commit().await.map_err(storage_error)?;
            if let Some(directory) = &self.directory {
                if let Err(error) = directory.remove(&id).await {
                    tracing::warn!(error_code = %error.code, certificate = %id, "certificate files not removed");
                }
            }
            Ok(())
        }
        .await;
        self.refused(cause, "delete", &id, result).await
    }

    async fn create(
        &self,
        cause: Cause<'_>,
        id: CertificateId,
        source: CertificateSource,
        accepted: Accepted,
    ) -> Result<Certificate> {
        let sealed = self
            .vault()?
            .seal(&owner(&id), accepted.key.as_bytes())
            .await?;
        let now = Utc::now();
        let certificate = Certificate {
            id,
            source,
            details: accepted.details,
            chain: accepted.chain,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let inserted = sqlx::query(
            "INSERT INTO certificates (certificate_id, source, details, chain, sealed_key, \
             not_after, version, created_at, updated_at) \
             VALUES ($1, $2, $3::jsonb, $4, $5, $6, 1, $7, $7) ON CONFLICT (certificate_id) DO NOTHING",
        )
        .bind(certificate.id.as_str())
        .bind(source_name(source)?)
        .bind(details(&certificate.details)?)
        .bind(&certificate.chain)
        .bind(sealed.as_str())
        .bind(certificate.details.not_after)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?
        .rows_affected();
        if inserted == 0 {
            return Err(PanelError::conflict(format!(
                "certificate {} already exists",
                certificate.id
            )));
        }
        self.publish(
            &mut transaction,
            cause,
            "tls.certificate.created",
            &certificate.id,
            &summary(&certificate),
        )
        .await?;
        transaction.commit().await.map_err(storage_error)?;
        self.deliver(&certificate, accepted.key).await;
        Ok(certificate)
    }

    async fn store_replacement(
        &self,
        cause: Cause<'_>,
        id: &CertificateId,
        expected: Option<u64>,
        source: CertificateSource,
        accepted: Accepted,
    ) -> Result<Certificate> {
        let sealed = self
            .vault()?
            .seal(&owner(id), accepted.key.as_bytes())
            .await?;
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let current = self.lock(&mut transaction, id).await?;
        check_version(id, current.version, expected)?;
        let certificate = Certificate {
            id: id.clone(),
            source,
            details: accepted.details,
            chain: accepted.chain,
            version: current.version + 1,
            created_at: current.created_at,
            updated_at: Utc::now(),
        };
        sqlx::query(
            "UPDATE certificates SET source = $2, details = $3::jsonb, chain = $4, sealed_key = $5, \
             not_after = $6, version = $7, updated_at = $8, reminded_days = NULL \
             WHERE certificate_id = $1",
        )
        .bind(id.as_str())
        .bind(source_name(source)?)
        .bind(details(&certificate.details)?)
        .bind(&certificate.chain)
        .bind(sealed.as_str())
        .bind(certificate.details.not_after)
        .bind(
            i64::try_from(certificate.version)
                .map_err(|_| PanelError::internal("version overflow"))?,
        )
        .bind(certificate.updated_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let mut data = summary(&certificate);
        data["previous_fingerprint"] = json!(current.details.fingerprint);
        self.publish(
            &mut transaction,
            cause,
            "tls.certificate.replaced",
            id,
            &data,
        )
        .await?;
        transaction.commit().await.map_err(storage_error)?;
        self.deliver(&certificate, accepted.key).await;
        Ok(certificate)
    }

    async fn lock(&self, connection: &mut PgConnection, id: &CertificateId) -> Result<Certificate> {
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM certificates WHERE certificate_id = $1 FOR UPDATE"
        ))
        .bind(id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .map(|row| certificate(&row))
        .transpose()?
        .ok_or_else(|| PanelError::not_found(format!("there is no certificate {id}")))
    }

    async fn publish<T: Serialize>(
        &self,
        connection: &mut PgConnection,
        cause: Cause<'_>,
        event_type: &str,
        id: &CertificateId,
        data: &T,
    ) -> Result<()> {
        let event = self.events.event_named_by(
            event_type,
            (AGGREGATE, id.as_str()),
            cause.scope,
            cause.principal,
            data,
        )?;
        PgOutbox::append(connection, &event).await
    }

    /// Records a refused change; what it changed is unchanged.
    async fn refused<T>(
        &self,
        cause: Cause<'_>,
        operation: &str,
        id: &CertificateId,
        result: Result<T>,
    ) -> Result<T> {
        if let Err(error) = &result {
            self.events
                .record_named_by(
                    "tls.certificate.refused",
                    (AGGREGATE, id.as_str()),
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

    /// Delivers a committed change; a failure leaves it to the next
    /// reconciliation.
    async fn deliver(&self, certificate: &Certificate, key: Zeroizing<String>) {
        let Some(directory) = &self.directory else {
            return;
        };
        let delivery = Delivery {
            id: certificate.id.clone(),
            chain: certificate.chain.clone(),
            key: Zeroizing::new(key.as_bytes().to_vec()),
        };
        if let Err(error) = directory.deliver(delivery).await {
            tracing::warn!(error_code = %error.code, certificate = %certificate.id, "certificate not delivered");
        }
    }

    async fn sealed(&self) -> Result<Vec<(CertificateId, String, Sealed)>> {
        sqlx::query("SELECT certificate_id, chain, sealed_key FROM certificates")
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .iter()
            .map(|row| {
                Ok((
                    stored_id(row)?,
                    row.try_get("chain").map_err(storage_error)?,
                    Sealed::new(
                        row.try_get::<String, _>("sealed_key")
                            .map_err(storage_error)?,
                    ),
                ))
            })
            .collect()
    }

    /// Seals keys sealed with a master key other than the active one again,
    /// so retired keys can be removed; returns how many changed.
    pub async fn reseal(&self) -> Result<usize> {
        let vault = self.vault()?;
        let mut changed = 0;
        for (id, _, sealed) in self.sealed().await? {
            if vault.is_current(&sealed) {
                continue;
            }
            let key = vault.open(&owner(&id), &sealed).await?;
            let resealed = vault.seal(&owner(&id), &key).await?;
            let updated = sqlx::query(
                "UPDATE certificates SET sealed_key = $3 WHERE certificate_id = $1 AND sealed_key = $2",
            )
            .bind(id.as_str())
            .bind(sealed.as_str())
            .bind(resealed.as_str())
            .execute(&self.pool)
            .await
            .map_err(storage_error)?;
            changed += usize::try_from(updated.rows_affected()).unwrap_or(0);
        }
        Ok(changed)
    }

    /// Brings the secret directory in line with the inventory; returns how
    /// many files changed.
    pub async fn reconcile(&self) -> Result<usize> {
        let Some(directory) = &self.directory else {
            return Ok(0);
        };
        let vault = self.vault()?;
        let mut deliveries = Vec::new();
        for (id, chain, sealed) in self.sealed().await? {
            let key = vault.open(&owner(&id), &sealed).await?;
            deliveries.push(Delivery { id, chain, key });
        }
        directory.reconcile(deliveries).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders_follow_the_smallest_threshold_reached() {
        assert_eq!(reminder(Duration::days(45)), None);
        assert_eq!(reminder(Duration::days(30)), Some(30));
        assert_eq!(reminder(Duration::days(20)), Some(30));
        assert_eq!(reminder(Duration::days(10)), Some(14));
        assert_eq!(reminder(Duration::days(5)), Some(7));
        assert_eq!(reminder(Duration::hours(30)), Some(3));
        assert_eq!(reminder(Duration::hours(2)), Some(1));
        assert_eq!(reminder(Duration::zero()), Some(0));
        assert_eq!(reminder(Duration::days(-3)), Some(0));
    }
}
