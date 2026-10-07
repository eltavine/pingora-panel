//! Delivers queued notifications to webhook channels, signed as Standard
//! Webhooks specify, or through the plugins of plugin channels, retrying
//! failures for a day (ADR 0027, ADR 0044).

use super::{
    channels::{AlertChannels, Destination, PluginTarget},
    payload::Notices,
};
use chrono::{DateTime, TimeDelta, Utc};
use panel_errors::{PanelError, Result};
use panel_sqlite::{storage_error, ServiceDatabase};
use plugin_contracts::{
    v1::{notification_provider_client::NotificationProviderClient, Alert, NotifyRequest},
    Plugin,
};
use reqwest::{header, redirect::Policy, StatusCode};
use serde_json::Value;
use sqlx::Row;
use standardwebhooks::Webhook;
use std::{future::Future, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use tonic::{transport::Channel, Code};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

/// How long a receiver may take to answer.
const TIMEOUT: Duration = Duration::from_secs(10);
/// How often due notifications are looked for.
const POLL: Duration = Duration::from_secs(5);
/// Delays before each retry; the last repeats.
const BACKOFF: [i64; 7] = [10, 30, 60, 300, 900, 1800, 3600];
/// How long a notification is retried.
const GIVE_UP_AFTER: TimeDelta = TimeDelta::days(1);
/// How long delivered and abandoned notifications are kept.
const KEEP: TimeDelta = TimeDelta::days(7);
const PURGE_EVERY: Duration = Duration::from_secs(3600);
const MAX_LIST: u32 = 200;
const DEFAULT_LIST: u32 = 50;

/// Alertmanager's `endsAt` for an alert that has not ended.
const UNENDED: &str = "0001-01-01T00:00:00Z";

/// How one attempt ended.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Attempt {
    /// With the receiver's HTTP status; plugins answer none.
    Delivered(Option<u16>),
    /// Worth trying again, with the receiver's status if it answered.
    Retry(Option<u16>, String),
    Abandon(Option<u16>, String),
}

/// How a test notification fared.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestOutcome {
    pub delivered: bool,
    /// The receiver's HTTP status, if it answered.
    pub status: Option<u16>,
    /// Why it was not delivered; empty when it was.
    pub failure: String,
}

/// A notification as kept.
#[derive(Clone, Debug, PartialEq)]
pub struct NotificationRecord {
    pub id: Uuid,
    pub rule: String,
    pub channel: String,
    /// `firing` or `resolved`.
    pub kind: String,
    /// `queued`, `delivered` or `abandoned`.
    pub state: String,
    pub attempts: u32,
    pub created_at: DateTime<Utc>,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub last_failure: String,
}

/// The notification a plugin delivers, from the Alertmanager payload that
/// holds its one alert.
fn notify_request(channel: &str, payload: &str) -> Option<NotifyRequest> {
    let payload: Value = serde_json::from_str(payload).ok()?;
    let alert = payload["alerts"].get(0)?;
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let strings = |value: &Value| {
        value
            .as_object()
            .map(|fields| {
                fields
                    .iter()
                    .map(|(name, value)| {
                        let value = value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_owned);
                        (name.clone(), value)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let ends_at = text(&alert["endsAt"]);
    Some(NotifyRequest {
        channel: channel.to_owned(),
        alert: Some(Alert {
            status: text(&alert["status"]),
            labels: strings(&alert["labels"]),
            annotations: strings(&alert["annotations"]),
            starts_at: text(&alert["startsAt"]),
            ends_at: if ends_at == UNENDED {
                String::new()
            } else {
                ends_at
            },
            generator_url: text(&alert["generatorURL"]),
            fingerprint: text(&alert["fingerprint"]),
        }),
        external_url: text(&payload["externalURL"]),
    })
}

/// Sends notifications.
#[derive(Clone)]
pub struct Notifier {
    database: ServiceDatabase,
    channels: AlertChannels,
    notices: Arc<Notices>,
    http: reqwest::Client,
    plugins: Option<Channel>,
}

impl Notifier {
    pub fn new(
        database: &ServiceDatabase,
        channels: AlertChannels,
        notices: Arc<Notices>,
    ) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(Policy::none())
            .user_agent(concat!("pingora-panel/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                PanelError::internal(format!("cannot build an HTTP client: {error}"))
            })?;
        Ok(Self {
            database: database.clone(),
            channels,
            notices,
            http,
            plugins: None,
        })
    }

    /// Delivers the notifications of plugin channels through the plugins
    /// module at `plugins`.
    pub fn with_plugins(mut self, plugins: Channel) -> Self {
        self.plugins = Some(plugins);
        self
    }

    /// Sends `body` to `destination` as notification `id`.
    async fn send(&self, destination: &Destination, id: &str, body: &str) -> Attempt {
        match destination {
            Destination::Webhook { url, secret } => self.post(url, secret, id, body).await,
            Destination::Plugin(target) => self.notify_plugin(target, body).await,
        }
    }

    async fn notify_plugin(&self, target: &PluginTarget, body: &str) -> Attempt {
        let Some(plugins) = self.plugins.clone() else {
            return Attempt::Retry(None, "plugins cannot be reached from here".into());
        };
        let Some(request) = notify_request(&target.channel, body) else {
            return Attempt::Abandon(None, "the notification cannot be read".into());
        };
        let plugin = match Plugin::named(&target.plugin) {
            Ok(plugin) => plugin,
            Err(status) => return Attempt::Abandon(None, status.message().to_owned()),
        };
        let mut request = tonic::Request::new(request);
        request.set_timeout(TIMEOUT);
        let mut client = NotificationProviderClient::with_interceptor(plugins, plugin);
        match tokio::time::timeout(TIMEOUT, client.notify(request)).await {
            Ok(Ok(_)) => Attempt::Delivered(None),
            Ok(Err(status)) => {
                let failure = format!("plugin {}: {}", target.plugin, status.message());
                match status.code() {
                    Code::InvalidArgument | Code::Unimplemented => Attempt::Abandon(None, failure),
                    _ => Attempt::Retry(None, failure),
                }
            }
            Err(_) => Attempt::Retry(
                None,
                format!("plugin {} did not answer in time", target.plugin),
            ),
        }
    }

    /// Posts `body` to `url`, signed with `secret` as notification `id`.
    async fn post(&self, url: &Url, secret: &Zeroizing<String>, id: &str, body: &str) -> Attempt {
        let timestamp = Utc::now().timestamp();
        let signature = match Webhook::new(secret)
            .and_then(|webhook| webhook.sign(id, timestamp, body.as_bytes()))
        {
            Ok(signature) => signature,
            Err(_) => return Attempt::Abandon(None, "the channel's secret cannot sign".into()),
        };
        let response = self
            .http
            .post(url.clone())
            .header(header::CONTENT_TYPE, "application/json")
            .header("webhook-id", id)
            .header("webhook-timestamp", timestamp.to_string())
            .header("webhook-signature", signature)
            .body(body.to_owned())
            .send()
            .await;
        match response {
            Ok(response) => {
                let status = response.status();
                let code = Some(status.as_u16());
                if status.is_success() {
                    Attempt::Delivered(code)
                } else if status == StatusCode::REQUEST_TIMEOUT
                    || status == StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()
                {
                    Attempt::Retry(code, format!("the receiver answered {}", status.as_u16()))
                } else {
                    Attempt::Abandon(code, format!("the receiver answered {}", status.as_u16()))
                }
            }
            // The URL can authorize whoever holds it, so failures never
            // repeat it.
            Err(error) => Attempt::Retry(None, error.without_url().to_string()),
        }
    }

    /// Attempts the notification due first; `false` when none is due. One
    /// task delivers, and no transaction waits on the receiver: a delivery
    /// cut short before its outcome is recorded is attempted again, and
    /// receivers recognize it by its `webhook-id`.
    pub async fn deliver_next(&self) -> Result<bool> {
        let Some(row) = sqlx::query(
            "SELECT notification_id, channel_id, payload, attempts, created_at \
             FROM alert_notifications WHERE state = 'queued' AND next_attempt_at <= ?1 \
             ORDER BY next_attempt_at LIMIT 1",
        )
        .bind(Utc::now())
        .fetch_optional(self.database.pool())
        .await
        .map_err(storage_error)?
        else {
            return Ok(false);
        };
        let id: Uuid = row.try_get("notification_id").map_err(storage_error)?;
        let channel: String = row.try_get("channel_id").map_err(storage_error)?;
        let payload: String = row.try_get("payload").map_err(storage_error)?;
        let attempts: i32 = row.try_get("attempts").map_err(storage_error)?;
        let created_at: DateTime<Utc> = row.try_get("created_at").map_err(storage_error)?;
        let attempt = match self.channels.destination(&channel).await {
            Ok(destination) => self.send(&destination, &id.to_string(), &payload).await,
            Err(error) => Attempt::Retry(None, error.message),
        };
        let now = Utc::now();
        let attempts = attempts.saturating_add(1);
        let delay = BACKOFF[usize::try_from(attempts - 1)
            .unwrap_or(0)
            .min(BACKOFF.len() - 1)];
        let retry_at = now + TimeDelta::seconds(delay);
        let (state, next_attempt_at, delivered_at, failure) = match attempt {
            Attempt::Delivered(_) => ("delivered", None, Some(now), String::new()),
            Attempt::Retry(_, failure) if retry_at < created_at + GIVE_UP_AFTER => {
                ("queued", Some(retry_at), None, failure)
            }
            Attempt::Retry(_, failure) | Attempt::Abandon(_, failure) => {
                ("abandoned", None, None, failure)
            }
        };
        sqlx::query(
            "UPDATE alert_notifications SET state = ?2, attempts = ?3, next_attempt_at = ?4, \
             delivered_at = ?5, last_failure = ?6 WHERE notification_id = ?1 AND state = 'queued'",
        )
        .bind(id)
        .bind(state)
        .bind(attempts)
        .bind(next_attempt_at)
        .bind(delivered_at)
        .bind(failure)
        .execute(self.database.pool())
        .await
        .map_err(storage_error)?;
        Ok(true)
    }

    /// Sends a test notification to `channel` now.
    pub async fn test(&self, channel: &str) -> Result<TestOutcome> {
        let destination = self.channels.destination(channel).await?;
        let body = self.notices.test_payload(channel, Utc::now()).to_string();
        Ok(
            match self
                .send(&destination, &Uuid::now_v7().to_string(), &body)
                .await
            {
                Attempt::Delivered(status) => TestOutcome {
                    delivered: true,
                    status,
                    failure: String::new(),
                },
                Attempt::Retry(status, failure) | Attempt::Abandon(status, failure) => {
                    TestOutcome {
                        delivered: false,
                        status,
                        failure,
                    }
                }
            },
        )
    }

    /// Notifications newest first, of one rule or channel when given.
    pub async fn list(
        &self,
        rule: Option<&str>,
        channel: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<NotificationRecord>> {
        let limit = limit.unwrap_or(DEFAULT_LIST).clamp(1, MAX_LIST);
        sqlx::query(
            "SELECT notification_id, rule_id, channel_id, kind, state, attempts, created_at, \
             next_attempt_at, delivered_at, last_failure FROM alert_notifications \
             WHERE (?1 IS NULL OR rule_id = ?1) AND (?2 IS NULL OR channel_id = ?2) \
             ORDER BY created_at DESC, notification_id DESC LIMIT ?3",
        )
        .bind(rule)
        .bind(channel)
        .bind(i64::from(limit))
        .fetch_all(self.database.pool())
        .await
        .map_err(storage_error)?
        .iter()
        .map(|row| {
            let attempts: i32 = row.try_get("attempts").map_err(storage_error)?;
            Ok(NotificationRecord {
                id: row.try_get("notification_id").map_err(storage_error)?,
                rule: row.try_get("rule_id").map_err(storage_error)?,
                channel: row.try_get("channel_id").map_err(storage_error)?,
                kind: row.try_get("kind").map_err(storage_error)?,
                state: row.try_get("state").map_err(storage_error)?,
                attempts: u32::try_from(attempts).unwrap_or_default(),
                created_at: row.try_get("created_at").map_err(storage_error)?,
                next_attempt_at: row.try_get("next_attempt_at").map_err(storage_error)?,
                delivered_at: row.try_get("delivered_at").map_err(storage_error)?,
                last_failure: row.try_get("last_failure").map_err(storage_error)?,
            })
        })
        .collect()
    }

    /// Forgets notifications delivered or abandoned a week ago.
    pub async fn purge(&self, now: DateTime<Utc>) -> Result<u64> {
        Ok(sqlx::query(
            "DELETE FROM alert_notifications WHERE state <> 'queued' \
             AND COALESCE(delivered_at, created_at) < ?1",
        )
        .bind(now - KEEP)
        .execute(self.database.pool())
        .await
        .map_err(storage_error)?
        .rows_affected())
    }

    /// Delivers due notifications until `shutdown`; `migrated` resolves once
    /// the schema is ready.
    pub async fn run(self, migrated: impl Future<Output = bool>, shutdown: CancellationToken) {
        tokio::select! {
            () = shutdown.cancelled() => return,
            ready = migrated => if !ready { return },
        }
        let mut poll = tokio::time::interval(POLL);
        let mut purge = tokio::time::interval(PURGE_EVERY);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = purge.tick() => {
                    if let Err(error) = self.purge(Utc::now()).await {
                        tracing::warn!(error = %error.message, "cannot forget old notifications");
                    }
                }
                _ = poll.tick() => loop {
                    match self.deliver_next().await {
                        Ok(true) if !shutdown.is_cancelled() => {}
                        Ok(_) => break,
                        Err(error) => {
                            tracing::warn!(error = %error.message, "cannot deliver notifications");
                            break;
                        }
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_receive_the_payloads_one_alert() {
        let payload = serde_json::json!({
            "version": "4",
            "status": "firing",
            "externalURL": "https://panel.example/",
            "alerts": [{
                "status": "firing",
                "labels": {"alertname": "errors", "severity": "critical"},
                "annotations": {"summary": "5xx above 5%", "value": 0.07},
                "startsAt": "2026-10-07T10:00:00Z",
                "endsAt": UNENDED,
                "generatorURL": "https://panel.example/alerts/errors",
                "fingerprint": "8a1f",
            }],
        });
        let request = notify_request("ops", &payload.to_string()).unwrap();
        assert_eq!(request.channel, "ops");
        assert_eq!(request.external_url, "https://panel.example/");
        let alert = request.alert.unwrap();
        assert_eq!(alert.status, "firing");
        assert_eq!(alert.labels["severity"], "critical");
        assert_eq!(alert.annotations["value"], "0.07");
        assert_eq!(alert.ends_at, "");
        assert_eq!(alert.fingerprint, "8a1f");
        assert!(notify_request("ops", "{}").is_none());
    }
}
