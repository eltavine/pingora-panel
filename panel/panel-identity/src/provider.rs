//! Identity providers people sign in with (ADR 0018): how the panel keeps
//! them, and the port to the OpenID Connect protocol, which an adapter
//! implements.

use crate::{
    store::{Attempt, Cause, NewAccount, NewSession},
    Account, AccountId, SecretHash,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_errors::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The claims that give a person's attributes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimNames {
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub groups: String,
}

impl Default for ClaimNames {
    fn default() -> Self {
        Self {
            username: "preferred_username".into(),
            display_name: "name".into(),
            email: "email".into(),
            groups: "groups".into(),
        }
    }
}

/// A role that members of a provider's group receive.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupRole {
    pub group: String,
    pub role: String,
}

/// An identity provider as the panel keeps it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityProvider {
    pub id: String,
    pub display_name: String,
    pub issuer: String,
    pub client_id: String,
    /// The sealed client secret of a confidential client.
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub claims: ClaimNames,
    pub group_roles: Vec<GroupRole>,
    /// Whether people unknown to the panel get an account on first sign-in.
    pub create_accounts: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A sign-in on its way through the provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingSignIn {
    /// A hash of the state the browser carries back.
    pub state: SecretHash,
    pub provider: String,
    pub nonce: String,
    /// The PKCE code verifier.
    pub verifier: String,
    /// The panel path to return to.
    pub return_to: String,
    pub expires_at: DateTime<Utc>,
}

/// A provider's subject linked to an account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderLink {
    pub provider: String,
    pub subject: String,
    pub account: AccountId,
    /// The roles the provider's group mappings added to the account.
    pub granted_roles: Vec<String>,
}

/// A sign-in the provider vouched for, stored at once.
#[derive(Clone, Debug)]
pub struct ProviderSignIn {
    pub link: ProviderLink,
    /// The account to create with the link, for a person new to the panel.
    pub new_account: Option<NewAccount>,
    /// The account's roles after the provider's are recalculated.
    pub roles: Vec<String>,
    pub session: NewSession,
    /// The provider's refresh token, sealed, kept with the session.
    pub refresh_token: Option<String>,
}

/// Where identity providers, their links and sign-ins are kept. Changes
/// record `identity.provider.created`, `identity.provider.updated`,
/// `identity.provider.deleted` and, for sign-ins,
/// `identity.login.succeeded` with the provider.
#[async_trait]
pub trait ProviderStore: Send + Sync {
    async fn providers(&self) -> Result<Vec<IdentityProvider>>;

    async fn provider(&self, id: &str) -> Result<Option<IdentityProvider>>;

    /// Creates or replaces the provider; returns whether it was created.
    async fn put_provider(&self, provider: IdentityProvider, cause: &Cause) -> Result<bool>;

    /// Deletes the provider with its links; the accounts stay.
    async fn delete_provider(&self, id: &str, cause: &Cause) -> Result<()>;

    async fn save_sign_in(&self, pending: PendingSignIn) -> Result<()>;

    /// Takes the sign-in saved under `state`, once, unless it expired.
    async fn take_sign_in(
        &self,
        state: &SecretHash,
        now: DateTime<Utc>,
    ) -> Result<Option<PendingSignIn>>;

    async fn link(&self, provider: &str, subject: &str) -> Result<Option<ProviderLink>>;

    /// Creates the account when it is new, links it, sets its roles and
    /// stores the session. Fails with a conflict when a new account's name
    /// is taken.
    async fn sign_in_with_provider(
        &self,
        sign_in: ProviderSignIn,
        attempt: &Attempt,
        cause: &Cause,
    ) -> Result<Account>;
}

/// What the panel tells a provider about itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSettings {
    pub issuer: String,
    pub client_id: String,
    /// Absent for public clients, which rely on PKCE alone.
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub redirect_uri: String,
}

/// Where to send the browser, and what the callback must match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignInRequest {
    pub url: String,
    pub state: String,
    pub nonce: String,
    /// The PKCE code verifier, kept until the callback.
    pub verifier: String,
}

/// A person the provider vouched for.
#[derive(Clone, Debug, PartialEq)]
pub struct SignedIn {
    pub subject: String,
    /// Every claim of the ID token.
    pub claims: Map<String, Value>,
    pub refresh_token: Option<String>,
}

/// What became of a refresh.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Refreshed {
    /// The provider still vouches; it may have rotated the refresh token.
    Valid { refresh_token: Option<String> },
    /// The provider refused the grant: the person's access has ended.
    Refused,
}

/// The OpenID Connect relying party.
#[async_trait]
pub trait OpenIdConnect: Send + Sync {
    /// Reads and checks the provider's discovery document and keys.
    async fn check(&self, issuer: &str) -> Result<()>;

    async fn sign_in_request(&self, settings: &ProviderSettings) -> Result<SignInRequest>;

    /// Exchanges the code from the callback and validates the ID token.
    async fn complete(
        &self,
        settings: &ProviderSettings,
        code: &str,
        verifier: &str,
        nonce: &str,
    ) -> Result<SignedIn>;

    /// Asks the provider whether the person may still sign in.
    async fn refresh(&self, settings: &ProviderSettings, refresh_token: &str) -> Result<Refreshed>;
}
