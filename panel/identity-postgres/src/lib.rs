#![forbid(unsafe_code)]

//! The identity store in PostgreSQL, in the `identity` schema `panel-api`
//! owns. Every change appends its event to the outbox in the transaction
//! that makes it.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use panel_events::EventData;
use panel_identity::events;
use panel_identity::{
    built_in_roles,
    store::{
        AccountChange, AccountStore, Attempt, Cause, Failure, GrantStore, NewAccount, NewSession,
        NewToken, RoleStore, SessionGrant, SessionStore, SignInPolicyStore, StoredAccount,
        StoredPassword, TokenGrant, TokenStore,
    },
    Account, AccountId, ApiToken, Grant, GrantId, PasswordSignIn, Permission, PermissionSet, Role,
    SecretHash, Session, SessionId, TokenId, Transport, Username,
};
use panel_postgres::{EventLog, PgOutbox, SchemaMigration, ServiceDatabase};
use serde_json::json;
use sqlx::{postgres::PgRow, PgConnection, PgPool, Row};
use std::sync::LazyLock;

mod providers;
mod workload;

pub const MIGRATIONS: &[SchemaMigration] = &[
    SchemaMigration::new(
        10_000,
        "accounts, roles, sessions and API tokens",
        include_str!("../migrations/10000_accounts.sql"),
    ),
    SchemaMigration::new(
        10_100,
        "identity providers, their links and sign-ins",
        include_str!("../migrations/10100_identity_providers.sql"),
    ),
    SchemaMigration::new(
        10_200,
        "break-glass accounts and the password sign-in policy",
        include_str!("../migrations/10200_break_glass.sql"),
    ),
    SchemaMigration::new(
        10_300,
        "service accounts",
        include_str!("../migrations/10300_service_accounts.sql"),
    ),
    SchemaMigration::new(
        10_400,
        "workload identity trusts",
        include_str!("../migrations/10400_workload_trusts.sql"),
    ),
    SchemaMigration::new(
        10_500,
        "scoped and conditional grants",
        include_str!("../migrations/10500_grants.sql"),
    ),
];

/// Ended sessions are deleted this many days after they expire.
const SESSION_RETENTION_DAYS: i64 = 30;

const ACCOUNT: &str = "a.id, a.username, a.display_name, a.disabled, a.locked, a.break_glass, \
     a.service, a.created_at, a.updated_at, a.last_login_at, a.password_changed_at, a.password_hash, \
     a.failures, a.retry_after, \
     ARRAY(SELECT b.role_id FROM role_bindings b WHERE b.account_id = a.id ORDER BY b.role_id) \
     AS roles";

const PERMISSIONS: &str = "ARRAY(SELECT DISTINCT p FROM role_bindings b \
     JOIN roles r ON r.id = b.role_id CROSS JOIN LATERAL unnest(r.permissions) AS p \
     WHERE b.account_id = a.id) AS permissions";

const SESSION: &str = "s.id AS session_id, s.account_id AS session_account, s.transport, \
     s.created_at AS session_created_at, s.last_seen_at, s.expires_at AS session_expires_at, \
     s.client_address, s.user_agent, s.revoked_at AS session_revoked_at";

const TOKEN: &str = "t.id AS token_id, t.account_id AS token_account, t.name AS token_name, \
     t.permissions AS token_permissions, t.created_at AS token_created_at, \
     t.expires_at AS token_expires_at, t.last_used_at, t.revoked_at AS token_revoked_at";

/// A query composed of the column lists above, built once.
macro_rules! composed {
    ($name:ident = $($part:expr),+ $(,)?) => {
        static $name: LazyLock<String> = LazyLock::new(|| format!($($part),+));
    };
}

composed!(ACCOUNT_BY_ID = "SELECT {ACCOUNT} FROM accounts a WHERE a.id = $1");
composed!(ACCOUNT_BY_NAME = "SELECT {ACCOUNT} FROM accounts a WHERE a.username = $1");
composed!(ACCOUNTS = "SELECT {ACCOUNT} FROM accounts a ORDER BY a.username");
composed!(
    SESSION_GRANT = "SELECT {SESSION}, {ACCOUNT}, {PERMISSIONS} FROM sessions s \
     JOIN accounts a ON a.id = s.account_id WHERE s.secret_hash = $1"
);
composed!(
    SESSIONS_OF = "SELECT {SESSION} FROM sessions s WHERE s.account_id = $1 \
     AND s.revoked_at IS NULL AND s.expires_at > $2 ORDER BY s.created_at DESC"
);
composed!(
    TOKEN_GRANT = "SELECT {TOKEN}, {ACCOUNT}, {PERMISSIONS} FROM api_tokens t \
     JOIN accounts a ON a.id = t.account_id WHERE t.secret_hash = $1"
);
composed!(
    TOKENS_OF =
        "SELECT {TOKEN} FROM api_tokens t WHERE t.account_id = $1 ORDER BY t.created_at DESC"
);

fn storage(error: sqlx::Error) -> PanelError {
    let code = error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|code| code.into_owned());
    match code.as_deref() {
        Some("23505") => PanelError::conflict("the username is taken"),
        Some("23503") => PanelError::invalid_argument("the account or role does not exist"),
        _ => PanelError::storage_unavailable("identity storage failed").with_source(error),
    }
}

fn get<'r, T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>>(
    row: &'r PgRow,
    column: &str,
) -> Result<T> {
    row.try_get(column).map_err(storage)
}

fn permissions(names: Vec<String>) -> PermissionSet {
    names
        .iter()
        .filter_map(|name| Permission::parse(name))
        .collect()
}

fn account(row: &PgRow) -> Result<Account> {
    Ok(Account {
        id: AccountId::from_uuid(get(row, "id")?),
        username: Username::new(&get::<String>(row, "username")?)?,
        display_name: get(row, "display_name")?,
        disabled: get(row, "disabled")?,
        locked: get(row, "locked")?,
        roles: get(row, "roles")?,
        break_glass: get(row, "break_glass")?,
        service: get(row, "service")?,
        created_at: get(row, "created_at")?,
        updated_at: get(row, "updated_at")?,
        last_login_at: get(row, "last_login_at")?,
        password_changed_at: get(row, "password_changed_at")?,
    })
}

fn stored_account(row: &PgRow) -> Result<StoredAccount> {
    let hash: Option<String> = get(row, "password_hash")?;
    let failures: i32 = get(row, "failures")?;
    let retry_after: Option<DateTime<Utc>> = get(row, "retry_after")?;
    Ok(StoredAccount {
        account: account(row)?,
        password: hash.map(|hash| StoredPassword {
            hash,
            failures: u32::try_from(failures).unwrap_or_default(),
            retry_after,
        }),
    })
}

fn session(row: &PgRow) -> Result<Session> {
    let transport: String = get(row, "transport")?;
    Ok(Session {
        id: SessionId::from_uuid(get(row, "session_id")?),
        account: AccountId::from_uuid(get(row, "session_account")?),
        transport: Transport::parse(&transport)
            .ok_or_else(|| PanelError::corrupt_state(format!("unknown transport {transport:?}")))?,
        created_at: get(row, "session_created_at")?,
        last_seen_at: get(row, "last_seen_at")?,
        expires_at: get(row, "session_expires_at")?,
        client_address: get(row, "client_address")?,
        user_agent: get(row, "user_agent")?,
        revoked_at: get(row, "session_revoked_at")?,
    })
}

fn token(row: &PgRow) -> Result<ApiToken> {
    Ok(ApiToken {
        id: TokenId::from_uuid(get(row, "token_id")?),
        account: AccountId::from_uuid(get(row, "token_account")?),
        name: get(row, "token_name")?,
        permissions: permissions(get(row, "token_permissions")?),
        created_at: get(row, "token_created_at")?,
        expires_at: get(row, "token_expires_at")?,
        last_used_at: get(row, "last_used_at")?,
        revoked_at: get(row, "token_revoked_at")?,
    })
}

#[derive(Clone)]
pub struct PgIdentityStore {
    pool: PgPool,
    events: EventLog,
}

impl PgIdentityStore {
    /// A store over `database`, recording events with `events`.
    pub fn new(database: &ServiceDatabase, events: EventLog) -> Self {
        Self {
            pool: database.pool().clone(),
            events,
        }
    }

    /// Writes the built-in roles as this release defines them.
    pub async fn sync_roles(&self) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        for role in built_in_roles() {
            sqlx::query(
                "INSERT INTO roles (id, name, description, permissions, built_in) \
                 VALUES ($1, $2, $3, $4, true) \
                 ON CONFLICT (id) DO UPDATE SET name = $2, description = $3, \
                 permissions = $4, built_in = true",
            )
            .bind(&role.id)
            .bind(&role.name)
            .bind(&role.description)
            .bind(role.permissions.names())
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)
    }

    async fn emit<E: EventData>(
        &self,
        connection: &mut PgConnection,
        account: AccountId,
        cause: &Cause,
        data: &E,
    ) -> Result<()> {
        self.emit_on(connection, ("account", &account.to_string()), cause, data)
            .await
    }

    async fn emit_on<E: EventData>(
        &self,
        connection: &mut PgConnection,
        aggregate: (&str, &str),
        cause: &Cause,
        data: &E,
    ) -> Result<()> {
        let event = self
            .events
            .event(aggregate, &cause.scope, &cause.actor, data)?;
        PgOutbox::append(connection, &event).await
    }

    async fn reread(connection: &mut PgConnection, id: AccountId) -> Result<Account> {
        let row = sqlx::query(ACCOUNT_BY_ID.as_str())
            .bind(id.as_uuid())
            .fetch_one(connection)
            .await
            .map_err(storage)?;
        account(&row)
    }

    async fn end_all(
        connection: &mut PgConnection,
        account: AccountId,
        keep: Option<SessionId>,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sessions SET revoked_at = $3, revoke_reason = $4 \
             WHERE account_id = $1 AND revoked_at IS NULL AND id IS DISTINCT FROM $2",
        )
        .bind(account.as_uuid())
        .bind(keep.map(|session| session.as_uuid()))
        .bind(now)
        .bind(reason)
        .execute(connection)
        .await
        .map_err(storage)
        .map(|_| ())
    }
}

#[async_trait]
impl AccountStore for PgIdentityStore {
    async fn has_accounts(&self) -> Result<bool> {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts)")
            .fetch_one(&self.pool)
            .await
            .map_err(storage)
    }

    async fn create_account(&self, new: NewAccount, cause: &Cause) -> Result<Account> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        if new.first {
            sqlx::query("LOCK TABLE accounts IN SHARE ROW EXCLUSIVE MODE")
                .execute(&mut *transaction)
                .await
                .map_err(storage)?;
            let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts)")
                .fetch_one(&mut *transaction)
                .await
                .map_err(storage)?;
            if exists {
                return Err(PanelError::conflict("the panel is already set up"));
            }
        }
        sqlx::query(
            "INSERT INTO accounts (id, username, display_name, password_hash, \
             password_changed_at, created_at, updated_at, service) \
             VALUES ($1, $2, $3, $4, $5, $6, $6, $7)",
        )
        .bind(new.id.as_uuid())
        .bind(new.username.as_str())
        .bind(&new.display_name)
        .bind(&new.password_hash)
        .bind(new.password_hash.as_ref().map(|_| new.now))
        .bind(new.now)
        .bind(new.service)
        .execute(&mut *transaction)
        .await
        .map_err(|error| match storage(error) {
            conflict if conflict.code.as_str() == panel_errors::ErrorCode::CONFLICT => {
                PanelError::conflict(format!("the username {} is taken", new.username))
            }
            other => other,
        })?;
        sqlx::query(
            "INSERT INTO role_bindings (account_id, role_id) SELECT $1, unnest($2::text[])",
        )
        .bind(new.id.as_uuid())
        .bind(&new.roles)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        let account = Self::reread(&mut transaction, new.id).await?;
        self.emit(
            &mut transaction,
            account.id,
            cause,
            &events::account_created(&account),
        )
        .await?;
        transaction.commit().await.map_err(storage)?;
        Ok(account)
    }

    async fn account(&self, id: AccountId) -> Result<Option<StoredAccount>> {
        let row = sqlx::query(ACCOUNT_BY_ID.as_str())
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?;
        row.as_ref().map(stored_account).transpose()
    }

    async fn account_named(&self, username: &Username) -> Result<Option<StoredAccount>> {
        let row = sqlx::query(ACCOUNT_BY_NAME.as_str())
            .bind(username.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?;
        row.as_ref().map(stored_account).transpose()
    }

    async fn accounts(&self) -> Result<Vec<Account>> {
        let rows = sqlx::query(ACCOUNTS.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        rows.iter().map(account).collect()
    }

    async fn update_account(
        &self,
        id: AccountId,
        change: AccountChange,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<Account> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let updated = sqlx::query(
            "UPDATE accounts SET \
             display_name = CASE WHEN $2 THEN $3 ELSE display_name END, \
             disabled = COALESCE($4, disabled), \
             locked = locked AND NOT $5, \
             failures = CASE WHEN $5 THEN 0 ELSE failures END, \
             retry_after = CASE WHEN $5 THEN NULL ELSE retry_after END, \
             break_glass = COALESCE($7, break_glass), \
             updated_at = $6 \
             WHERE id = $1",
        )
        .bind(id.as_uuid())
        .bind(change.display_name.is_some())
        .bind(change.display_name.clone().flatten())
        .bind(change.disabled)
        .bind(change.unlock)
        .bind(now)
        .bind(change.break_glass)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        if updated.rows_affected() == 0 {
            return Err(PanelError::not_found(format!("there is no account {id}")));
        }
        if let Some(roles) = &change.roles {
            sqlx::query("DELETE FROM role_bindings WHERE account_id = $1")
                .bind(id.as_uuid())
                .execute(&mut *transaction)
                .await
                .map_err(storage)?;
            sqlx::query(
                "INSERT INTO role_bindings (account_id, role_id) SELECT $1, unnest($2::text[])",
            )
            .bind(id.as_uuid())
            .bind(roles)
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        }
        if change.disabled == Some(true) {
            Self::end_all(&mut transaction, id, None, "disabled", now).await?;
            sqlx::query(
                "UPDATE api_tokens SET revoked_at = $2 WHERE account_id = $1 AND revoked_at IS NULL",
            )
            .bind(id.as_uuid())
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        }
        let account = Self::reread(&mut transaction, id).await?;
        self.emit(
            &mut transaction,
            id,
            cause,
            &events::account_updated(id, &change),
        )
        .await?;
        transaction.commit().await.map_err(storage)?;
        Ok(account)
    }

    async fn set_password(
        &self,
        id: AccountId,
        hash: String,
        keep: Option<SessionId>,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let updated = sqlx::query(
            "UPDATE accounts SET password_hash = $2, password_changed_at = $3, failures = 0, \
             retry_after = NULL, locked = false, updated_at = $3 WHERE id = $1",
        )
        .bind(id.as_uuid())
        .bind(hash)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        if updated.rows_affected() == 0 {
            return Err(PanelError::not_found(format!("there is no account {id}")));
        }
        Self::end_all(&mut transaction, id, keep, "password_changed", now).await?;
        self.emit(&mut transaction, id, cause, &events::password_changed(id))
            .await?;
        transaction.commit().await.map_err(storage)
    }

    async fn rehash_password(&self, id: AccountId, hash: String) -> Result<()> {
        sqlx::query("UPDATE accounts SET password_hash = $2 WHERE id = $1")
            .bind(id.as_uuid())
            .bind(hash)
            .execute(&self.pool)
            .await
            .map_err(storage)
            .map(|_| ())
    }

    async fn login_failed(&self, failure: Failure, attempt: &Attempt, cause: &Cause) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "UPDATE accounts SET failures = $2, retry_after = $3, locked = locked OR $4 WHERE id = $1",
        )
        .bind(failure.account.as_uuid())
        .bind(i32::try_from(failure.failures).unwrap_or(i32::MAX))
        .bind(failure.retry_after)
        .bind(failure.lock)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        self.emit(
            &mut transaction,
            failure.account,
            cause,
            &events::wrong_password(attempt, &failure),
        )
        .await?;
        transaction.commit().await.map_err(storage)
    }

    async fn login_refused(&self, attempt: &Attempt, reason: &str, cause: &Cause) {
        let subject = if attempt.username.is_empty() {
            "unknown".to_owned()
        } else {
            attempt.username.clone()
        };
        self.events
            .record(
                ("login", &subject),
                &cause.scope,
                &cause.actor,
                &events::login_refused(attempt, reason),
            )
            .await;
    }
}

#[async_trait]
impl SessionStore for PgIdentityStore {
    async fn create_session(
        &self,
        new: NewSession,
        attempt: &Attempt,
        cause: &Cause,
    ) -> Result<()> {
        let session = &new.session;
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO sessions (id, account_id, secret_hash, transport, created_at, \
             last_seen_at, expires_at, client_address, user_agent) \
             VALUES ($1, $2, $3, $4, $5, $5, $6, $7, $8)",
        )
        .bind(session.id.as_uuid())
        .bind(session.account.as_uuid())
        .bind(new.secret.as_bytes().as_slice())
        .bind(session.transport.as_str())
        .bind(session.created_at)
        .bind(session.expires_at)
        .bind(&session.client_address)
        .bind(&session.user_agent)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE accounts SET failures = 0, retry_after = NULL, last_login_at = $2 WHERE id = $1",
        )
        .bind(session.account.as_uuid())
        .bind(session.created_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        sqlx::query("DELETE FROM sessions WHERE expires_at < $1")
            .bind(session.created_at - chrono::Duration::days(SESSION_RETENTION_DAYS))
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        if attempt.break_glass {
            self.emit(
                &mut transaction,
                session.account,
                cause,
                &events::break_glass_used(attempt, session),
            )
            .await?;
        }
        self.emit(
            &mut transaction,
            session.account,
            cause,
            &events::login_succeeded(attempt, session),
        )
        .await?;
        transaction.commit().await.map_err(storage)
    }

    async fn session(&self, secret: &SecretHash) -> Result<Option<SessionGrant>> {
        let Some(row) = sqlx::query(SESSION_GRANT.as_str())
            .bind(secret.as_bytes().as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
        else {
            return Ok(None);
        };
        Ok(Some(SessionGrant {
            session: session(&row)?,
            account: account(&row)?,
            permissions: permissions(get(&row, "permissions")?),
        }))
    }

    async fn touch_session(&self, id: SessionId, at: DateTime<Utc>) -> Result<()> {
        sqlx::query("UPDATE sessions SET last_seen_at = $2 WHERE id = $1 AND last_seen_at < $2")
            .bind(id.as_uuid())
            .bind(at)
            .execute(&self.pool)
            .await
            .map_err(storage)
            .map(|_| ())
    }

    async fn sessions(&self, account: AccountId, now: DateTime<Utc>) -> Result<Vec<Session>> {
        let rows = sqlx::query(SESSIONS_OF.as_str())
            .bind(account.as_uuid())
            .bind(now)
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        rows.iter().map(session).collect()
    }

    async fn end_session(
        &self,
        account: AccountId,
        id: SessionId,
        reason: &str,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<bool> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let ended = sqlx::query(
            "UPDATE sessions SET revoked_at = $3, revoke_reason = $4 \
             WHERE id = $1 AND account_id = $2 AND revoked_at IS NULL",
        )
        .bind(id.as_uuid())
        .bind(account.as_uuid())
        .bind(now)
        .bind(reason)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?
        .rows_affected()
            > 0;
        if ended {
            self.emit(
                &mut transaction,
                account,
                cause,
                &events::session_ended(account, id, reason),
            )
            .await?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(ended)
    }

    async fn end_sessions(
        &self,
        account: AccountId,
        keep: Option<SessionId>,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<u64> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let ended = sqlx::query(
            "UPDATE sessions SET revoked_at = $3, revoke_reason = 'revoked' \
             WHERE account_id = $1 AND revoked_at IS NULL AND id IS DISTINCT FROM $2",
        )
        .bind(account.as_uuid())
        .bind(keep.map(|session| session.as_uuid()))
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?
        .rows_affected();
        if ended > 0 {
            self.emit(
                &mut transaction,
                account,
                cause,
                &events::sessions_revoked(account, ended),
            )
            .await?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(ended)
    }
}

#[async_trait]
impl TokenStore for PgIdentityStore {
    async fn create_token(&self, new: NewToken, cause: &Cause) -> Result<()> {
        let token = &new.token;
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO api_tokens (id, account_id, name, secret_hash, permissions, created_at, \
             expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(token.id.as_uuid())
        .bind(token.account.as_uuid())
        .bind(&token.name)
        .bind(new.secret.as_bytes().as_slice())
        .bind(token.permissions.names())
        .bind(token.created_at)
        .bind(token.expires_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        self.emit(
            &mut transaction,
            token.account,
            cause,
            &events::token_created(token),
        )
        .await?;
        transaction.commit().await.map_err(storage)
    }

    async fn token(&self, secret: &SecretHash) -> Result<Option<TokenGrant>> {
        let Some(row) = sqlx::query(TOKEN_GRANT.as_str())
            .bind(secret.as_bytes().as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
        else {
            return Ok(None);
        };
        Ok(Some(TokenGrant {
            token: token(&row)?,
            account: account(&row)?,
            permissions: permissions(get(&row, "permissions")?),
        }))
    }

    async fn touch_token(&self, id: TokenId, at: DateTime<Utc>) -> Result<()> {
        sqlx::query("UPDATE api_tokens SET last_used_at = $2 WHERE id = $1")
            .bind(id.as_uuid())
            .bind(at)
            .execute(&self.pool)
            .await
            .map_err(storage)
            .map(|_| ())
    }

    async fn tokens(&self, account: AccountId) -> Result<Vec<ApiToken>> {
        let rows = sqlx::query(TOKENS_OF.as_str())
            .bind(account.as_uuid())
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        rows.iter().map(token).collect()
    }

    async fn revoke_token(
        &self,
        account: AccountId,
        id: TokenId,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<bool> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let revoked = sqlx::query(
            "UPDATE api_tokens SET revoked_at = $3 \
             WHERE id = $1 AND account_id = $2 AND revoked_at IS NULL",
        )
        .bind(id.as_uuid())
        .bind(account.as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?
        .rows_affected()
            > 0;
        if revoked {
            self.emit(
                &mut transaction,
                account,
                cause,
                &events::token_revoked(account, id),
            )
            .await?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(revoked)
    }

    async fn rotate_token(
        &self,
        old: TokenId,
        new: NewToken,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<bool> {
        let token = &new.token;
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let revoked = sqlx::query(
            "UPDATE api_tokens SET revoked_at = $3 \
             WHERE id = $1 AND account_id = $2 AND revoked_at IS NULL",
        )
        .bind(old.as_uuid())
        .bind(token.account.as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?
        .rows_affected()
            > 0;
        if !revoked {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO api_tokens (id, account_id, name, secret_hash, permissions, created_at, \
             expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(token.id.as_uuid())
        .bind(token.account.as_uuid())
        .bind(&token.name)
        .bind(new.secret.as_bytes().as_slice())
        .bind(token.permissions.names())
        .bind(token.created_at)
        .bind(token.expires_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        self.emit(
            &mut transaction,
            token.account,
            cause,
            &events::token_rotated(token, old),
        )
        .await?;
        transaction.commit().await.map_err(storage)?;
        Ok(true)
    }
}

#[async_trait]
impl RoleStore for PgIdentityStore {
    async fn roles(&self) -> Result<Vec<Role>> {
        let rows = sqlx::query(
            "SELECT id, name, description, permissions, built_in FROM roles \
             ORDER BY built_in DESC, id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.iter()
            .map(|row| {
                Ok(Role {
                    id: get(row, "id")?,
                    name: get(row, "name")?,
                    description: get(row, "description")?,
                    permissions: permissions(get(row, "permissions")?),
                    built_in: get(row, "built_in")?,
                })
            })
            .collect()
    }

    async fn create_role(&self, role: Role, cause: &Cause) -> Result<Role> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO roles (id, name, description, permissions, built_in) \
             VALUES ($1, $2, $3, $4, false)",
        )
        .bind(&role.id)
        .bind(&role.name)
        .bind(&role.description)
        .bind(role.permissions.names())
        .execute(&mut *transaction)
        .await
        .map_err(|error| match storage(error) {
            conflict if conflict.code.as_str() == panel_errors::ErrorCode::CONFLICT => {
                PanelError::conflict(format!("the role {} exists", role.id))
            }
            other => other,
        })?;
        self.emit_on(
            &mut transaction,
            ("role", &role.id),
            cause,
            &events::role_created(&role),
        )
        .await?;
        transaction.commit().await.map_err(storage)?;
        Ok(role)
    }

    async fn update_role(&self, role: Role, cause: &Cause) -> Result<Role> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let updated = sqlx::query(
            "UPDATE roles SET name = $2, description = $3, permissions = $4 \
             WHERE id = $1 AND NOT built_in",
        )
        .bind(&role.id)
        .bind(&role.name)
        .bind(&role.description)
        .bind(role.permissions.names())
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        if updated.rows_affected() == 0 {
            return Err(PanelError::not_found(format!(
                "there is no custom role {}",
                role.id
            )));
        }
        self.emit_on(
            &mut transaction,
            ("role", &role.id),
            cause,
            &events::role_updated(&role),
        )
        .await?;
        transaction.commit().await.map_err(storage)?;
        Ok(role)
    }

    async fn delete_role(&self, id: &str, cause: &Cause) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let deleted = sqlx::query("DELETE FROM roles WHERE id = $1 AND NOT built_in")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| match storage(error) {
                held if held.code.as_str() == panel_errors::ErrorCode::INVALID_ARGUMENT => {
                    PanelError::conflict(format!("the role {id} is still granted to accounts"))
                }
                other => other,
            })?;
        if deleted.rows_affected() == 0 {
            return Err(PanelError::not_found(format!(
                "there is no custom role {id}"
            )));
        }
        self.emit_on(
            &mut transaction,
            ("role", id),
            cause,
            &events::role_deleted(id),
        )
        .await?;
        transaction.commit().await.map_err(storage)
    }
}

#[async_trait]
impl GrantStore for PgIdentityStore {
    async fn grants(&self, account: AccountId) -> Result<Vec<Grant>> {
        sqlx::query(
            "SELECT id, account_id, role_id, scope::text AS scope, conditions::text AS conditions, \
             created_at, created_by FROM grants WHERE account_id = $1 ORDER BY created_at, id",
        )
        .bind(account.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?
        .iter()
        .map(|row| {
            let scope: String = get(row, "scope")?;
            let conditions: String = get(row, "conditions")?;
            let corrupt = |_| PanelError::corrupt_state("a stored grant is invalid");
            Ok(Grant {
                id: GrantId::from_uuid(get(row, "id")?),
                account: AccountId::from_uuid(get(row, "account_id")?),
                role: get(row, "role_id")?,
                scope: serde_json::from_str(&scope).map_err(corrupt)?,
                conditions: serde_json::from_str(&conditions).map_err(corrupt)?,
                created_at: get(row, "created_at")?,
                created_by: get(row, "created_by")?,
            })
        })
        .collect()
    }

    async fn create_grant(&self, grant: Grant, cause: &Cause) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO grants (id, account_id, role_id, scope, conditions, created_at, \
             created_by) VALUES ($1, $2, $3, $4::jsonb, $5::jsonb, $6, $7)",
        )
        .bind(grant.id.as_uuid())
        .bind(grant.account.as_uuid())
        .bind(&grant.role)
        .bind(json!(grant.scope).to_string())
        .bind(json!(grant.conditions).to_string())
        .bind(grant.created_at)
        .bind(&grant.created_by)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        self.emit(
            &mut transaction,
            grant.account,
            cause,
            &events::grant_created(&grant),
        )
        .await?;
        transaction.commit().await.map_err(storage)
    }

    async fn delete_grant(&self, account: AccountId, id: GrantId, cause: &Cause) -> Result<bool> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let deleted = sqlx::query("DELETE FROM grants WHERE id = $1 AND account_id = $2")
            .bind(id.as_uuid())
            .bind(account.as_uuid())
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        if deleted.rows_affected() == 0 {
            return Ok(false);
        }
        self.emit(
            &mut transaction,
            account,
            cause,
            &events::grant_deleted(account, id),
        )
        .await?;
        transaction.commit().await.map_err(storage)?;
        Ok(true)
    }
}

#[async_trait]
impl SignInPolicyStore for PgIdentityStore {
    async fn password_sign_in(&self) -> Result<PasswordSignIn> {
        let policy: String =
            sqlx::query_scalar("SELECT password_sign_in FROM sign_in_policy WHERE singleton")
                .fetch_one(&self.pool)
                .await
                .map_err(storage)?;
        PasswordSignIn::parse(&policy)
            .ok_or_else(|| PanelError::corrupt_state("the password sign-in policy is unknown"))
    }

    async fn set_password_sign_in(
        &self,
        policy: PasswordSignIn,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "UPDATE sign_in_policy SET password_sign_in = $1, updated_at = $2 WHERE singleton",
        )
        .bind(policy.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        self.emit_on(
            &mut transaction,
            ("sign_in_policy", "password"),
            cause,
            &events::sign_in_policy_updated(policy),
        )
        .await?;
        transaction.commit().await.map_err(storage)
    }
}
