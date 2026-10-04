//! Notifications as Alertmanager's webhook payload, version 4, one alert
//! each, queued in the transaction of the state change they report.

use super::rules::RuleRecord;
use chrono::{DateTime, SecondsFormat, Utc};
use panel_errors::Result;
use panel_sqlite::storage_error;
use serde_json::{json, Map, Value};
use sqlx::SqliteConnection;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Alertmanager's `endsAt` for an alert that has not ended.
const UNENDED: &str = "0001-01-01T00:00:00Z";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Firing,
    Resolved,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Firing => "firing",
            Self::Resolved => "resolved",
        }
    }
}

fn rfc3339(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A label set's fingerprint as Prometheus and Alertmanager compute it:
/// FNV-1a over each name and value, each followed by the byte 255.
fn fingerprint(labels: &BTreeMap<&str, String>) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = OFFSET;
    for (name, value) in labels {
        for byte in name.bytes().chain([255]).chain(value.bytes()).chain([255]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    format!("{hash:016x}")
}

/// One alert as it happened.
pub(crate) struct Occurrence {
    pub kind: Kind,
    pub fired_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub value: Option<f64>,
}

/// Builds and queues notifications, with links to the console when the
/// panel knows where it is reached.
#[derive(Clone, Debug, Default)]
pub struct Notices {
    console: String,
}

impl Notices {
    /// `console` is the console's public origin, such as
    /// `https://panel.example`.
    pub fn new(console: Option<String>) -> Self {
        Self {
            console: console
                .map(|origin| origin.trim_end_matches('/').to_owned())
                .unwrap_or_default(),
        }
    }

    fn link(&self, path: &str) -> String {
        if self.console.is_empty() {
            String::new()
        } else {
            format!("{}{path}", self.console)
        }
    }

    fn envelope(
        &self,
        channel: &str,
        status: &str,
        labels: &BTreeMap<&str, String>,
        annotations: &Map<String, Value>,
        alert: Value,
        group: &str,
    ) -> Value {
        json!({
            "version": "4",
            "groupKey": format!("{{}}:{{rule=\"{group}\"}}"),
            "truncatedAlerts": 0,
            "status": status,
            "receiver": channel,
            "groupLabels": { "rule": group },
            "commonLabels": labels,
            "commonAnnotations": annotations,
            "externalURL": self.link("/"),
            "alerts": [alert],
        })
    }

    /// The payload telling `channel` that `rule`'s alert fired or resolved.
    pub(crate) fn payload(
        &self,
        rule: &RuleRecord,
        channel: &str,
        occurrence: &Occurrence,
    ) -> Value {
        let spec = &rule.spec;
        let mut labels = BTreeMap::from([
            ("alertname", spec.name.clone()),
            ("rule", rule.id.clone()),
            ("measure", spec.measure.name().to_owned()),
            ("severity", spec.severity.name().to_owned()),
        ]);
        if let Some(site) = &spec.site {
            labels.insert("site", site.as_str().to_owned());
        }
        if let Some(route) = &spec.route {
            labels.insert("route", route.as_str().to_owned());
        }
        if let Some(upstream) = &spec.upstream {
            labels.insert("upstream", upstream.as_str().to_owned());
        }
        let mut annotations = Map::new();
        annotations.insert(
            "summary".into(),
            json!(format!("{}: {}", spec.name, spec.condition())),
        );
        if !spec.description.is_empty() {
            annotations.insert("description".into(), json!(spec.description));
        }
        annotations.insert("threshold".into(), json!(spec.threshold.to_string()));
        if let Some(value) = occurrence.value {
            annotations.insert("value".into(), json!(value.to_string()));
        }
        let status = occurrence.kind.name();
        let alert = json!({
            "status": status,
            "labels": labels,
            "annotations": annotations,
            "startsAt": rfc3339(occurrence.fired_at),
            "endsAt": occurrence.resolved_at.map_or_else(|| UNENDED.to_owned(), rfc3339),
            "generatorURL": self.link(&format!("/alerts?rule={}", rule.id)),
            "fingerprint": fingerprint(&labels),
        });
        self.envelope(channel, status, &labels, &annotations, alert, &rule.id)
    }

    /// A firing notification that names no rule, to check a receiver.
    pub(crate) fn test_payload(&self, channel: &str, at: DateTime<Utc>) -> Value {
        let labels = BTreeMap::from([
            ("alertname", "Test notification".to_owned()),
            ("severity", "warning".to_owned()),
            ("test", "true".to_owned()),
        ]);
        let mut annotations = Map::new();
        annotations.insert(
            "summary".into(),
            json!("A test of this channel; no rule fired."),
        );
        let alert = json!({
            "status": "firing",
            "labels": labels,
            "annotations": annotations,
            "startsAt": rfc3339(at),
            "endsAt": UNENDED,
            "generatorURL": self.link("/alerts"),
            "fingerprint": fingerprint(&labels),
        });
        self.envelope(channel, "firing", &labels, &annotations, alert, "test")
    }

    /// Queues the notification of `occurrence` for each of `rule`'s
    /// channels, due now.
    pub(crate) async fn queue(
        &self,
        connection: &mut SqliteConnection,
        rule: &RuleRecord,
        occurrence: &Occurrence,
        now: DateTime<Utc>,
    ) -> Result<()> {
        for channel in &rule.spec.channels {
            sqlx::query(
                "INSERT INTO alert_notifications (notification_id, rule_id, channel_id, kind, \
                 payload, state, created_at, next_attempt_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?6)",
            )
            .bind(Uuid::now_v7())
            .bind(&rule.id)
            .bind(channel)
            .bind(occurrence.kind.name())
            .bind(self.payload(rule, channel, occurrence).to_string())
            .bind(now)
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_hash_names_and_values_with_separators() {
        // FNV-1a 64 of `alertname`, 0xff, `up`, 0xff.
        let labels = BTreeMap::from([("alertname", "up".to_owned())]);
        assert_eq!(fingerprint(&labels), "41f2ed4244916247");
        assert_ne!(
            fingerprint(&BTreeMap::from([("a", "bc".to_owned())])),
            fingerprint(&BTreeMap::from([("ab", "c".to_owned())]))
        );
    }
}
