//! Identity providers, their links to accounts and sign-ins in progress.

use super::{storage, PgIdentityStore};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use panel_identity::{
    store::{Attempt, Cause},
    Account, AccountId, ClaimNames, GroupRole, IdentityProvider, PendingSignIn, ProviderLink,
    ProviderSession, ProviderSignIn, ProviderStore, SecretHash, SessionId,
};
use serde_json::json;
use sqlx::{postgres::PgRow, Postgres, Row, Transaction};

/// Selects every provider column, the JSON ones as text.
macro_rules! select_providers {
    ($rest:literal) => {
        concat!(
            "SELECT id, display_name, issuer, client_id, client_secret, scopes, ",
            "claims::text AS claims, group_roles::text AS group_roles, create_accounts, ",
            "enabled, created_at, updated_at FROM identity_providers ",
            $rest
        )
    };
}

fn provider(row: &PgRow) -> Result<IdentityProvider> {
    let claims: String = row.try_get("claims").map_err(storage)?;
    let group_roles: String = row.try_get("group_roles").map_err(storage)?;
    Ok(IdentityProvider {
        id: row.try_get("id").map_err(storage)?,
        display_name: row.try_get("display_name").map_err(storage)?,
        issuer: row.try_get("issuer").map_err(storage)?,
        client_id: row.try_get("client_id").map_err(storage)?,
        client_secret: row.try_get("client_secret").map_err(storage)?,
        scopes: row.try_get("scopes").map_err(storage)?,
        claims: serde_json::from_str::<ClaimNames>(&claims)
            .map_err(|_| PanelError::corrupt_state("a provider's claim names are unreadable"))?,
        group_roles: serde_json::from_str::<Vec<GroupRole>>(&group_roles)
            .map_err(|_| PanelError::corrupt_state("a provider's group mappings are unreadable"))?,
        create_accounts: row.try_get("create_accounts").map_err(storage)?,
        enabled: row.try_get("enabled").map_err(storage)?,
        created_at: row.try_get("created_at").map_err(storage)?,
        updated_at: row.try_get("updated_at").map_err(storage)?,
    })
}

#[async_trait]
impl ProviderStore for PgIdentityStore {
    async fn providers(&self) -> Result<Vec<IdentityProvider>> {
        sqlx::query(select_providers!("ORDER BY id"))
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
            .iter()
            .map(provider)
            .collect()
    }

    async fn provider(&self, id: &str) -> Result<Option<IdentityProvider>> {
        sqlx::query(select_providers!("WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .as_ref()
            .map(provider)
            .transpose()
    }

    async fn put_provider(&self, provider: IdentityProvider, cause: &Cause) -> Result<bool> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let created: bool = sqlx::query_scalar(
            "INSERT INTO identity_providers (id, display_name, issuer, client_id, client_secret, \
             scopes, claims, group_roles, create_accounts, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, $8::jsonb, $9, $10, $11, $12) \
             ON CONFLICT (id) DO UPDATE SET display_name = EXCLUDED.display_name, \
             issuer = EXCLUDED.issuer, client_id = EXCLUDED.client_id, \
             client_secret = EXCLUDED.client_secret, scopes = EXCLUDED.scopes, \
             claims = EXCLUDED.claims, group_roles = EXCLUDED.group_roles, \
             create_accounts = EXCLUDED.create_accounts, enabled = EXCLUDED.enabled, \
             updated_at = EXCLUDED.updated_at \
             RETURNING (xmax = 0)",
        )
        .bind(&provider.id)
        .bind(&provider.display_name)
        .bind(&provider.issuer)
        .bind(&provider.client_id)
        .bind(&provider.client_secret)
        .bind(&provider.scopes)
        .bind(json!(provider.claims).to_string())
        .bind(json!(provider.group_roles).to_string())
        .bind(provider.create_accounts)
        .bind(provider.enabled)
        .bind(provider.created_at)
        .bind(provider.updated_at)
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage)?;
        if !provider.enabled {
            end_sessions(&mut transaction, &provider.id, "provider_disabled").await?;
        }
        self.emit_on(
            &mut transaction,
            if created {
                "identity.provider.created"
            } else {
                "identity.provider.updated"
            },
            ("provider", &provider.id),
            cause,
            &json!({ "provider": provider.id, "issuer": provider.issuer, "enabled": provider.enabled }),
        )
        .await?;
        transaction.commit().await.map_err(storage)?;
        Ok(created)
    }

    async fn delete_provider(&self, id: &str, cause: &Cause) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        end_sessions(&mut transaction, id, "provider_deleted").await?;
        let deleted = sqlx::query("DELETE FROM identity_providers WHERE id = $1")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        if deleted.rows_affected() == 0 {
            return Err(PanelError::not_found(format!(
                "there is no identity provider {id}"
            )));
        }
        self.emit_on(
            &mut transaction,
            "identity.provider.deleted",
            ("provider", id),
            cause,
            &json!({ "provider": id }),
        )
        .await?;
        transaction.commit().await.map_err(storage)
    }

    async fn save_sign_in(&self, pending: PendingSignIn) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        sqlx::query("DELETE FROM pending_sign_ins WHERE expires_at < now() - interval '1 hour'")
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        sqlx::query(
            "INSERT INTO pending_sign_ins (state_hash, provider_id, nonce, verifier, return_to, \
             expires_at) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(pending.state.as_bytes().as_slice())
        .bind(&pending.provider)
        .bind(&pending.nonce)
        .bind(&pending.verifier)
        .bind(&pending.return_to)
        .bind(pending.expires_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        transaction.commit().await.map_err(storage)
    }

    async fn take_sign_in(
        &self,
        state: &SecretHash,
        now: DateTime<Utc>,
    ) -> Result<Option<PendingSignIn>> {
        let row = sqlx::query(
            "DELETE FROM pending_sign_ins WHERE state_hash = $1 \
             RETURNING provider_id, nonce, verifier, return_to, expires_at",
        )
        .bind(state.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let expires_at: DateTime<Utc> = row.try_get("expires_at").map_err(storage)?;
        if expires_at <= now {
            return Ok(None);
        }
        Ok(Some(PendingSignIn {
            state: *state,
            provider: row.try_get("provider_id").map_err(storage)?,
            nonce: row.try_get("nonce").map_err(storage)?,
            verifier: row.try_get("verifier").map_err(storage)?,
            return_to: row.try_get("return_to").map_err(storage)?,
            expires_at,
        }))
    }

    async fn link(&self, provider: &str, subject: &str) -> Result<Option<ProviderLink>> {
        let row = sqlx::query(
            "SELECT account_id, granted_roles FROM provider_links \
             WHERE provider_id = $1 AND subject = $2",
        )
        .bind(provider)
        .bind(subject)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        row.map(|row| {
            Ok(ProviderLink {
                provider: provider.to_owned(),
                subject: subject.to_owned(),
                account: AccountId::from_uuid(row.try_get("account_id").map_err(storage)?),
                granted_roles: row.try_get("granted_roles").map_err(storage)?,
            })
        })
        .transpose()
    }

    async fn sign_in_with_provider(
        &self,
        sign_in: ProviderSignIn,
        attempt: &Attempt,
        cause: &Cause,
    ) -> Result<Account> {
        let session = &sign_in.session.session;
        let account_id = sign_in.link.account;
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        if let Some(new) = &sign_in.new_account {
            sqlx::query(
                "INSERT INTO accounts (id, username, display_name, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $4)",
            )
            .bind(new.id.as_uuid())
            .bind(new.username.as_str())
            .bind(&new.display_name)
            .bind(new.now)
            .execute(&mut *transaction)
            .await
            .map_err(|error| match storage(error) {
                conflict if conflict.code.as_str() == panel_errors::ErrorCode::CONFLICT => {
                    PanelError::conflict(format!("the username {} is taken", new.username))
                }
                other => other,
            })?;
            self.emit(
                &mut transaction,
                "identity.account.created",
                new.id,
                cause,
                &json!({ "account": new.id, "username": new.username, "provider": sign_in.link.provider }),
            )
            .await?;
        }
        let disabled: bool =
            sqlx::query_scalar("SELECT disabled FROM accounts WHERE id = $1 FOR UPDATE")
                .bind(account_id.as_uuid())
                .fetch_one(&mut *transaction)
                .await
                .map_err(storage)?;
        if disabled {
            return Err(PanelError::permission_denied("the account is disabled"));
        }
        let held: Vec<String> = sqlx::query_scalar(
            "SELECT role_id FROM role_bindings WHERE account_id = $1 ORDER BY role_id",
        )
        .bind(account_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage)?;
        let mut roles = sign_in.roles.clone();
        roles.sort();
        if held != roles {
            sqlx::query("DELETE FROM role_bindings WHERE account_id = $1")
                .bind(account_id.as_uuid())
                .execute(&mut *transaction)
                .await
                .map_err(storage)?;
            sqlx::query(
                "INSERT INTO role_bindings (account_id, role_id) SELECT $1, unnest($2::text[])",
            )
            .bind(account_id.as_uuid())
            .bind(&roles)
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
            sqlx::query("UPDATE accounts SET updated_at = $2 WHERE id = $1")
                .bind(account_id.as_uuid())
                .bind(session.created_at)
                .execute(&mut *transaction)
                .await
                .map_err(storage)?;
        }
        sqlx::query(
            "INSERT INTO provider_links (provider_id, subject, account_id, granted_roles, created_at) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (provider_id, subject) \
             DO UPDATE SET granted_roles = EXCLUDED.granted_roles",
        )
        .bind(&sign_in.link.provider)
        .bind(&sign_in.link.subject)
        .bind(account_id.as_uuid())
        .bind(&sign_in.link.granted_roles)
        .bind(session.created_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        sqlx::query(
            "INSERT INTO sessions (id, account_id, secret_hash, transport, created_at, \
             last_seen_at, expires_at, client_address, user_agent) \
             VALUES ($1, $2, $3, $4, $5, $5, $6, $7, $8)",
        )
        .bind(session.id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(sign_in.session.secret.as_bytes().as_slice())
        .bind(session.transport.as_str())
        .bind(session.created_at)
        .bind(session.expires_at)
        .bind(&session.client_address)
        .bind(&session.user_agent)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        sqlx::query(
            "INSERT INTO provider_sessions (session_id, provider_id, refresh_token, checked_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(session.id.as_uuid())
        .bind(&sign_in.link.provider)
        .bind(&sign_in.refresh_token)
        .bind(session.created_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        sqlx::query("UPDATE accounts SET last_login_at = $2 WHERE id = $1")
            .bind(account_id.as_uuid())
            .bind(session.created_at)
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        self.emit(
            &mut transaction,
            "identity.login.succeeded",
            account_id,
            cause,
            &json!({
                "attempt": attempt,
                "provider": sign_in.link.provider,
                "session": session.id,
                "transport": session.transport,
            }),
        )
        .await?;
        let account = Self::reread(&mut transaction, account_id).await?;
        transaction.commit().await.map_err(storage)?;
        Ok(account)
    }

    async fn claim_rechecks(
        &self,
        checked_before: DateTime<Utc>,
        seen_after: DateTime<Utc>,
        now: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<ProviderSession>> {
        let rows = sqlx::query(
            "UPDATE provider_sessions AS claimed SET checked_at = $3 FROM ( \
             SELECT p.session_id, s.account_id FROM provider_sessions p \
             JOIN sessions s ON s.id = p.session_id \
             WHERE p.refresh_token IS NOT NULL AND p.checked_at < $1 \
             AND s.revoked_at IS NULL AND s.expires_at > $3 AND s.last_seen_at > $2 \
             ORDER BY p.checked_at LIMIT $4 FOR UPDATE OF p SKIP LOCKED) AS due \
             WHERE claimed.session_id = due.session_id \
             RETURNING claimed.session_id, due.account_id, claimed.provider_id, \
             claimed.refresh_token",
        )
        .bind(checked_before)
        .bind(seen_after)
        .bind(now)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.iter()
            .map(|row| {
                Ok(ProviderSession {
                    session: SessionId::from_uuid(row.try_get("session_id").map_err(storage)?),
                    account: AccountId::from_uuid(row.try_get("account_id").map_err(storage)?),
                    provider: row.try_get("provider_id").map_err(storage)?,
                    refresh_token: row.try_get("refresh_token").map_err(storage)?,
                })
            })
            .collect()
    }

    async fn rotate_refresh_token(&self, session: SessionId, refresh_token: String) -> Result<()> {
        sqlx::query("UPDATE provider_sessions SET refresh_token = $2 WHERE session_id = $1")
            .bind(session.as_uuid())
            .bind(refresh_token)
            .execute(&self.pool)
            .await
            .map_err(storage)?;
        Ok(())
    }
}

/// Ends the sessions signed in through a provider.
async fn end_sessions(
    transaction: &mut Transaction<'_, Postgres>,
    provider: &str,
    reason: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE sessions SET revoked_at = now(), revoke_reason = $2 \
         WHERE revoked_at IS NULL AND id IN \
         (SELECT session_id FROM provider_sessions WHERE provider_id = $1)",
    )
    .bind(provider)
    .bind(reason)
    .execute(&mut **transaction)
    .await
    .map_err(storage)?;
    Ok(())
}
