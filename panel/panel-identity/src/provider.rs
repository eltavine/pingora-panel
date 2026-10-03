//! Identity providers people sign in with (ADR 0018): the port to the
//! OpenID Connect protocol, which an adapter implements.

use async_trait::async_trait;
use panel_errors::Result;
use serde_json::{Map, Value};

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
