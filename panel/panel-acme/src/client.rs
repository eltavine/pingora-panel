//! Accounts, orders and renewal information over RFC 8555.

use crate::challenges::{Challenge, ChallengeKind, ChallengeSolver};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::{client::legacy::Client as HyperClient, rt::TokioExecutor};
use instant_acme::{
    Account, AccountBuilder, AccountCredentials, AuthorizationStatus, BodyWrapper,
    CertificateIdentifier, ChallengeType, Error as AcmeError, ExternalAccountKey, HttpClient,
    Identifier, NewAccount, NewOrder, OrderStatus, Problem, RetryPolicy,
};
use panel_certificates::signing_request;
use panel_errors::{PanelError, Result};
use rustls::{crypto::ring, ClientConfig, RootCertStore};
use rustls_pki_types::{pem::PemObject, CertificateDer};
use rustls_platform_verifier::BuilderVerifierExt;
use std::{borrow::Cow, future::Future, net::IpAddr, sync::Arc, time::Duration};
use zeroize::Zeroizing;

/// Let's Encrypt's production directory.
pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";
/// Let's Encrypt's staging directory, for trying configurations out.
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// Where a CA's ACME server is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Directory {
    pub url: String,
    /// PEM certificates to trust for the server instead of the platform's
    /// roots, for private CAs.
    pub ca_bundle: Option<String>,
}

/// An external account binding (RFC 8555 §7.3.4): the key identifier and
/// base64url-encoded MAC key a CA hands out for one registration.
pub struct ExternalAccount {
    pub key_id: String,
    pub mac_key: Zeroizing<String>,
}

/// What a new account is registered with.
pub struct Registration {
    /// `mailto:` URIs the CA may contact.
    pub contact: Vec<String>,
    pub terms_of_service_agreed: bool,
    pub external_account: Option<ExternalAccount>,
}

/// A registered account.
pub struct Registered {
    /// The account's URL at the CA.
    pub url: String,
    /// The account key and URLs; opaque and secret.
    pub credentials: Zeroizing<String>,
}

impl std::fmt::Debug for Registered {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Registered")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// A certificate to order.
#[derive(Clone, Copy, Debug)]
pub struct OrderRequest<'a> {
    /// DNS names, wildcards or IP addresses.
    pub names: &'a [String],
    pub challenge: ChallengeKind,
    /// The renewal identifier (RFC 9773) of the certificate this one
    /// replaces; ignored by CAs without renewal information.
    pub replaces: Option<&'a str>,
}

/// An issued chain, leaf first, with the key of its leaf.
pub struct Issued {
    pub chain: String,
    pub key: Zeroizing<String>,
}

impl std::fmt::Debug for Issued {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Issued").finish_non_exhaustive()
    }
}

/// When a CA suggests renewing a certificate (RFC 9773 §4.2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenewalWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// A page explaining an unusually early window, such as a revocation.
    pub explanation_url: Option<String>,
    /// When to ask again.
    pub next_check: DateTime<Utc>,
}

impl RenewalWindow {
    /// A uniformly random moment within the window, so that clients do not
    /// renew all at once; the window's end when it has passed.
    pub fn pick(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        if self.end <= now {
            return now;
        }
        let start = self.start.max(now);
        let span = (self.end - start).num_seconds().max(0);
        let mut random = [0_u8; 8];
        if getrandom::fill(&mut random).is_err() || span == 0 {
            return start;
        }
        let offset = u64::from_le_bytes(random) % (span.unsigned_abs() + 1);
        start + chrono::Duration::seconds(i64::try_from(offset).unwrap_or(0))
    }
}

/// Speaks ACME with the ring provider, giving up on each operation after
/// `timeout`.
#[derive(Clone, Debug)]
pub struct AcmeClient {
    timeout: Duration,
}

impl Default for AcmeClient {
    fn default() -> Self {
        Self::new(Duration::from_secs(120))
    }
}

/// A CA's refusal or failure, as far as it tells.
fn acme_error(error: AcmeError) -> PanelError {
    match error {
        AcmeError::Api(problem) => problem_error(&problem),
        AcmeError::Timeout(_) => {
            PanelError::deadline_exceeded("the CA did not finish in time").retryable(true)
        }
        AcmeError::Unsupported(feature) => {
            PanelError::unsupported_capability(format!("the CA does not support {feature}"))
        }
        AcmeError::Http(_) | AcmeError::Hyper(_) | AcmeError::InvalidUri(_) => {
            PanelError::unavailable(format!("the CA cannot be reached: {error}")).retryable(true)
        }
        AcmeError::Other(error) => {
            PanelError::unavailable(format!("the CA cannot be reached: {error}")).retryable(true)
        }
        other => PanelError::internal(format!("ACME failed: {other}")),
    }
}

fn problem_error(problem: &Problem) -> PanelError {
    let kind = problem
        .r#type
        .as_deref()
        .and_then(|kind| kind.strip_prefix("urn:ietf:params:acme:error:"))
        .unwrap_or("unknown");
    let detail = problem.detail.as_deref().unwrap_or("no detail given");
    let message = format!("the CA refused ({kind}): {detail}");
    match kind {
        "rateLimited" => PanelError::resource_exhausted(message).retryable(true),
        "serverInternal" => PanelError::unavailable(message).retryable(true),
        _ => PanelError::validation_failed(message),
    }
}

fn identifier(name: &str) -> Identifier {
    match name.parse::<IpAddr>() {
        Ok(address) => Identifier::Ip(address),
        Err(_) => Identifier::Dns(name.to_owned()),
    }
}

fn certificate_identifier(value: &str) -> Result<CertificateIdentifier<'_>> {
    let (authority_key_identifier, serial) = value
        .split_once('.')
        .filter(|(_, serial)| !serial.contains('.'))
        .ok_or_else(|| {
            PanelError::invalid_argument(format!("{value:?} is not a renewal identifier"))
        })?;
    Ok(CertificateIdentifier {
        authority_key_identifier: Cow::Borrowed(authority_key_identifier),
        serial: Cow::Borrowed(serial),
    })
}

fn time(value: impl Into<i128>) -> DateTime<Utc> {
    let nanoseconds: i128 = value.into();
    i64::try_from(nanoseconds.div_euclid(1_000_000_000))
        .ok()
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

impl AcmeClient {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    async fn within<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        tokio::time::timeout(self.timeout, work)
            .await
            .map_err(|_| {
                PanelError::deadline_exceeded(format!(
                    "the CA did not answer within {} seconds",
                    self.timeout.as_secs()
                ))
                .retryable(true)
            })?
    }

    fn builder(directory: &Directory) -> Result<AccountBuilder> {
        let provider = Arc::new(ring::default_provider());
        let versions = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .map_err(|error| PanelError::internal(format!("TLS client setup: {error}")))?;
        let config = match &directory.ca_bundle {
            Some(bundle) => {
                let mut roots = RootCertStore::empty();
                for certificate in CertificateDer::pem_slice_iter(bundle.as_bytes()) {
                    let certificate = certificate.map_err(|_| {
                        PanelError::validation_failed("the CA bundle is not PEM certificates")
                    })?;
                    roots.add(certificate).map_err(|error| {
                        PanelError::validation_failed(format!(
                            "the CA bundle holds an unusable certificate: {error}"
                        ))
                    })?;
                }
                if roots.is_empty() {
                    return Err(PanelError::validation_failed(
                        "the CA bundle holds no certificate",
                    ));
                }
                versions.with_root_certificates(roots).with_no_client_auth()
            }
            None => versions
                .with_platform_verifier()
                .map_err(|error| {
                    PanelError::unavailable(format!("the system's trusted roots: {error}"))
                })?
                .with_no_client_auth(),
        };
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_only()
            .enable_http1()
            .enable_http2()
            .build();
        let http: HyperClient<_, BodyWrapper<Bytes>> =
            HyperClient::builder(TokioExecutor::new()).build(connector);
        Ok(Account::builder_with_http(
            Box::new(http) as Box<dyn HttpClient>
        ))
    }

    async fn account(&self, directory: &Directory, credentials: &str) -> Result<Account> {
        let credentials: AccountCredentials = serde_json::from_str(credentials)
            .map_err(|_| PanelError::corrupt_state("the account credentials are unreadable"))?;
        Self::builder(directory)?
            .from_credentials(credentials)
            .await
            .map_err(acme_error)
    }

    /// Registers a new account with the CA at `directory`.
    pub async fn register(
        &self,
        directory: &Directory,
        registration: &Registration,
    ) -> Result<Registered> {
        if !registration.terms_of_service_agreed {
            return Err(PanelError::validation_failed(
                "the CA's terms of service must be agreed to",
            ));
        }
        let external = registration
            .external_account
            .as_ref()
            .map(|binding| {
                let key = base64_url(&binding.mac_key).ok_or_else(|| {
                    PanelError::validation_failed(
                        "the external account MAC key is not base64url-encoded",
                    )
                })?;
                Ok::<_, PanelError>(ExternalAccountKey::new(binding.key_id.clone(), &key))
            })
            .transpose()?;
        let contact: Vec<&str> = registration.contact.iter().map(String::as_str).collect();
        self.within(async {
            let (account, credentials) = Self::builder(directory)?
                .create(
                    &NewAccount {
                        contact: &contact,
                        terms_of_service_agreed: true,
                        only_return_existing: false,
                    },
                    directory.url.clone(),
                    external.as_ref(),
                )
                .await
                .map_err(acme_error)?;
            let credentials = serde_json::to_string(&credentials)
                .map_err(|_| PanelError::internal("account credentials do not serialize"))?;
            Ok(Registered {
                url: account.id().to_owned(),
                credentials: Zeroizing::new(credentials),
            })
        })
        .await
    }

    /// Orders a certificate for `request.names`, answering each pending
    /// authorization's challenge with `solver`, and returns the chain with
    /// the new key it certifies. Everything `solver` set up is removed.
    pub async fn issue(
        &self,
        directory: &Directory,
        credentials: &str,
        request: OrderRequest<'_>,
        solver: &dyn ChallengeSolver,
    ) -> Result<Issued> {
        if request.challenge == ChallengeKind::Http01
            && request.names.iter().any(|name| name.starts_with("*."))
        {
            return Err(PanelError::validation_failed(
                "wildcard names can only be validated with DNS-01",
            ));
        }
        let signing = signing_request(request.names)?;
        let mut presented = Vec::new();
        let issued = self
            .within(self.order(
                directory,
                credentials,
                request,
                solver,
                &mut presented,
                &signing.der,
            ))
            .await;
        for challenge in &presented {
            if let Err(error) = solver.clean_up(challenge).await {
                tracing::warn!(error_code = %error.code, identifier = %challenge.identifier, "challenge not removed");
            }
        }
        Ok(Issued {
            chain: issued?,
            key: signing.key,
        })
    }

    async fn order(
        &self,
        directory: &Directory,
        credentials: &str,
        request: OrderRequest<'_>,
        solver: &dyn ChallengeSolver,
        presented: &mut Vec<Challenge>,
        csr: &[u8],
    ) -> Result<String> {
        let account = self.account(directory, credentials).await?;
        let identifiers: Vec<Identifier> =
            request.names.iter().map(|name| identifier(name)).collect();
        let replaces = request.replaces.map(certificate_identifier).transpose()?;
        let mut order = match replaces {
            Some(replaces) => {
                match account
                    .new_order(&NewOrder::new(&identifiers).replaces(replaces))
                    .await
                {
                    Err(AcmeError::Unsupported(_)) => {
                        account.new_order(&NewOrder::new(&identifiers)).await
                    }
                    other => other,
                }
            }
            None => account.new_order(&NewOrder::new(&identifiers)).await,
        }
        .map_err(acme_error)?;

        let kind = match request.challenge {
            ChallengeKind::Http01 => ChallengeType::Http01,
            ChallengeKind::Dns01 => ChallengeType::Dns01,
        };
        let mut authorizations = order.authorizations();
        while let Some(authorization) = authorizations.next().await {
            let mut authorization = authorization.map_err(acme_error)?;
            let identifier = authorization.identifier();
            let name = match identifier.identifier {
                Identifier::Dns(name) => name.clone(),
                Identifier::Ip(address) => address.to_string(),
                other => format!("{other:?}"),
            };
            let wildcard = identifier.wildcard;
            match authorization.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => {}
                status => {
                    return Err(PanelError::validation_failed(format!(
                        "the CA's authorization for {name} is {status:?}"
                    )))
                }
            }
            let mut challenge = authorization.challenge(kind.clone()).ok_or_else(|| {
                PanelError::validation_failed(format!(
                    "the CA offers no {} challenge for {name}",
                    request.challenge.as_str()
                ))
            })?;
            let key_authorization = challenge.key_authorization();
            let pending = Challenge {
                kind: request.challenge,
                identifier: name,
                wildcard,
                token: challenge.token.clone(),
                key_authorization: key_authorization.as_str().to_owned(),
                dns_value: key_authorization.dns_value(),
            };
            solver.present(&pending).await?;
            presented.push(pending);
            challenge.set_ready().await.map_err(acme_error)?;
        }

        let patience = RetryPolicy::new()
            .initial_delay(Duration::from_millis(500))
            .timeout(self.timeout);
        let status = order.poll_ready(&patience).await.map_err(acme_error)?;
        if status != OrderStatus::Ready {
            return Err(self.rejection(&mut order).await);
        }
        order.finalize_csr(csr).await.map_err(acme_error)?;
        order.poll_certificate(&patience).await.map_err(acme_error)
    }

    /// Why the CA found an order invalid: the first failed challenge.
    async fn rejection(&self, order: &mut instant_acme::Order) -> PanelError {
        let mut authorizations = order.authorizations();
        while let Some(Ok(mut authorization)) = authorizations.next().await {
            let Ok(state) = authorization.refresh().await else {
                continue;
            };
            if let Some(problem) = state
                .challenges
                .iter()
                .find_map(|challenge| challenge.error.as_ref())
            {
                return problem_error(problem);
            }
        }
        PanelError::validation_failed("the CA could not validate the order")
    }

    /// The CA's suggested renewal window for the certificate with renewal
    /// identifier `identifier`, or `None` when the CA does not offer
    /// renewal information.
    pub async fn renewal_window(
        &self,
        directory: &Directory,
        credentials: &str,
        identifier: &str,
    ) -> Result<Option<RenewalWindow>> {
        let certificate = certificate_identifier(identifier)?;
        self.within(async {
            let account = self.account(directory, credentials).await?;
            match account.renewal_info(&certificate).await {
                Ok((info, wait)) => Ok(Some(RenewalWindow {
                    start: time(info.suggested_window.start.unix_timestamp_nanos()),
                    end: time(info.suggested_window.end.unix_timestamp_nanos()),
                    explanation_url: info.explanation_url,
                    next_check: Utc::now()
                        + chrono::Duration::from_std(wait).unwrap_or(chrono::Duration::hours(6)),
                })),
                Err(AcmeError::Unsupported(_)) => Ok(None),
                Err(error) => Err(acme_error(error)),
            }
        })
        .await
    }
}

fn base64_url(value: &str) -> Option<Vec<u8>> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD
        .decode(value.trim().trim_end_matches('='))
        .ok()
        .filter(|key| !key.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn renewal_times_fall_inside_the_window() {
        let window = RenewalWindow {
            start: at("2026-03-01T00:00:00Z"),
            end: at("2026-03-03T00:00:00Z"),
            explanation_url: None,
            next_check: at("2026-03-01T06:00:00Z"),
        };
        for _ in 0..100 {
            let picked = window.pick(at("2026-02-01T00:00:00Z"));
            assert!(picked >= window.start && picked <= window.end, "{picked}");
        }
        let picked = window.pick(at("2026-03-02T00:00:00Z"));
        assert!(picked >= at("2026-03-02T00:00:00Z"));
        assert_eq!(
            window.pick(at("2026-04-01T00:00:00Z")),
            at("2026-04-01T00:00:00Z")
        );
    }

    #[test]
    fn problems_keep_the_cas_words() {
        let problem: Problem = serde_json::from_value(serde_json::json!({
            "type": "urn:ietf:params:acme:error:rateLimited",
            "detail": "too many certificates already issued",
            "status": 429,
        }))
        .unwrap();
        let error = problem_error(&problem);
        assert_eq!(error.code.as_str(), "RESOURCE_EXHAUSTED");
        assert!(error.retryable);
        assert_eq!(
            error.message,
            "the CA refused (rateLimited): too many certificates already issued"
        );
        assert!(certificate_identifier("a.b.c").is_err());
        assert!(base64_url("not base64!").is_none());
    }
}
