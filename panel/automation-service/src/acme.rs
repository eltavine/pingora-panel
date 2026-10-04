//! Certificates obtained and renewed through ACME: accounts with CAs,
//! automatic certificates, and the jobs that issue and renew them.

use crate::{
    certificates::{Cause, CertificateInventory},
    dns::DnsProviders,
    events::refused,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use panel_acme::{
    AcmeClient, ChallengeKind, ChallengeSolver, Directory, ExternalAccount, Http01, OrderRequest,
    Registration,
};
use panel_certificates::{
    accept, renewal_identifier, renewal_time, requested_names, CertificateId, CertificateSource,
    ACME_CHALLENGE_DIRECTORY,
};
use panel_errors::{PanelError, Result};
use panel_event_contracts::tls::v1 as event;
use panel_events::{Actor, EventData, IdempotencyKey, Principal, RequestScope};
use panel_jobs::{
    Job, JobContext, JobHandler, JobKind, JobOrigin, JobSpec, JobStore, JobTemplate, Recurrence,
    Schedule, ScheduleName,
};
use panel_secrets::{Sealed, SecretVault};
use panel_sqlite::{storage_error, EventLog, ServiceDatabase, SqliteOutbox};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{sqlite::SqliteRow, types::Json, Row, SqliteConnection};
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};
use zeroize::Zeroizing;

/// Issues one automatic certificate, which the payload names.
pub const ISSUE_JOB: &str = "certificate.issue";
/// Enqueues the issuance of automatic certificates that are due and
/// announces expiring certificates.
pub const RENEWAL_CHECK_JOB: &str = "certificate.renewal-check";
const ACCOUNT: &str = "acme_account";
const AUTOMATIC: &str = "acme_certificate";
/// How long one issuance may run before another job may take it over.
const ISSUING_LEASE: Duration = Duration::minutes(15);
/// The longest wait after consecutive failures.
const MAX_BACKOFF: Duration = Duration::hours(24);
/// Bounds on how soon the CA's renewal window is asked for again.
const MIN_WINDOW_CHECK: Duration = Duration::hours(1);
const MAX_WINDOW_CHECK: Duration = Duration::hours(24);

/// The service itself, as the cause of issuance, renewal and reminders.
static SYSTEM: LazyLock<Principal> = LazyLock::new(|| {
    Principal::system(Actor::new("automation-service").expect("the service name is an actor"))
});

/// Checks an identifier of 1 to 64 lowercase letters, digits and hyphens.
pub(crate) fn slug(value: &str, what: &str) -> Result<String> {
    let valid = (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-');
    if valid {
        Ok(value.to_owned())
    } else {
        Err(PanelError::invalid_argument(format!(
            "{value:?} is not {what}: use 1 to 64 lowercase letters, digits and hyphens"
        )))
    }
}

/// Names an ACME account: 1 to 64 lowercase letters, digits and hyphens.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AccountId(String);

impl AccountId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        slug(&value.into(), "an account ID").map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AccountId {
    type Error = PanelError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<AccountId> for String {
    fn from(value: AccountId) -> Self {
        value.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An account with an ACME CA. Its key never leaves the service.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AcmeAccount {
    pub id: AccountId,
    /// The CA's directory URL.
    pub directory: String,
    /// PEM roots trusted for a private directory.
    pub ca_bundle: Option<String>,
    /// Email addresses the CA may write to.
    pub contact: Vec<String>,
    pub external_account_key_id: Option<String>,
    /// The account's URL at the CA.
    pub url: String,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AcmeAccount {
    pub fn etag(&self) -> String {
        format!("\"{}\"", self.version)
    }
}

/// The MAC key is used once to register and not kept.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAccountBody {
    pub key_id: String,
    pub mac_key: Zeroizing<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewAccount {
    pub id: AccountId,
    pub directory: String,
    #[serde(default)]
    pub ca_bundle: Option<String>,
    #[serde(default)]
    pub contact: Vec<String>,
    #[serde(default)]
    pub terms_of_service_agreed: bool,
    #[serde(default)]
    pub external_account: Option<ExternalAccountBody>,
}

/// Where an automatic certificate stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IssuanceState {
    /// Not issued yet.
    Pending,
    /// Issued; renewed when due.
    Issued,
    /// The last attempt failed; another follows after a pause.
    Failing,
}

/// The last failed attempt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LastError {
    pub code: String,
    pub message: String,
    pub at: DateTime<Utc>,
}

/// A certificate the service obtains from an ACME CA and renews, by the ID
/// of the inventory certificate it produces.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AutomaticCertificate {
    pub id: CertificateId,
    pub account: AccountId,
    pub names: Vec<String>,
    pub challenge: ChallengeKind,
    /// The provider that publishes DNS-01 records.
    pub dns_provider: Option<String>,
    pub state: IssuanceState,
    /// When it is issued next.
    pub renew_after: DateTime<Utc>,
    /// The CA's explanation of an unusually early renewal window.
    pub renewal_explanation_url: Option<String>,
    /// Consecutive failed attempts.
    pub failures: u32,
    pub last_error: Option<LastError>,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AutomaticCertificate {
    pub fn etag(&self) -> String {
        format!("\"{}\"", self.version)
    }
}

fn http01() -> ChallengeKind {
    ChallengeKind::Http01
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewAutomaticCertificate {
    pub id: CertificateId,
    pub account: AccountId,
    pub names: Vec<String>,
    #[serde(default = "http01")]
    pub challenge: ChallengeKind,
    /// Required for DNS-01.
    #[serde(default)]
    pub dns_provider: Option<String>,
}

/// The columns read into an [`AcmeAccount`], as a literal so queries stay
/// static.
macro_rules! account_columns {
    () => {
        "account_id, directory_url, ca_bundle, contact, external_account_key_id, account_url, \
         version, created_at, updated_at"
    };
}

/// The columns read into an [`AutomaticCertificate`], from
/// `acme_certificates a` joined with the inventory as `c`.
macro_rules! automatic_columns {
    () => {
        "a.certificate_id, a.account_id, a.names, a.challenge, a.dns_provider_id, a.renew_after, \
         a.window_explanation_url, a.failures, a.last_error_code, a.last_error_message, \
         a.last_attempt_at, a.version, a.created_at, a.updated_at, \
         (c.source = 'acme') AS issued"
    };
}

macro_rules! automatic_from {
    () => {
        " FROM acme_certificates a LEFT JOIN certificates c ON c.certificate_id = a.certificate_id"
    };
}

fn corrupt(what: &str) -> PanelError {
    PanelError::corrupt_state(format!("a stored ACME record has an invalid {what}"))
}

fn version(row: &SqliteRow) -> Result<u64> {
    let version: i64 = row.try_get("version").map_err(storage_error)?;
    u64::try_from(version).map_err(|_| corrupt("version"))
}

fn account(row: &SqliteRow) -> Result<AcmeAccount> {
    let Json(contact): Json<Vec<String>> = row.try_get("contact").map_err(storage_error)?;
    Ok(AcmeAccount {
        id: AccountId::new(
            row.try_get::<String, _>("account_id")
                .map_err(storage_error)?,
        )
        .map_err(|_| corrupt("account ID"))?,
        directory: row.try_get("directory_url").map_err(storage_error)?,
        ca_bundle: row.try_get("ca_bundle").map_err(storage_error)?,
        contact: contact
            .into_iter()
            .map(|uri| uri.trim_start_matches("mailto:").to_owned())
            .collect(),
        external_account_key_id: row
            .try_get("external_account_key_id")
            .map_err(storage_error)?,
        url: row.try_get("account_url").map_err(storage_error)?,
        version: version(row)?,
        created_at: row.try_get("created_at").map_err(storage_error)?,
        updated_at: row.try_get("updated_at").map_err(storage_error)?,
    })
}

fn challenge_kind(name: &str) -> Result<ChallengeKind> {
    serde_json::from_value(json!(name)).map_err(|_| corrupt("challenge"))
}

fn automatic(row: &SqliteRow) -> Result<AutomaticCertificate> {
    let failures: i32 = row.try_get("failures").map_err(storage_error)?;
    let issued: Option<bool> = row.try_get("issued").map_err(storage_error)?;
    let code: Option<String> = row.try_get("last_error_code").map_err(storage_error)?;
    let message: Option<String> = row.try_get("last_error_message").map_err(storage_error)?;
    let attempted: Option<DateTime<Utc>> = row.try_get("last_attempt_at").map_err(storage_error)?;
    let failures = u32::try_from(failures).map_err(|_| corrupt("failure count"))?;
    Ok(AutomaticCertificate {
        id: CertificateId::new(
            row.try_get::<String, _>("certificate_id")
                .map_err(storage_error)?,
        )
        .map_err(|_| corrupt("certificate ID"))?,
        account: AccountId::new(
            row.try_get::<String, _>("account_id")
                .map_err(storage_error)?,
        )
        .map_err(|_| corrupt("account ID"))?,
        names: row
            .try_get::<Json<Vec<String>>, _>("names")
            .map_err(storage_error)?
            .0,
        challenge: challenge_kind(
            &row.try_get::<String, _>("challenge")
                .map_err(storage_error)?,
        )?,
        dns_provider: row.try_get("dns_provider_id").map_err(storage_error)?,
        state: if failures > 0 {
            IssuanceState::Failing
        } else if issued == Some(true) {
            IssuanceState::Issued
        } else {
            IssuanceState::Pending
        },
        renew_after: row.try_get("renew_after").map_err(storage_error)?,
        renewal_explanation_url: row
            .try_get("window_explanation_url")
            .map_err(storage_error)?,
        failures,
        last_error: match (code, message, attempted) {
            (Some(code), Some(message), Some(at)) if failures > 0 => {
                Some(LastError { code, message, at })
            }
            _ => None,
        },
        version: version(row)?,
        created_at: row.try_get("created_at").map_err(storage_error)?,
        updated_at: row.try_get("updated_at").map_err(storage_error)?,
    })
}

/// The owner account credentials are sealed for.
fn owner(id: &AccountId) -> String {
    format!("acme-account/{id}/credentials")
}

/// The pause after `failures` consecutive failed attempts.
fn backoff(failures: u32) -> Duration {
    let hours = 1_i64 << failures.saturating_sub(1).min(5);
    Duration::hours(hours).min(MAX_BACKOFF)
}

fn check_directory(url: &str) -> Result<()> {
    let host = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|host| !host.is_empty());
    let valid = host.is_some() && url.len() <= 2048 && !url.contains(char::is_whitespace);
    if valid {
        Ok(())
    } else {
        Err(PanelError::invalid_argument(format!(
            "{url:?} is not an https:// URL of an ACME directory"
        )))
    }
}

/// Email addresses as the `mailto:` URIs RFC 8555 §7.3 registers.
fn contact_uris(contact: &[String]) -> Result<Vec<String>> {
    contact
        .iter()
        .map(|address| {
            let address = address.trim().trim_start_matches("mailto:");
            let valid = address.len() <= 254
                && address
                    .split_once('@')
                    .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
                && !address
                    .chars()
                    .any(|character| character.is_whitespace() || ",;<>\"".contains(character));
            if valid {
                Ok(format!("mailto:{address}"))
            } else {
                Err(PanelError::invalid_argument(format!(
                    "{address:?} is not an email address"
                )))
            }
        })
        .collect()
}

fn origin(scope: &RequestScope) -> JobOrigin {
    JobOrigin {
        correlation_id: scope.correlation_id().clone(),
        causation_id: scope.request_id().clone(),
    }
}

/// Fails unless `expected`, an entity tag, names the current version.
fn check_version(what: &str, current: u64, expected: Option<u64>) -> Result<()> {
    match expected {
        Some(expected) if expected != current => Err(PanelError::precondition_failed(format!(
            "{what} has changed; it is at version {current}"
        ))),
        _ => Ok(()),
    }
}

/// ACME accounts and automatic certificates in the module's database.
///
/// Changes write their `tls.acme.*` events in the same transaction;
/// issuances store their certificates in the inventory, which publishes
/// `tls.certificate.*` events.
#[derive(Clone)]
pub struct AcmeAutomation {
    database: ServiceDatabase,
    events: EventLog,
    vault: Option<Arc<dyn SecretVault>>,
    inventory: CertificateInventory,
    dns: DnsProviders,
    jobs: Arc<dyn JobStore>,
    client: AcmeClient,
    challenges: Option<PathBuf>,
}

impl AcmeAutomation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: &ServiceDatabase,
        events: EventLog,
        vault: Option<Arc<dyn SecretVault>>,
        inventory: CertificateInventory,
        dns: DnsProviders,
        jobs: Arc<dyn JobStore>,
        client: AcmeClient,
        secret_directory: Option<&Path>,
    ) -> Self {
        Self {
            database: database.clone(),
            events,
            vault,
            inventory,
            dns,
            jobs,
            client,
            challenges: secret_directory.map(|directory| directory.join(ACME_CHALLENGE_DIRECTORY)),
        }
    }

    fn vault(&self) -> Result<&dyn SecretVault> {
        self.vault.as_deref().ok_or_else(|| {
            PanelError::unavailable("ACME accounts cannot be kept until master keys are configured")
        })
    }

    pub async fn accounts(&self) -> Result<Vec<AcmeAccount>> {
        sqlx::query(concat!(
            "SELECT ",
            account_columns!(),
            " FROM acme_accounts ORDER BY account_id"
        ))
        .fetch_all(self.database.pool())
        .await
        .map_err(storage_error)?
        .iter()
        .map(account)
        .collect()
    }

    pub async fn account(&self, id: &AccountId) -> Result<AcmeAccount> {
        sqlx::query(concat!(
            "SELECT ",
            account_columns!(),
            " FROM acme_accounts WHERE account_id = ?1"
        ))
        .bind(id.as_str())
        .fetch_optional(self.database.pool())
        .await
        .map_err(storage_error)?
        .map(|row| account(&row))
        .transpose()?
        .ok_or_else(|| PanelError::not_found(format!("there is no ACME account {id}")))
    }

    /// Registers an account with the CA at the directory and keeps it.
    pub async fn create_account(&self, cause: Cause<'_>, body: NewAccount) -> Result<AcmeAccount> {
        let id = body.id.clone();
        let result = async {
            check_directory(&body.directory)?;
            let contact = contact_uris(&body.contact)?;
            let vault = self.vault()?;
            if self.account(&id).await.is_ok() {
                return Err(PanelError::conflict(format!(
                    "ACME account {id} already exists"
                )));
            }
            let directory = Directory {
                url: body.directory.clone(),
                ca_bundle: body
                    .ca_bundle
                    .clone()
                    .filter(|bundle| !bundle.trim().is_empty()),
            };
            let external_account_key_id = body
                .external_account
                .as_ref()
                .map(|binding| binding.key_id.trim().to_owned());
            let registered = self
                .client
                .register(
                    &directory,
                    &Registration {
                        contact: contact.clone(),
                        terms_of_service_agreed: body.terms_of_service_agreed,
                        external_account: body.external_account.map(|binding| ExternalAccount {
                            key_id: binding.key_id.trim().to_owned(),
                            mac_key: binding.mac_key,
                        }),
                    },
                )
                .await?;
            let sealed = vault
                .seal(&owner(&id), registered.credentials.as_bytes())
                .await?;
            let now = Utc::now();
            let mut transaction = self.database.begin().await?;
            let inserted = sqlx::query(
                "INSERT INTO acme_accounts (account_id, directory_url, ca_bundle, contact, \
                 external_account_key_id, account_url, sealed_credentials, version, created_at, \
                 updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8) \
                 ON CONFLICT (account_id) DO NOTHING",
            )
            .bind(id.as_str())
            .bind(&directory.url)
            .bind(&directory.ca_bundle)
            .bind(Json(&contact))
            .bind(&external_account_key_id)
            .bind(&registered.url)
            .bind(sealed.as_str())
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?
            .rows_affected();
            if inserted == 0 {
                return Err(PanelError::conflict(format!(
                    "ACME account {id} already exists"
                )));
            }
            let created = AcmeAccount {
                id: id.clone(),
                directory: directory.url,
                ca_bundle: directory.ca_bundle,
                contact: body
                    .contact
                    .iter()
                    .map(|address| address.trim().to_owned())
                    .collect(),
                external_account_key_id,
                url: registered.url,
                version: 1,
                created_at: now,
                updated_at: now,
            };
            self.publish(
                &mut transaction,
                cause,
                (ACCOUNT, id.as_str()),
                &event::AcmeAccountCreated {
                    id: created.id.as_str().to_owned(),
                    directory: created.directory.clone(),
                    url: created.url.clone(),
                    external_account_key_id: created.external_account_key_id.clone(),
                },
            )
            .await?;
            transaction.commit().await.map_err(storage_error)?;
            Ok(created)
        }
        .await;
        refused::<event::AcmeAccountRefused, _>(
            &self.events,
            cause,
            (ACCOUNT, id.as_str()),
            "create",
            result,
        )
        .await
    }

    /// Forgets an account that no automatic certificate uses any more.
    pub async fn delete_account(
        &self,
        cause: Cause<'_>,
        id: AccountId,
        expected: Option<u64>,
    ) -> Result<()> {
        let result = async {
            let mut transaction = self.database.begin().await?;
            let current = sqlx::query(concat!(
                "SELECT ",
                account_columns!(),
                " FROM acme_accounts WHERE account_id = ?1"
            ))
            .bind(id.as_str())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage_error)?
            .map(|row| account(&row))
            .transpose()?
            .ok_or_else(|| PanelError::not_found(format!("there is no ACME account {id}")))?;
            check_version(&format!("ACME account {id}"), current.version, expected)?;
            let users: Vec<String> = sqlx::query_scalar(
                "SELECT certificate_id FROM acme_certificates WHERE account_id = ?1 \
                 ORDER BY certificate_id",
            )
            .bind(id.as_str())
            .fetch_all(&mut *transaction)
            .await
            .map_err(storage_error)?;
            if !users.is_empty() {
                return Err(PanelError::conflict(format!(
                    "ACME account {id} still renews {}",
                    users.join(", ")
                )));
            }
            sqlx::query("DELETE FROM acme_accounts WHERE account_id = ?1")
                .bind(id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
            self.publish(
                &mut transaction,
                cause,
                (ACCOUNT, id.as_str()),
                &event::AcmeAccountDeleted {
                    id: id.as_str().to_owned(),
                    directory: current.directory,
                },
            )
            .await?;
            transaction.commit().await.map_err(storage_error)
        }
        .await;
        refused::<event::AcmeAccountRefused, _>(
            &self.events,
            cause,
            (ACCOUNT, id.as_str()),
            "delete",
            result,
        )
        .await
    }

    pub async fn certificates(&self) -> Result<Vec<AutomaticCertificate>> {
        sqlx::query(concat!(
            "SELECT ",
            automatic_columns!(),
            automatic_from!(),
            " ORDER BY a.certificate_id"
        ))
        .fetch_all(self.database.pool())
        .await
        .map_err(storage_error)?
        .iter()
        .map(automatic)
        .collect()
    }

    pub async fn certificate(&self, id: &CertificateId) -> Result<AutomaticCertificate> {
        sqlx::query(concat!(
            "SELECT ",
            automatic_columns!(),
            automatic_from!(),
            " WHERE a.certificate_id = ?1"
        ))
        .bind(id.as_str())
        .fetch_optional(self.database.pool())
        .await
        .map_err(storage_error)?
        .map(|row| automatic(&row))
        .transpose()?
        .ok_or_else(|| PanelError::not_found(format!("{id} is not an automatic certificate")))
    }

    /// Starts obtaining and renewing a certificate; the first issuance is
    /// enqueued at once.
    pub async fn create_certificate(
        &self,
        cause: Cause<'_>,
        body: NewAutomaticCertificate,
    ) -> Result<AutomaticCertificate> {
        let id = body.id.clone();
        let result = async {
            let names = requested_names(&body.names)?;
            if body.challenge == ChallengeKind::Http01
                && names.iter().any(|name| name.starts_with("*."))
            {
                return Err(PanelError::validation_failed(
                    "wildcard names can only be validated with DNS-01",
                ));
            }
            match (body.challenge, body.dns_provider.as_deref()) {
                (ChallengeKind::Dns01, Some(provider)) => {
                    self.dns.get(provider).await?;
                }
                (ChallengeKind::Dns01, None) => {
                    return Err(PanelError::validation_failed(
                        "DNS-01 needs a DNS provider to publish its records",
                    ))
                }
                (_, Some(_)) => {
                    return Err(PanelError::validation_failed(
                        "only DNS-01 certificates name a DNS provider",
                    ))
                }
                _ => {}
            }
            self.account(&body.account).await?;
            let now = Utc::now();
            let mut transaction = self.database.begin().await?;
            let inserted = sqlx::query(
                "INSERT INTO acme_certificates (certificate_id, account_id, names, challenge, \
                 dns_provider_id, renew_after, version, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?6, ?6) ON CONFLICT (certificate_id) DO NOTHING",
            )
            .bind(id.as_str())
            .bind(body.account.as_str())
            .bind(Json(&names))
            .bind(body.challenge.as_str())
            .bind(&body.dns_provider)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?
            .rows_affected();
            if inserted == 0 {
                return Err(PanelError::conflict(format!(
                    "{id} is already an automatic certificate"
                )));
            }
            self.publish(
                &mut transaction,
                cause,
                (AUTOMATIC, id.as_str()),
                &event::AcmeCertificateCreated {
                    id: id.to_string(),
                    account: body.account.as_str().to_owned(),
                    names: names.clone(),
                    challenge: body.challenge.as_str().to_owned(),
                    dns_provider: body.dns_provider.clone(),
                },
            )
            .await?;
            transaction.commit().await.map_err(storage_error)?;
            self.enqueue(&id, now, origin(cause.scope)).await?;
            self.certificate(&id).await
        }
        .await;
        refused::<event::AcmeCertificateRefused, _>(
            &self.events,
            cause,
            (AUTOMATIC, id.as_str()),
            "create",
            result,
        )
        .await
    }

    /// Issues an automatic certificate again now, for example after its
    /// names' DNS was fixed.
    pub async fn renew(&self, cause: Cause<'_>, id: CertificateId) -> Result<AutomaticCertificate> {
        let result = async {
            let now = Utc::now();
            let mut transaction = self.database.begin().await?;
            let updated = sqlx::query(
                "UPDATE acme_certificates SET renew_after = ?2 WHERE certificate_id = ?1",
            )
            .bind(id.as_str())
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?
            .rows_affected();
            if updated == 0 {
                return Err(PanelError::not_found(format!(
                    "{id} is not an automatic certificate"
                )));
            }
            self.publish(
                &mut transaction,
                cause,
                (AUTOMATIC, id.as_str()),
                &event::AcmeCertificateRenewalRequested { id: id.to_string() },
            )
            .await?;
            transaction.commit().await.map_err(storage_error)?;
            self.enqueue(&id, now, origin(cause.scope)).await?;
            self.certificate(&id).await
        }
        .await;
        refused::<event::AcmeCertificateRefused, _>(
            &self.events,
            cause,
            (AUTOMATIC, id.as_str()),
            "renew",
            result,
        )
        .await
    }

    /// Stops renewing a certificate; the certificate stays in the inventory.
    pub async fn delete_certificate(
        &self,
        cause: Cause<'_>,
        id: CertificateId,
        expected: Option<u64>,
    ) -> Result<()> {
        let result = async {
            let mut transaction = self.database.begin().await?;
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT version FROM acme_certificates WHERE certificate_id = ?1",
            )
            .bind(id.as_str())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage_error)?;
            let current = current
                .and_then(|version| u64::try_from(version).ok())
                .ok_or_else(|| {
                    PanelError::not_found(format!("{id} is not an automatic certificate"))
                })?;
            check_version(&format!("automatic certificate {id}"), current, expected)?;
            sqlx::query("DELETE FROM acme_certificates WHERE certificate_id = ?1")
                .bind(id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
            self.publish(
                &mut transaction,
                cause,
                (AUTOMATIC, id.as_str()),
                &event::AcmeCertificateDeleted { id: id.to_string() },
            )
            .await?;
            transaction.commit().await.map_err(storage_error)
        }
        .await;
        refused::<event::AcmeCertificateRefused, _>(
            &self.events,
            cause,
            (AUTOMATIC, id.as_str()),
            "delete",
            result,
        )
        .await
    }

    /// Enqueues the issuance due at `due`; enqueuing it again is a no-op.
    async fn enqueue(
        &self,
        id: &CertificateId,
        due: DateTime<Utc>,
        origin: JobOrigin,
    ) -> Result<()> {
        let mut spec = JobSpec::json(
            JobKind::new(ISSUE_JOB)?,
            IdempotencyKey::new(format!("acme-issue:{id}:{}", due.timestamp_millis()))?,
            &json!({ "certificate_id": id }),
            origin,
        )?;
        spec.max_attempts = 1;
        self.jobs.enqueue(&spec).await?;
        Ok(())
    }

    /// Issues `id` if it is due and no other job is issuing it: orders the
    /// certificate, stores it in the inventory and schedules the renewal.
    /// A failure is recorded with the certificate and published as
    /// `tls.acme.certificate.failed` before it is returned.
    pub async fn issue(&self, scope: &RequestScope, id: &CertificateId) -> Result<()> {
        let now = Utc::now();
        let Some(row) = sqlx::query(
            "UPDATE acme_certificates SET issuing_until = ?2 WHERE certificate_id = ?1 \
             AND renew_after <= ?3 AND (issuing_until IS NULL OR issuing_until < ?3) \
             RETURNING account_id, names, challenge, dns_provider_id, failures",
        )
        .bind(id.as_str())
        .bind(now + ISSUING_LEASE)
        .bind(now)
        .fetch_optional(self.database.pool())
        .await
        .map_err(storage_error)?
        else {
            return Ok(());
        };
        let account = AccountId::new(
            row.try_get::<String, _>("account_id")
                .map_err(storage_error)?,
        )
        .map_err(|_| corrupt("account ID"))?;
        let Json(names): Json<Vec<String>> = row.try_get("names").map_err(storage_error)?;
        let challenge = challenge_kind(
            &row.try_get::<String, _>("challenge")
                .map_err(storage_error)?,
        )?;
        let provider: Option<String> = row.try_get("dns_provider_id").map_err(storage_error)?;
        let failures: i32 = row.try_get("failures").map_err(storage_error)?;
        let cause = Cause {
            scope,
            principal: &SYSTEM,
        };
        match self
            .order(cause, id, &account, &names, challenge, provider.as_deref())
            .await
        {
            Ok(renew_after) => {
                sqlx::query(
                    "UPDATE acme_certificates SET renew_after = ?2, window_checked_after = ?3, \
                     window_explanation_url = NULL, issuing_until = NULL, failures = 0, \
                     last_error_code = NULL, last_error_message = NULL, last_attempt_at = ?3 \
                     WHERE certificate_id = ?1",
                )
                .bind(id.as_str())
                .bind(renew_after)
                .bind(Utc::now())
                .execute(self.database.pool())
                .await
                .map_err(storage_error)?;
                Ok(())
            }
            Err(error) => {
                let failures = u32::try_from(failures).unwrap_or(0).saturating_add(1);
                let attempted = Utc::now();
                let next = attempted + backoff(failures);
                let mut transaction = self.database.begin().await?;
                sqlx::query(
                    "UPDATE acme_certificates SET renew_after = ?2, issuing_until = NULL, \
                     failures = ?3, last_error_code = ?4, last_error_message = ?5, \
                     last_attempt_at = ?6 WHERE certificate_id = ?1",
                )
                .bind(id.as_str())
                .bind(next)
                .bind(i32::try_from(failures).unwrap_or(i32::MAX))
                .bind(error.code.as_str())
                .bind(&error.message)
                .bind(attempted)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
                self.publish(
                    &mut transaction,
                    cause,
                    (AUTOMATIC, id.as_str()),
                    &event::AcmeCertificateFailed {
                        id: id.to_string(),
                        names,
                        failures,
                        code: error.code.as_str().to_owned(),
                        message: error.message.clone(),
                        next_attempt_at: Some(next.into()),
                    },
                )
                .await?;
                transaction.commit().await.map_err(storage_error)?;
                Err(error)
            }
        }
    }

    /// Orders and stores the certificate; returns when it is due next.
    async fn order(
        &self,
        cause: Cause<'_>,
        id: &CertificateId,
        account: &AccountId,
        names: &[String],
        challenge: ChallengeKind,
        provider: Option<&str>,
    ) -> Result<DateTime<Utc>> {
        let (directory, credentials) = self.credentials(account).await?;
        let solver: Box<dyn ChallengeSolver> = match challenge {
            ChallengeKind::Http01 => {
                Box::new(Http01::new(self.challenges.clone().ok_or_else(|| {
                    PanelError::unavailable(
                        "HTTP-01 needs the gateway's secret directory, and none is configured",
                    )
                })?))
            }
            _ => Box::new(
                self.dns
                    .solver(provider.ok_or_else(|| {
                        PanelError::validation_failed(
                            "DNS-01 needs a DNS provider to publish its records",
                        )
                    })?)
                    .await?,
            ),
        };
        let replaces = match self.inventory.chain(id).await? {
            Some((CertificateSource::Acme, chain)) => renewal_identifier(&chain).ok().flatten(),
            _ => None,
        };
        let issued = self
            .client
            .issue(
                &directory,
                &credentials,
                OrderRequest {
                    names,
                    challenge,
                    replaces: replaces.as_deref(),
                },
                solver.as_ref(),
            )
            .await?;
        let accepted = accept(&issued.chain, &issued.key, Utc::now())?;
        let renew_after = renewal_time(&accepted.details);
        self.inventory.store_issued(cause, id, accepted).await?;
        Ok(renew_after)
    }

    async fn credentials(&self, id: &AccountId) -> Result<(Directory, Zeroizing<String>)> {
        let row = sqlx::query(
            "SELECT directory_url, ca_bundle, sealed_credentials FROM acme_accounts \
             WHERE account_id = ?1",
        )
        .bind(id.as_str())
        .fetch_optional(self.database.pool())
        .await
        .map_err(storage_error)?
        .ok_or_else(|| PanelError::not_found(format!("there is no ACME account {id}")))?;
        let sealed = Sealed::new(
            row.try_get::<String, _>("sealed_credentials")
                .map_err(storage_error)?,
        );
        let opened = self.vault()?.open(&owner(id), &sealed).await?;
        let credentials =
            String::from_utf8(opened.to_vec()).map_err(|_| corrupt("credential encoding"))?;
        Ok((
            Directory {
                url: row.try_get("directory_url").map_err(storage_error)?,
                ca_bundle: row.try_get("ca_bundle").map_err(storage_error)?,
            },
            Zeroizing::new(credentials),
        ))
    }

    /// Announces expiring certificates, follows the CAs' renewal windows and
    /// enqueues the issuance of automatic certificates that are due.
    pub async fn check_renewals(&self, scope: &RequestScope, now: DateTime<Utc>) -> Result<()> {
        let cause = Cause {
            scope,
            principal: &SYSTEM,
        };
        if let Err(error) = self.inventory.remind_expiring(cause, now).await {
            tracing::warn!(error_code = %error.code, "expiring certificates not announced");
        }
        let windows: Vec<(String, String, DateTime<Utc>, String)> = sqlx::query_as(
            "SELECT a.certificate_id, a.account_id, a.renew_after, c.chain \
             FROM acme_certificates a JOIN certificates c ON c.certificate_id = a.certificate_id \
             WHERE c.source = 'acme' AND a.failures = 0 \
             AND (a.window_checked_after IS NULL OR a.window_checked_after <= ?1)",
        )
        .bind(now)
        .fetch_all(self.database.pool())
        .await
        .map_err(storage_error)?;
        for (id, account, renew_after, chain) in windows {
            if let Err(error) = self
                .follow_window(&id, &account, renew_after, &chain, now)
                .await
            {
                tracing::warn!(error_code = %error.code, certificate = %id, "renewal window not checked");
            }
        }
        let due: Vec<(String, DateTime<Utc>)> = sqlx::query_as(
            "SELECT certificate_id, renew_after FROM acme_certificates WHERE renew_after <= ?1 \
             AND (issuing_until IS NULL OR issuing_until < ?1) ORDER BY renew_after",
        )
        .bind(now)
        .fetch_all(self.database.pool())
        .await
        .map_err(storage_error)?;
        for (id, renew_after) in due {
            let id = CertificateId::new(id).map_err(|_| corrupt("certificate ID"))?;
            self.enqueue(&id, renew_after, origin(scope)).await?;
        }
        Ok(())
    }

    /// Moves a certificate's renewal into the CA's current window.
    async fn follow_window(
        &self,
        id: &str,
        account: &str,
        renew_after: DateTime<Utc>,
        chain: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let account = AccountId::new(account).map_err(|_| corrupt("account ID"))?;
        let Some(identifier) = renewal_identifier(chain)? else {
            return self
                .window_checked(id, renew_after, None, now + MAX_WINDOW_CHECK)
                .await;
        };
        let (directory, credentials) = self.credentials(&account).await?;
        match self
            .client
            .renewal_window(&directory, &credentials, &identifier)
            .await
        {
            Ok(Some(window)) => {
                let next = if window.start <= renew_after && renew_after <= window.end {
                    renew_after
                } else {
                    window.pick(now)
                };
                let check = window
                    .next_check
                    .clamp(now + MIN_WINDOW_CHECK, now + MAX_WINDOW_CHECK);
                self.window_checked(id, next, window.explanation_url, check)
                    .await
            }
            Ok(None) => {
                self.window_checked(id, renew_after, None, now + MAX_WINDOW_CHECK)
                    .await
            }
            Err(error) => {
                self.window_checked(id, renew_after, None, now + MIN_WINDOW_CHECK)
                    .await?;
                Err(error)
            }
        }
    }

    async fn window_checked(
        &self,
        id: &str,
        renew_after: DateTime<Utc>,
        explanation: Option<String>,
        next_check: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE acme_certificates SET renew_after = ?2, window_explanation_url = ?3, \
             window_checked_after = ?4 WHERE certificate_id = ?1",
        )
        .bind(id)
        .bind(renew_after)
        .bind(explanation)
        .bind(next_check)
        .execute(self.database.pool())
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn publish<E: EventData>(
        &self,
        connection: &mut SqliteConnection,
        cause: Cause<'_>,
        aggregate: (&str, &str),
        data: &E,
    ) -> Result<()> {
        let event = self
            .events
            .event_by(aggregate, cause.scope, cause.principal, data)?;
        SqliteOutbox::append(connection, &event).await
    }
}

/// The scope of a job's work: its request, within its correlation.
fn job_scope(job: &Job) -> RequestScope {
    RequestScope::new(job.origin.causation_id.clone())
        .with_correlation_id(job.origin.correlation_id.clone())
}

/// The hourly schedule of [`RENEWAL_CHECK_JOB`].
pub fn renewal_schedule() -> Result<Schedule> {
    Ok(Schedule {
        name: ScheduleName::new("certificate-renewals")?,
        recurrence: Recurrence::parse("DTSTART:20260101T000000Z\nRRULE:FREQ=HOURLY")?,
        template: JobTemplate {
            kind: JobKind::new(RENEWAL_CHECK_JOB)?,
            media_type: "application/json".into(),
            payload: b"{}".to_vec(),
            max_attempts: 1,
            priority: 0,
            maintenance_window: None,
        },
        enabled: true,
    })
}

/// Runs [`ISSUE_JOB`].
pub struct IssueHandler(pub AcmeAutomation);

#[async_trait]
impl JobHandler for IssueHandler {
    async fn run(&self, context: JobContext) -> Result<()> {
        #[derive(Deserialize)]
        struct Payload {
            certificate_id: CertificateId,
        }
        let payload: Payload = context.job().json()?;
        let scope = job_scope(context.job());
        tokio::select! {
            result = self.0.issue(&scope, &payload.certificate_id) => result,
            () = context.cancellation().cancelled() => {
                Err(PanelError::unavailable("the issuance stopped before it finished"))
            }
        }
    }
}

/// Runs [`RENEWAL_CHECK_JOB`].
pub struct RenewalCheckHandler(pub AcmeAutomation);

#[async_trait]
impl JobHandler for RenewalCheckHandler {
    async fn run(&self, context: JobContext) -> Result<()> {
        self.0
            .check_renewals(&job_scope(context.job()), Utc::now())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_back_off_from_an_hour_to_a_day() {
        let hours: Vec<i64> = (1..=8)
            .map(|failures| backoff(failures).num_hours())
            .collect();
        assert_eq!(hours, [1, 2, 4, 8, 16, 24, 24, 24]);
    }

    #[test]
    fn requests_are_checked_before_any_ca_is_asked() {
        assert!(AccountId::new("letsencrypt-1").is_ok());
        for invalid in ["", "Upper", "-edge", "edge-", "a b", &"a".repeat(65)] {
            assert!(AccountId::new(invalid).is_err(), "{invalid}");
        }
        assert!(check_directory(panel_acme::LETS_ENCRYPT).is_ok());
        assert!(check_directory("http://acme.example/directory").is_err());
        assert!(check_directory("not a url").is_err());
        assert_eq!(
            contact_uris(&[
                " ops@example.com".to_owned(),
                "mailto:dev@example.com".to_owned()
            ])
            .unwrap(),
            ["mailto:ops@example.com", "mailto:dev@example.com"]
        );
        for invalid in ["ops", "@example.com", "ops@localhost", "a,b@example.com"] {
            assert!(contact_uris(&[invalid.to_owned()]).is_err(), "{invalid}");
        }
    }
}
