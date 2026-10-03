//! The relying party side of a provider: discovery, sign-in requests with
//! PKCE, the code exchange, ID token validation and refreshing.

use crate::{
    http::{self, Http, UNRESERVED},
    jose::{self, Algorithm, JwkSet},
};
use ::http::{HeaderValue, StatusCode};
use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use panel_errors::{PanelError, Result};
use panel_identity::{
    OpenIdConnect, ProviderSettings, Refreshed, SignInRequest, SignedIn, VerifiedWorkload,
    WorkloadVerifier,
};
use percent_encoding::utf8_percent_encode;
use ring::{
    digest::{digest, SHA256},
    rand::{SecureRandom, SystemRandom},
};
use rustls::{crypto::ring as provider, ClientConfig};
use rustls_platform_verifier::BuilderVerifierExt;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How long discovery documents and keys are trusted.
const CACHE_FOR: Duration = Duration::from_secs(3600);
/// The least time between fetches of keys a token names but the cache lacks.
const KEY_REFETCH: Duration = Duration::from_secs(60);
/// The clock difference tolerated between the panel and a provider.
const LEEWAY_SECONDS: u64 = 60;

/// The parts of a discovery document the panel uses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Metadata {
    pub issuer: String,
    /// Absent for issuers of workload tokens, which nobody signs in through.
    #[serde(default)]
    pub authorization_endpoint: String,
    #[serde(default)]
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default)]
    pub id_token_signing_alg_values_supported: Vec<String>,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Vec<String>,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
}

struct Cached {
    metadata: Metadata,
    keys: JwkSet,
    fetched: Instant,
    keys_fetched: Instant,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
}

/// Talks to providers on behalf of the panel.
pub struct OidcClient {
    http: Http,
    random: SystemRandom,
    cache: Mutex<HashMap<String, Arc<Cached>>>,
}

impl OidcClient {
    /// A client trusting the platform's certificate authorities.
    pub fn new(timeout: Duration) -> Result<Self> {
        let provider = Arc::new(provider::default_provider());
        let tls = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| PanelError::internal(format!("TLS client setup: {error}")))?
            .with_platform_verifier()
            .map_err(|error| {
                PanelError::unavailable(format!("the system's trusted roots: {error}"))
            })?
            .with_no_client_auth();
        Ok(Self {
            http: Http::new(tls, timeout),
            random: SystemRandom::new(),
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Reads the provider's discovery document and keys, checking them and
    /// that people can sign in through it.
    pub async fn discover(&self, issuer: &str) -> Result<Metadata> {
        let metadata = self.provider(issuer, false).await?.metadata.clone();
        for (endpoint, name) in [
            (&metadata.authorization_endpoint, "authorization endpoint"),
            (&metadata.token_endpoint, "token endpoint"),
        ] {
            if endpoint.is_empty() {
                return Err(PanelError::validation_failed(format!(
                    "the provider publishes no {name}, so nobody can sign in through it"
                )));
            }
        }
        Ok(metadata)
    }

    /// Starts a sign-in: the authorization URL with a fresh state, nonce and
    /// PKCE challenge.
    pub async fn sign_in_request(&self, settings: &ProviderSettings) -> Result<SignInRequest> {
        let provider = self.provider(&settings.issuer, false).await?;
        let state = self.random_value()?;
        let nonce = self.random_value()?;
        let verifier = self.random_value()?;
        let challenge = URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()));
        let scope = scopes(&settings.scopes);
        let endpoint = &provider.metadata.authorization_endpoint;
        let separator = if endpoint.contains('?') { '&' } else { '?' };
        let query = http::encode(&[
            ("response_type", "code"),
            ("client_id", settings.client_id.as_str()),
            ("redirect_uri", settings.redirect_uri.as_str()),
            ("scope", scope.as_str()),
            ("state", state.as_str()),
            ("nonce", nonce.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ]);
        Ok(SignInRequest {
            url: format!("{endpoint}{separator}{query}"),
            state,
            nonce,
            verifier,
        })
    }

    /// Exchanges the code from the callback and validates the ID token.
    pub async fn complete(
        &self,
        settings: &ProviderSettings,
        code: &str,
        verifier: &str,
        nonce: &str,
    ) -> Result<SignedIn> {
        let provider = self.provider(&settings.issuer, false).await?;
        let response = self
            .token(
                settings,
                &provider.metadata,
                &[
                    ("grant_type", "authorization_code"),
                    ("code", code),
                    ("redirect_uri", settings.redirect_uri.as_str()),
                    ("code_verifier", verifier),
                ],
            )
            .await?;
        let tokens: TokenResponse = match response {
            Ok(tokens) => tokens,
            Err(error) => {
                return Err(PanelError::unauthenticated(format!(
                    "the provider refused the sign-in: {error}"
                )))
            }
        };
        let id_token = tokens
            .id_token
            .ok_or_else(|| PanelError::unauthenticated("the provider sent no ID token"))?;
        let claims = self.validate(settings, provider, &id_token, nonce).await?;
        let subject = claims
            .get("sub")
            .and_then(Value::as_str)
            .filter(|subject| !subject.is_empty())
            .ok_or_else(|| PanelError::unauthenticated("the ID token names no subject"))?
            .to_owned();
        Ok(SignedIn {
            subject,
            claims,
            refresh_token: tokens.refresh_token,
        })
    }

    /// Asks the provider whether the person behind `refresh_token` may
    /// still sign in.
    pub async fn refresh(
        &self,
        settings: &ProviderSettings,
        refresh_token: &str,
    ) -> Result<Refreshed> {
        let provider = self.provider(&settings.issuer, false).await?;
        let response = self
            .token(
                settings,
                &provider.metadata,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                ],
            )
            .await?;
        Ok(match response {
            Ok(tokens) => Refreshed::Valid {
                refresh_token: tokens.refresh_token,
            },
            Err(error) if error == "invalid_grant" => Refreshed::Refused,
            Err(error) => {
                return Err(PanelError::unavailable(format!(
                    "the provider could not refresh the sign-in: {error}"
                )))
            }
        })
    }

    fn random_value(&self) -> Result<String> {
        let mut bytes = [0; 32];
        self.random
            .fill(&mut bytes)
            .map_err(|_| PanelError::internal("no randomness is available"))?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    /// POSTs to the token endpoint; `Err` inside carries the OAuth error.
    async fn token(
        &self,
        settings: &ProviderSettings,
        metadata: &Metadata,
        form: &[(&str, &str)],
    ) -> Result<std::result::Result<TokenResponse, String>> {
        let mut form: Vec<(&str, &str)> = form.to_vec();
        let mut authorization = None;
        match &settings.client_secret {
            Some(secret) if prefers_post(metadata) => {
                form.push(("client_id", &settings.client_id));
                form.push(("client_secret", secret));
            }
            Some(secret) => {
                // RFC 6749 §2.3.1 encodes both before Basic authentication.
                let credentials = format!(
                    "{}:{}",
                    utf8_percent_encode(&settings.client_id, UNRESERVED),
                    utf8_percent_encode(secret, UNRESERVED)
                );
                let mut value =
                    HeaderValue::try_from(format!("Basic {}", STANDARD.encode(credentials)))
                        .map_err(|_| PanelError::internal("client credentials are not a header"))?;
                value.set_sensitive(true);
                authorization = Some(value);
            }
            None => form.push(("client_id", &settings.client_id)),
        }
        let (status, body) = self
            .http
            .post_form(&metadata.token_endpoint, &form, authorization)
            .await
            .map_err(|error| unavailable("the token endpoint", &error))?;
        if status.is_success() {
            return serde_json::from_slice(&body).map(Ok).map_err(|_| {
                PanelError::unavailable(
                    "the token endpoint answered with something else than tokens",
                )
            });
        }
        if status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED {
            if let Ok(error) = serde_json::from_slice::<TokenError>(&body) {
                return Ok(Err(error.error));
            }
        }
        Err(PanelError::unavailable(format!(
            "the token endpoint answered {status}"
        )))
    }

    async fn validate(
        &self,
        settings: &ProviderSettings,
        provider: Arc<Cached>,
        id_token: &str,
        nonce: &str,
    ) -> Result<Map<String, Value>> {
        let claims = self
            .signed_claims(&settings.issuer, provider, id_token)
            .await?;
        check_claims(&claims, settings, nonce, now_seconds())?;
        Ok(claims)
    }

    /// The claims of a token whose signature one of the issuer's keys
    /// verifies, reading the keys again once if the issuer may have rotated
    /// them.
    async fn signed_claims(
        &self,
        issuer: &str,
        provider: Arc<Cached>,
        token: &str,
    ) -> Result<Map<String, Value>> {
        let allowed = algorithms(&provider.metadata);
        let verified = match jose::verify(token, &provider.keys, &allowed) {
            Ok(verified) => verified,
            Err(error) if provider.keys_fetched.elapsed() >= KEY_REFETCH => {
                let refreshed = self.provider(issuer, true).await?;
                jose::verify(token, &refreshed.keys, &allowed).map_err(|_| error)?
            }
            Err(error) => return Err(error),
        };
        serde_json::from_slice(&verified.payload)
            .map_err(|_| PanelError::unauthenticated("the token's claims are not a JSON object"))
    }

    /// The provider's checked discovery document and keys, from the cache
    /// unless they are old or `refresh_keys` asks for new keys.
    async fn provider(&self, issuer: &str, refresh_keys: bool) -> Result<Arc<Cached>> {
        let cached = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(issuer)
            .cloned();
        if let Some(cached) = &cached {
            if cached.fetched.elapsed() < CACHE_FOR && !refresh_keys {
                return Ok(Arc::clone(cached));
            }
        }
        let metadata = match cached
            .as_ref()
            .filter(|cached| cached.fetched.elapsed() < CACHE_FOR)
        {
            Some(cached) => cached.metadata.clone(),
            None => self.fetch_metadata(issuer).await?,
        };
        let keys: JwkSet = self
            .get_json(&metadata.jwks_uri, "the provider's keys")
            .await?;
        let fresh = Arc::new(Cached {
            fetched: cached
                .filter(|cached| cached.fetched.elapsed() < CACHE_FOR)
                .map_or_else(Instant::now, |cached| cached.fetched),
            keys_fetched: Instant::now(),
            metadata,
            keys,
        });
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(issuer.to_owned(), Arc::clone(&fresh));
        Ok(fresh)
    }

    async fn fetch_metadata(&self, issuer: &str) -> Result<Metadata> {
        require_secure(issuer, "the issuer")?;
        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let metadata: Metadata = self.get_json(&url, "the discovery document").await?;
        if metadata.issuer != issuer {
            return Err(PanelError::validation_failed(format!(
                "the discovery document names the issuer {:?} instead of {issuer:?}",
                metadata.issuer
            )));
        }
        require_secure(&metadata.jwks_uri, "the key set")?;
        for (endpoint, name) in [
            (
                &metadata.authorization_endpoint,
                "the authorization endpoint",
            ),
            (&metadata.token_endpoint, "the token endpoint"),
        ] {
            if !endpoint.is_empty() {
                require_secure(endpoint, name)?;
            }
        }
        if !metadata.code_challenge_methods_supported.is_empty()
            && !metadata
                .code_challenge_methods_supported
                .iter()
                .any(|method| method == "S256")
        {
            return Err(PanelError::validation_failed(
                "the provider does not support PKCE with S256",
            ));
        }
        Ok(metadata)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str, what: &str) -> Result<T> {
        let (status, body) = self
            .http
            .get(url)
            .await
            .map_err(|error| unavailable(what, &error))?;
        if !status.is_success() {
            return Err(PanelError::unavailable(format!("{what} answered {status}")));
        }
        serde_json::from_slice(&body).map_err(|_| {
            PanelError::unavailable(format!("{what} is not what OpenID Connect describes"))
        })
    }
}

#[async_trait]
impl OpenIdConnect for OidcClient {
    async fn check(&self, issuer: &str) -> Result<()> {
        self.discover(issuer).await.map(|_| ())
    }

    async fn sign_in_request(&self, settings: &ProviderSettings) -> Result<SignInRequest> {
        OidcClient::sign_in_request(self, settings).await
    }

    async fn complete(
        &self,
        settings: &ProviderSettings,
        code: &str,
        verifier: &str,
        nonce: &str,
    ) -> Result<SignedIn> {
        OidcClient::complete(self, settings, code, verifier, nonce).await
    }

    async fn refresh(&self, settings: &ProviderSettings, refresh_token: &str) -> Result<Refreshed> {
        OidcClient::refresh(self, settings, refresh_token).await
    }
}

fn unavailable(what: &str, error: &str) -> PanelError {
    PanelError::unavailable(format!("{what} is unreachable: {error}")).retryable(true)
}

fn scopes(requested: &[String]) -> String {
    let mut scopes = vec!["openid"];
    scopes.extend(
        requested
            .iter()
            .map(String::as_str)
            .filter(|scope| *scope != "openid"),
    );
    scopes.join(" ")
}

fn prefers_post(metadata: &Metadata) -> bool {
    let methods = &metadata.token_endpoint_auth_methods_supported;
    !methods.is_empty()
        && !methods.iter().any(|method| method == "client_secret_basic")
        && methods.iter().any(|method| method == "client_secret_post")
}

fn algorithms(metadata: &Metadata) -> Vec<Algorithm> {
    let named: Vec<Algorithm> = metadata
        .id_token_signing_alg_values_supported
        .iter()
        .filter_map(|name| Algorithm::parse(name))
        .collect();
    if named.is_empty() {
        // OpenID Connect Core §15.1: RS256 is the default.
        vec![Algorithm::Rs256]
    } else {
        named
    }
}

/// HTTPS, or HTTP on loopback for tests and local providers.
fn require_secure(url: &str, what: &str) -> Result<()> {
    if http::secure(url) {
        Ok(())
    } else {
        Err(PanelError::validation_failed(format!(
            "{what} {url:?} must be an HTTPS URL"
        )))
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[async_trait]
impl WorkloadVerifier for OidcClient {
    async fn verify(&self, token: &str, issuers: &[String]) -> Result<VerifiedWorkload> {
        let issuer = unverified_issuer(token)?;
        if !issuers.contains(&issuer) {
            return Err(PanelError::unauthenticated(
                "no workload identity trusts the token's issuer",
            ));
        }
        let provider = self.provider(&issuer, false).await?;
        let claims = self.signed_claims(&issuer, provider, token).await?;
        check_workload_claims(&claims, &issuer, now_seconds())?;
        Ok(VerifiedWorkload {
            subject: claims
                .get("sub")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            audiences: audiences(&claims).into_iter().map(str::to_owned).collect(),
            issuer,
            claims,
        })
    }
}

/// The issuer a token names, before anything about it is trusted; only
/// used to pick the keys that must then verify it.
fn unverified_issuer(token: &str) -> Result<String> {
    let refused = || PanelError::unauthenticated("the workload token is not a signed JWT");
    let payload = token.split('.').nth(1).ok_or_else(refused)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| refused())?;
    let claims: Map<String, Value> = serde_json::from_slice(&bytes).map_err(|_| refused())?;
    claims
        .get("iss")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(refused)
}

fn audiences(claims: &Map<String, Value>) -> Vec<&str> {
    match claims.get("aud") {
        Some(Value::String(audience)) => vec![audience.as_str()],
        Some(Value::Array(audiences)) => audiences.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// RFC 7519 §4.1 for a workload token whose signature verified.
pub(crate) fn check_workload_claims(
    claims: &Map<String, Value>,
    issuer: &str,
    now: u64,
) -> Result<()> {
    let refused = |message: &str| {
        Err(PanelError::unauthenticated(format!(
            "the workload token {message}"
        )))
    };
    if claims.get("iss").and_then(Value::as_str) != Some(issuer) {
        return refused("comes from another issuer");
    }
    if claims
        .get("sub")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return refused("names no subject");
    }
    if audiences(claims).is_empty() {
        return refused("names no audience");
    }
    let Some(expires) = claims.get("exp").and_then(Value::as_u64) else {
        return refused("has no expiry");
    };
    if expires + LEEWAY_SECONDS <= now {
        return refused("has expired");
    }
    for (claim, what) in [
        ("nbf", "is not valid yet"),
        ("iat", "was issued in the future"),
    ] {
        if claims
            .get(claim)
            .and_then(Value::as_u64)
            .is_some_and(|time| time > now + LEEWAY_SECONDS)
        {
            return refused(what);
        }
    }
    Ok(())
}

/// OpenID Connect Core §3.1.3.7 for a token whose signature verified.
pub(crate) fn check_claims(
    claims: &Map<String, Value>,
    settings: &ProviderSettings,
    nonce: &str,
    now: u64,
) -> Result<()> {
    let refused = |message: &str| {
        Err(PanelError::unauthenticated(format!(
            "the ID token {message}"
        )))
    };
    if claims.get("iss").and_then(Value::as_str) != Some(settings.issuer.as_str()) {
        return refused("comes from another issuer");
    }
    let audiences: Vec<&str> = match claims.get("aud") {
        Some(Value::String(audience)) => vec![audience.as_str()],
        Some(Value::Array(audiences)) => audiences.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !audiences.contains(&settings.client_id.as_str()) {
        return refused("is meant for another client");
    }
    match claims.get("azp").and_then(Value::as_str) {
        Some(party) if party != settings.client_id => {
            return refused("was issued to another party")
        }
        None if audiences.len() > 1 => {
            return refused("names several audiences but no authorized party")
        }
        _ => {}
    }
    let Some(expires) = claims.get("exp").and_then(Value::as_u64) else {
        return refused("has no expiry");
    };
    if expires + LEEWAY_SECONDS <= now {
        return refused("has expired");
    }
    if claims
        .get("iat")
        .and_then(Value::as_u64)
        .is_some_and(|issued| issued > now + LEEWAY_SECONDS)
    {
        return refused("was issued in the future");
    }
    if claims.get("nonce").and_then(Value::as_str) != Some(nonce) {
        return refused("answers another sign-in");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings() -> ProviderSettings {
        ProviderSettings {
            issuer: "https://id.example".into(),
            client_id: "panel".into(),
            client_secret: None,
            scopes: vec!["profile".into(), "openid".into()],
            redirect_uri: "https://panel.example/callback".into(),
        }
    }

    #[test]
    fn workload_claims_follow_rfc_7519() {
        let now = 1_000_000;
        let good = json!({
            "iss": "https://token.example", "aud": ["pingora-panel"], "sub": "repo:a/b",
            "exp": now + 300, "iat": now, "nbf": now
        });
        let check = |claims: &Value| {
            check_workload_claims(claims.as_object().unwrap(), "https://token.example", now)
        };
        assert!(check(&good).is_ok());
        for (field, value) in [
            ("iss", json!("https://evil.example")),
            ("sub", json!("")),
            ("aud", json!([])),
            ("exp", json!(now - 61)),
            ("exp", Value::Null),
            ("nbf", json!(now + 61)),
            ("iat", json!(now + 61)),
        ] {
            let mut bad = good.clone();
            bad[field] = value.clone();
            assert!(check(&bad).is_err(), "{field} = {value}");
        }
    }

    #[test]
    fn claims_follow_openid_connect_core() {
        let now = 1_000_000;
        let good = json!({
            "iss": "https://id.example", "aud": "panel", "sub": "alice",
            "exp": now + 300, "iat": now, "nonce": "n-1"
        });
        let check =
            |claims: &Value| check_claims(claims.as_object().unwrap(), &settings(), "n-1", now);
        assert!(check(&good).is_ok());
        for (field, value) in [
            ("iss", json!("https://evil.example")),
            ("aud", json!("other")),
            ("aud", json!(["panel", "other"])),
            ("azp", json!("other")),
            ("exp", json!(now - 61)),
            ("exp", Value::Null),
            ("iat", json!(now + 120)),
            ("nonce", json!("n-2")),
        ] {
            let mut claims = good.clone();
            claims[field] = value.clone();
            assert!(check(&claims).is_err(), "{field}: {value}");
        }
        let mut shared = good.clone();
        shared["aud"] = json!(["panel", "other"]);
        shared["azp"] = json!("panel");
        assert!(check(&shared).is_ok());
        let mut late = good;
        late["exp"] = json!(now - 30);
        assert!(check(&late).is_ok(), "a minute of leeway");
    }

    #[test]
    fn openid_is_always_requested_once() {
        assert_eq!(scopes(&settings().scopes), "openid profile");
        assert!(require_secure("http://id.example", "the issuer").is_err());
    }
}
