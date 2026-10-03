//! Trusts in workload tokens, beside the accounts they act as.

use super::{storage, PgIdentityStore};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use panel_identity::events;
use panel_identity::{store::Cause, AccountId, WorkloadStore, WorkloadTrust};
use serde_json::json;
use sqlx::{postgres::PgRow, Row};

fn trust(row: &PgRow) -> Result<WorkloadTrust> {
    let claims: String = row.try_get("claims").map_err(storage)?;
    let minutes: i32 = row.try_get("session_minutes").map_err(storage)?;
    Ok(WorkloadTrust {
        id: row.try_get("id").map_err(storage)?,
        account: AccountId::from_uuid(row.try_get("account_id").map_err(storage)?),
        issuer: row.try_get("issuer").map_err(storage)?,
        audience: row.try_get("audience").map_err(storage)?,
        subject: row.try_get("subject").map_err(storage)?,
        claims: serde_json::from_str(&claims)
            .map_err(|_| PanelError::corrupt_state("stored workload claims are invalid"))?,
        session_minutes: u32::try_from(minutes)
            .map_err(|_| PanelError::corrupt_state("stored session length is negative"))?,
        enabled: row.try_get("enabled").map_err(storage)?,
        created_at: row.try_get("created_at").map_err(storage)?,
        updated_at: row.try_get("updated_at").map_err(storage)?,
    })
}

#[async_trait]
impl WorkloadStore for PgIdentityStore {
    async fn trusts(&self) -> Result<Vec<WorkloadTrust>> {
        sqlx::query(
            "SELECT id, account_id, issuer, audience, subject, claims::text AS claims, \
             session_minutes, enabled, created_at, updated_at FROM workload_trusts ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?
        .iter()
        .map(trust)
        .collect()
    }

    async fn put_trust(&self, trust: WorkloadTrust, cause: &Cause) -> Result<bool> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let created: bool = sqlx::query_scalar(
            "INSERT INTO workload_trusts (id, account_id, issuer, audience, subject, claims, \
             session_minutes, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8, $9, $10) \
             ON CONFLICT (id) DO UPDATE SET account_id = EXCLUDED.account_id, \
             issuer = EXCLUDED.issuer, audience = EXCLUDED.audience, \
             subject = EXCLUDED.subject, claims = EXCLUDED.claims, \
             session_minutes = EXCLUDED.session_minutes, enabled = EXCLUDED.enabled, \
             updated_at = EXCLUDED.updated_at \
             RETURNING (xmax = 0)",
        )
        .bind(&trust.id)
        .bind(trust.account.as_uuid())
        .bind(&trust.issuer)
        .bind(&trust.audience)
        .bind(&trust.subject)
        .bind(json!(trust.claims).to_string())
        .bind(i32::try_from(trust.session_minutes).unwrap_or(i32::MAX))
        .bind(trust.enabled)
        .bind(trust.created_at)
        .bind(trust.updated_at)
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage)?;
        if created {
            self.emit_on(
                &mut transaction,
                ("workload_trust", &trust.id),
                cause,
                &events::workload_trust_created(&trust),
            )
            .await?;
        } else {
            self.emit_on(
                &mut transaction,
                ("workload_trust", &trust.id),
                cause,
                &events::workload_trust_updated(&trust),
            )
            .await?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(created)
    }

    async fn delete_trust(&self, id: &str, cause: &Cause) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let deleted = sqlx::query("DELETE FROM workload_trusts WHERE id = $1")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        if deleted.rows_affected() == 0 {
            return Err(PanelError::not_found(format!(
                "there is no workload identity {id}"
            )));
        }
        self.emit_on(
            &mut transaction,
            ("workload_trust", id),
            cause,
            &events::workload_trust_deleted(id),
        )
        .await?;
        transaction.commit().await.map_err(storage)
    }
}
