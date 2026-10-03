//! The storage the identity rules run on. Every change records its event
//! in the same transaction, named after what happened:
//! `identity.account.created`, `identity.account.updated`,
//! `identity.password.changed`, `identity.login.succeeded`,
//! `identity.break_glass.used`, `identity.login.failed`,
//! `identity.session.ended`, `identity.token.created`,
//! `identity.token.revoked` and `identity.sign_in_policy.updated`.

use crate::{
    Account, AccountId, ApiToken, Grant, GrantId, PasswordSignIn, PermissionSet, Role, SecretHash,
    Session, SessionId, TokenId, Username,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_context::RequestScope;
use panel_errors::Result;
use serde::Serialize;

/// Why a change happens, carried into its event.
#[derive(Clone, Debug)]
pub struct Cause {
    pub scope: RequestScope,
    /// The account acting, or the name a login attempt gave.
    pub actor: String,
}

/// An account with its password state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAccount {
    pub account: Account,
    pub password: Option<StoredPassword>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredPassword {
    /// An Argon2id PHC string.
    pub hash: String,
    /// Consecutive failed logins.
    pub failures: u32,
    /// No login is accepted before this.
    pub retry_after: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct NewAccount {
    pub id: AccountId,
    pub username: Username,
    pub display_name: Option<String>,
    pub password_hash: Option<String>,
    pub roles: Vec<String>,
    pub now: DateTime<Utc>,
    /// Created only while no account exists, atomically.
    pub first: bool,
    pub service: bool,
}

/// What an update changes; `None` leaves a field as it is.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccountChange {
    pub display_name: Option<Option<String>>,
    /// Disabling also ends the account's sessions and revokes its tokens.
    pub disabled: Option<bool>,
    pub roles: Option<Vec<String>>,
    pub break_glass: Option<bool>,
    /// Clears failed logins and re-enables a locked password.
    pub unlock: bool,
}

/// A login attempt, as the audit trail records it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Attempt {
    pub username: String,
    /// The identity provider the person signed in through.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The account is a break-glass account; a successful sign-in with it is
    /// also recorded as `identity.break_glass.used`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub break_glass: bool,
    pub client_address: Option<String>,
    pub user_agent: Option<String>,
}

/// A wrong password for an existing account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub account: AccountId,
    /// Consecutive failures, this one included.
    pub failures: u32,
    pub retry_after: Option<DateTime<Utc>>,
    /// Whether this failure locks the password.
    pub lock: bool,
}

#[derive(Clone, Debug)]
pub struct NewSession {
    pub session: Session,
    pub secret: SecretHash,
}

/// A session found by its secret, with what authenticating needs.
#[derive(Clone, Debug)]
pub struct SessionGrant {
    pub session: Session,
    pub account: Account,
    pub permissions: PermissionSet,
}

#[derive(Clone, Debug)]
pub struct NewToken {
    pub token: ApiToken,
    pub secret: SecretHash,
}

/// A token found by its secret, with its owner and the owner's permissions.
#[derive(Clone, Debug)]
pub struct TokenGrant {
    pub token: ApiToken,
    pub account: Account,
    pub permissions: PermissionSet,
}

/// Accounts, their passwords and the sign-in attempts against them.
#[async_trait]
pub trait AccountStore: Send + Sync {
    async fn has_accounts(&self) -> Result<bool>;

    /// Fails with a conflict when the username is taken, or when `first`
    /// is set and an account exists.
    async fn create_account(&self, account: NewAccount, cause: &Cause) -> Result<Account>;

    async fn account(&self, id: AccountId) -> Result<Option<StoredAccount>>;

    async fn account_named(&self, username: &Username) -> Result<Option<StoredAccount>>;

    async fn accounts(&self) -> Result<Vec<Account>>;

    async fn update_account(
        &self,
        id: AccountId,
        change: AccountChange,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<Account>;

    /// Replaces the password and ends every session of the account but
    /// `keep`.
    async fn set_password(
        &self,
        id: AccountId,
        hash: String,
        keep: Option<SessionId>,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<()>;

    /// Stores a hash of the same password with current parameters.
    async fn rehash_password(&self, id: AccountId, hash: String) -> Result<()>;

    async fn login_failed(&self, failure: Failure, attempt: &Attempt, cause: &Cause) -> Result<()>;

    /// Records a login refused before any password was checked against an
    /// account: unknown, disabled, locked or waiting. Best effort.
    async fn login_refused(&self, attempt: &Attempt, reason: &str, cause: &Cause);
}

/// Sessions: started at sign-in, found by their secret, ended.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Stores the session, clears failed logins and records the login, and
    /// the use of a break-glass account with it.
    async fn create_session(
        &self,
        session: NewSession,
        attempt: &Attempt,
        cause: &Cause,
    ) -> Result<()>;

    async fn session(&self, secret: &SecretHash) -> Result<Option<SessionGrant>>;

    async fn touch_session(&self, id: SessionId, at: DateTime<Utc>) -> Result<()>;

    /// The sessions of an account that have not ended.
    async fn sessions(&self, account: AccountId, now: DateTime<Utc>) -> Result<Vec<Session>>;

    /// Ends a session of `account`; false when it has none such.
    async fn end_session(
        &self,
        account: AccountId,
        id: SessionId,
        reason: &str,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<bool>;

    /// Ends every session of `account` but `keep`; returns how many ended.
    async fn end_sessions(
        &self,
        account: AccountId,
        keep: Option<SessionId>,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<u64>;
}

/// API tokens, found by their secret.
#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn create_token(&self, token: NewToken, cause: &Cause) -> Result<()>;

    async fn token(&self, secret: &SecretHash) -> Result<Option<TokenGrant>>;

    async fn touch_token(&self, id: TokenId, at: DateTime<Utc>) -> Result<()>;

    async fn tokens(&self, account: AccountId) -> Result<Vec<ApiToken>>;

    /// Revokes a token of `account`; false when it has none such.
    async fn revoke_token(
        &self,
        account: AccountId,
        id: TokenId,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<bool>;

    /// Revokes token `old` of the new token's account and stores the new
    /// one in its place, at once; false when the account has no such live
    /// token.
    async fn rotate_token(
        &self,
        old: TokenId,
        new: NewToken,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<bool>;
}

/// Roles and the permissions they grant.
#[async_trait]
pub trait RoleStore: Send + Sync {
    async fn roles(&self) -> Result<Vec<Role>>;

    /// Fails with a conflict when a role with the same identifier exists.
    async fn create_role(&self, role: Role, cause: &Cause) -> Result<Role>;

    /// Replaces a role that is not built in; not found otherwise.
    async fn update_role(&self, role: Role, cause: &Cause) -> Result<Role>;

    /// Deletes a role that is not built in and that no account holds.
    async fn delete_role(&self, id: &str, cause: &Cause) -> Result<()>;
}

/// Roles given with a scope and conditions (ADR 0021).
#[async_trait]
pub trait GrantStore: Send + Sync {
    /// An account's grants, oldest first.
    async fn grants(&self, account: AccountId) -> Result<Vec<Grant>>;

    /// Records `identity.grant.created`.
    async fn create_grant(&self, grant: Grant, cause: &Cause) -> Result<()>;

    /// Records `identity.grant.deleted`; false when the account has no such
    /// grant.
    async fn delete_grant(&self, account: AccountId, id: GrantId, cause: &Cause) -> Result<bool>;
}

/// Who may sign in with a password (ADR 0018).
#[async_trait]
pub trait SignInPolicyStore: Send + Sync {
    /// Who may sign in with a password.
    async fn password_sign_in(&self) -> Result<PasswordSignIn>;

    async fn set_password_sign_in(
        &self,
        policy: PasswordSignIn,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<()>;
}

/// Everything the identity service keeps about accounts and their access.
pub trait IdentityStore:
    AccountStore + SessionStore + TokenStore + RoleStore + GrantStore + SignInPolicyStore
{
}

impl<T> IdentityStore for T where
    T: AccountStore
        + SessionStore
        + TokenStore
        + RoleStore
        + GrantStore
        + SignInPolicyStore
        + ?Sized
{
}
