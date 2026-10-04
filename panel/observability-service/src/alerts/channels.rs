//! The channels alert rules notify. Where a channel sends can authorize
//! whoever holds it, so it is sealed with the channel's signing secret and
//! shown only as its origin.

use super::{expected, model::identifier, publish, refused, Cause};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use panel_event_contracts::observability::v1 as event;
use panel_postgres::{storage_error, EventLog, ServiceDatabase};
use panel_secrets::{Sealed, SecretVault};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgRow, PgPool, Row};
use std::sync::Arc;
use url::Url;
use zeroize::Zeroizing;

const AGGREGATE: &str = "alert_channel";
const MAX_URL: usize = 2048;
const SECRET_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ChannelKind {
    Webhook,
    /// Reserved: refused until the panel can send mail.
    Email,
}

impl ChannelKind {
    fn name(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::Email => "email",
        }
    }
}

/// A channel as shown: where it sends only as its origin.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelRecord {
    pub id: String,
    pub kind: ChannelKind,
    pub target: String,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What sending needs, as sealed.
#[derive(Deserialize, Serialize)]
struct Endpoint {
    url: String,
    secret: String,
}

/// Where a channel posts and the secret it signs with.
pub(crate) struct Destination {
    pub url: Url,
    pub secret: Zeroizing<String>,
}

fn webhook_url(raw: &str) -> Result<Url> {
    let invalid = |reason: &str| PanelError::invalid_argument(format!("a webhook URL {reason}"));
    if raw.len() > MAX_URL {
        return Err(invalid(&format!("is at most {MAX_URL} bytes")));
    }
    let url = Url::parse(raw).map_err(|error| invalid(&format!("is not a URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid("is http or https"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(invalid("names a host"));
    }
    Ok(url)
}

fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

/// A new Standard Webhooks secret: `whsec_` and 32 random bytes in base64.
fn new_secret() -> Result<Zeroizing<String>> {
    let mut bytes = Zeroizing::new([0_u8; SECRET_BYTES]);
    getrandom::fill(bytes.as_mut())
        .map_err(|error| PanelError::internal(format!("no randomness: {error}")))?;
    Ok(Zeroizing::new(format!("whsec_{}", STANDARD.encode(*bytes))))
}

fn owner(id: &str) -> String {
    format!("alert-channel/{id}/endpoint")
}

fn corrupt(what: &str) -> PanelError {
    PanelError::corrupt_state(format!("a stored alert channel has an invalid {what}"))
}

fn record(row: &PgRow) -> Result<ChannelRecord> {
    let kind: String = row.try_get("kind").map_err(storage_error)?;
    let version: i64 = row.try_get("version").map_err(storage_error)?;
    Ok(ChannelRecord {
        id: row.try_get("channel_id").map_err(storage_error)?,
        kind: match kind.as_str() {
            "webhook" => ChannelKind::Webhook,
            _ => return Err(corrupt("kind")),
        },
        target: row.try_get("target").map_err(storage_error)?,
        version: u64::try_from(version).map_err(|_| corrupt("version"))?,
        created_at: row.try_get("created_at").map_err(storage_error)?,
        updated_at: row.try_get("updated_at").map_err(storage_error)?,
    })
}

macro_rules! columns {
    () => {
        "channel_id, kind, target, version, created_at, updated_at"
    };
}

/// Alert channels in the service schema. Changes write their
/// `observability.alert_channel.*` events in the same transaction.
#[derive(Clone)]
pub struct AlertChannels {
    pool: PgPool,
    events: EventLog,
    vault: Option<Arc<dyn SecretVault>>,
}

impl AlertChannels {
    pub fn new(
        database: &ServiceDatabase,
        events: EventLog,
        vault: Option<Arc<dyn SecretVault>>,
    ) -> Self {
        Self {
            pool: database.pool().clone(),
            events,
            vault,
        }
    }

    fn vault(&self) -> Result<&dyn SecretVault> {
        self.vault.as_deref().ok_or_else(|| {
            PanelError::unavailable(
                "alert channels cannot be kept until master keys are configured",
            )
        })
    }

    async fn seal(&self, id: &str, url: &Url, secret: &str) -> Result<Sealed> {
        let endpoint = Zeroizing::new(
            serde_json::to_vec(&Endpoint {
                url: url.to_string(),
                secret: secret.to_owned(),
            })
            .map_err(|_| PanelError::internal("a channel endpoint does not serialize"))?,
        );
        self.vault()?.seal(&owner(id), &endpoint).await
    }

    pub async fn list(&self) -> Result<Vec<ChannelRecord>> {
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM alert_channels ORDER BY channel_id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?
        .iter()
        .map(record)
        .collect()
    }

    /// Creates a channel and returns it with its signing secret.
    pub async fn create(
        &self,
        cause: Cause<'_>,
        id: &str,
        kind: ChannelKind,
        url: &str,
    ) -> Result<(ChannelRecord, Zeroizing<String>)> {
        let result = async {
            let id = identifier(id, "a channel ID")?;
            if kind == ChannelKind::Email {
                return Err(PanelError::unsupported_capability(
                    "email channels are not available; notify a webhook",
                ));
            }
            let url = webhook_url(url)?;
            let secret = new_secret()?;
            let sealed = self.seal(&id, &url, &secret).await?;
            let now = Utc::now();
            let target = origin(&url);
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            let inserted = sqlx::query(
                "INSERT INTO alert_channels (channel_id, kind, target, sealed, version, \
                 created_at, updated_at) VALUES ($1, 'webhook', $2, $3, 1, $4, $4) \
                 ON CONFLICT (channel_id) DO NOTHING",
            )
            .bind(&id)
            .bind(&target)
            .bind(sealed.as_str())
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?
            .rows_affected();
            if inserted == 0 {
                return Err(PanelError::conflict(format!(
                    "alert channel {id} already exists"
                )));
            }
            let created = ChannelRecord {
                id: id.clone(),
                kind,
                target,
                version: 1,
                created_at: now,
                updated_at: now,
            };
            publish(
                &self.events,
                &mut transaction,
                cause,
                (AGGREGATE, &id),
                &event::AlertChannelCreated {
                    id: id.clone(),
                    kind: kind.name().to_owned(),
                    target: created.target.clone(),
                    version: 1,
                },
            )
            .await?;
            transaction.commit().await.map_err(storage_error)?;
            Ok((created, secret))
        }
        .await;
        refused::<event::AlertChannelRefused, _>(
            &self.events,
            cause,
            (AGGREGATE, id),
            "create",
            result,
        )
        .await
    }

    /// Replaces a channel's signing secret, and its URL when `url` is given.
    pub async fn rotate(
        &self,
        cause: Cause<'_>,
        id: &str,
        url: Option<&str>,
        version: u64,
    ) -> Result<(ChannelRecord, Zeroizing<String>)> {
        let result = async {
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            let row = sqlx::query(concat!(
                "SELECT ",
                columns!(),
                ", sealed FROM alert_channels WHERE channel_id = $1 FOR UPDATE"
            ))
            .bind(id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| PanelError::not_found(format!("there is no alert channel {id}")))?;
            let current = record(&row)?;
            if let Some(version) = expected(version).filter(|version| *version != current.version) {
                return Err(PanelError::precondition_failed(format!(
                    "alert channel {id} has changed; it is at version {}, not {version}",
                    current.version
                )));
            }
            let url = match url {
                Some(url) => webhook_url(url)?,
                None => {
                    let sealed =
                        Sealed::new(row.try_get::<String, _>("sealed").map_err(storage_error)?);
                    self.open(id, &sealed).await?.url
                }
            };
            let secret = new_secret()?;
            let sealed = self.seal(id, &url, &secret).await?;
            let now = Utc::now();
            let rotated = ChannelRecord {
                target: origin(&url),
                version: current.version + 1,
                updated_at: now,
                ..current
            };
            sqlx::query(
                "UPDATE alert_channels SET target = $2, sealed = $3, version = $4, \
                 updated_at = $5 WHERE channel_id = $1",
            )
            .bind(id)
            .bind(&rotated.target)
            .bind(sealed.as_str())
            .bind(i64::try_from(rotated.version).unwrap_or(i64::MAX))
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            publish(
                &self.events,
                &mut transaction,
                cause,
                (AGGREGATE, id),
                &event::AlertChannelRotated {
                    id: id.to_owned(),
                    target: rotated.target.clone(),
                    version: rotated.version,
                },
            )
            .await?;
            transaction.commit().await.map_err(storage_error)?;
            Ok((rotated, secret))
        }
        .await;
        refused::<event::AlertChannelRefused, _>(
            &self.events,
            cause,
            (AGGREGATE, id),
            "rotate",
            result,
        )
        .await
    }

    /// Deletes a channel no rule names, with its notifications.
    pub async fn delete(&self, cause: Cause<'_>, id: &str, version: u64) -> Result<()> {
        let result = async {
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT version FROM alert_channels WHERE channel_id = $1 FOR UPDATE",
            )
            .bind(id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage_error)?;
            let current = current
                .and_then(|version| u64::try_from(version).ok())
                .ok_or_else(|| PanelError::not_found(format!("there is no alert channel {id}")))?;
            if let Some(version) = expected(version).filter(|version| *version != current) {
                return Err(PanelError::precondition_failed(format!(
                    "alert channel {id} has changed; it is at version {current}, not {version}"
                )));
            }
            let rules: Vec<String> = sqlx::query_scalar(
                "SELECT rule_id FROM alert_rule_channels WHERE channel_id = $1 ORDER BY rule_id",
            )
            .bind(id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(storage_error)?;
            if !rules.is_empty() {
                return Err(PanelError::conflict(format!(
                    "alert channel {id} is notified by {}",
                    rules.join(", ")
                )));
            }
            sqlx::query("DELETE FROM alert_channels WHERE channel_id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
            publish(
                &self.events,
                &mut transaction,
                cause,
                (AGGREGATE, id),
                &event::AlertChannelDeleted { id: id.to_owned() },
            )
            .await?;
            transaction.commit().await.map_err(storage_error)
        }
        .await;
        refused::<event::AlertChannelRefused, _>(
            &self.events,
            cause,
            (AGGREGATE, id),
            "delete",
            result,
        )
        .await
    }

    async fn open(&self, id: &str, sealed: &Sealed) -> Result<Destination> {
        let opened = Zeroizing::new(self.vault()?.open(&owner(id), sealed).await?.to_vec());
        let endpoint: Endpoint =
            serde_json::from_slice(&opened).map_err(|_| corrupt("endpoint"))?;
        let endpoint = Zeroizing::new(endpoint);
        Ok(Destination {
            url: Url::parse(&endpoint.url).map_err(|_| corrupt("URL"))?,
            secret: Zeroizing::new(endpoint.secret.clone()),
        })
    }

    /// Where channel `id` posts and the secret it signs with.
    pub(crate) async fn destination(&self, id: &str) -> Result<Destination> {
        let sealed: Option<String> =
            sqlx::query_scalar("SELECT sealed FROM alert_channels WHERE channel_id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?;
        let sealed = sealed
            .map(Sealed::new)
            .ok_or_else(|| PanelError::not_found(format!("there is no alert channel {id}")))?;
        self.open(id, &sealed).await
    }
}

impl zeroize::Zeroize for Endpoint {
    fn zeroize(&mut self) {
        self.url.zeroize();
        self.secret.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhooks_are_http_urls_shown_by_their_origin() {
        let url = webhook_url("https://hooks.example:8443/services/T0/B0/secret?x=1").unwrap();
        assert_eq!(origin(&url), "https://hooks.example:8443");
        let credentials = webhook_url("https://user:secret@hooks.example/notify").unwrap();
        assert_eq!(origin(&credentials), "https://hooks.example");
        assert!(webhook_url("ftp://hooks.example/").is_err());
        assert!(webhook_url("https://").is_err());
        assert!(webhook_url("not a url").is_err());
        assert!(webhook_url(&format!("https://h.example/{}", "a".repeat(MAX_URL))).is_err());
    }

    #[test]
    fn secrets_are_standard_webhooks_keys() {
        let secret = new_secret().unwrap();
        let key = secret.strip_prefix("whsec_").unwrap();
        assert_eq!(STANDARD.decode(key).unwrap().len(), SECRET_BYTES);
        assert_ne!(*secret, *new_secret().unwrap());
    }
}
