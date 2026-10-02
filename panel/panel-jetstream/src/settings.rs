use panel_errors::{PanelError, Result};
use panel_events::{ConsumerName, EventType, EventVersion};
use std::time::Duration;

/// Names and retention of the product's JetStream resources.
///
/// Subjects:
/// - `<prefix>.events.<event type>.v<major>` carries every domain event;
/// - `<prefix>.replay.<consumer>` redelivers a parked event to one consumer;
/// - `<prefix>.dlq.<consumer>` parks events a consumer cannot process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JetStreamSettings {
    subject_prefix: String,
    stream_prefix: String,
    events_max_age: Duration,
    dead_letter_max_age: Duration,
    duplicate_window: Duration,
}

impl Default for JetStreamSettings {
    fn default() -> Self {
        Self {
            subject_prefix: "panel".into(),
            stream_prefix: "PANEL".into(),
            events_max_age: Duration::from_secs(7 * 24 * 3600),
            dead_letter_max_age: Duration::from_secs(30 * 24 * 3600),
            duplicate_window: Duration::from_secs(10 * 60),
        }
    }
}

impl JetStreamSettings {
    /// `subject_prefix` is one or more lowercase subject tokens;
    /// `stream_prefix` is an uppercase stream-name stem.
    pub fn with_prefixes(
        mut self,
        subject_prefix: impl Into<String>,
        stream_prefix: impl Into<String>,
    ) -> Result<Self> {
        let subject_prefix = subject_prefix.into();
        let stream_prefix = stream_prefix.into();
        let token = |token: &str| {
            !token.is_empty()
                && token.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
                })
        };
        if subject_prefix.len() > 64 || !subject_prefix.split('.').all(token) {
            return Err(PanelError::invalid_argument(
                "subject prefixes are dot-separated lowercase tokens of at most 64 bytes",
            ));
        }
        if stream_prefix.is_empty()
            || stream_prefix.len() > 32
            || !stream_prefix
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(PanelError::invalid_argument(
                "stream prefixes contain 1..=32 uppercase letters, digits or '_'",
            ));
        }
        self.subject_prefix = subject_prefix;
        self.stream_prefix = stream_prefix;
        Ok(self)
    }

    pub fn with_retention(mut self, events: Duration, dead_letters: Duration) -> Self {
        self.events_max_age = events;
        self.dead_letter_max_age = dead_letters;
        self
    }

    /// Window in which a repeated event ID is acknowledged but not stored
    /// again. It bounds how long a crashed relay's republish is deduplicated.
    pub fn with_duplicate_window(mut self, window: Duration) -> Self {
        self.duplicate_window = window;
        self
    }

    pub fn events_stream(&self) -> String {
        format!("{}_EVENTS", self.stream_prefix)
    }

    pub fn dead_letter_stream(&self) -> String {
        format!("{}_DLQ", self.stream_prefix)
    }

    /// Key-value bucket of live service instances.
    pub fn service_bucket(&self) -> String {
        format!("{}_SERVICES", self.stream_prefix)
    }

    pub fn event_subject(&self, event_type: &EventType, version: EventVersion) -> String {
        format!(
            "{}.events.{event_type}.v{}",
            self.subject_prefix,
            version.get()
        )
    }

    /// Subject filter for events matching `pattern`, a dot-separated subject
    /// pattern over event types and versions such as `config.>` or
    /// `config.revision.*.v1`.
    pub fn event_filter(&self, pattern: &str) -> Result<String> {
        let tokens = pattern.split('.').collect::<Vec<_>>();
        let valid = tokens.iter().enumerate().all(|(index, token)| {
            *token == "*"
                || (*token == ">" && index == tokens.len() - 1)
                || (!token.is_empty()
                    && token.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
                    }))
        });
        if !valid || pattern.len() > 160 {
            return Err(PanelError::invalid_argument(format!(
                "{pattern:?} is not an event subject pattern"
            )));
        }
        Ok(format!("{}.events.{pattern}", self.subject_prefix))
    }

    pub fn replay_subject(&self, consumer: &ConsumerName) -> String {
        format!("{}.replay.{consumer}", self.subject_prefix)
    }

    pub fn dead_letter_subject(&self, consumer: &ConsumerName) -> String {
        format!("{}.dlq.{consumer}", self.subject_prefix)
    }

    pub(crate) fn event_subjects(&self) -> Vec<String> {
        vec![
            format!("{}.events.>", self.subject_prefix),
            format!("{}.replay.>", self.subject_prefix),
        ]
    }

    pub(crate) fn dead_letter_subjects(&self) -> Vec<String> {
        vec![format!("{}.dlq.>", self.subject_prefix)]
    }

    pub(crate) fn events_max_age(&self) -> Duration {
        self.events_max_age
    }

    pub(crate) fn dead_letter_max_age(&self) -> Duration {
        self.dead_letter_max_age
    }

    pub(crate) fn duplicate_window(&self) -> Duration {
        self.duplicate_window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_follow_the_documented_layout() {
        let settings = JetStreamSettings::default();
        let consumer = ConsumerName::new("audit-projection").unwrap();
        assert_eq!(settings.events_stream(), "PANEL_EVENTS");
        assert_eq!(settings.dead_letter_stream(), "PANEL_DLQ");
        assert_eq!(
            settings.event_subject(
                &EventType::new("config.revision.activated").unwrap(),
                EventVersion::new(2).unwrap()
            ),
            "panel.events.config.revision.activated.v2"
        );
        assert_eq!(
            settings.replay_subject(&consumer),
            "panel.replay.audit-projection"
        );
        assert_eq!(
            settings.dead_letter_subject(&consumer),
            "panel.dlq.audit-projection"
        );
        assert_eq!(
            settings.event_filter("config.>").unwrap(),
            "panel.events.config.>"
        );
        assert_eq!(
            settings.event_filter("config.*.activated.v1").unwrap(),
            "panel.events.config.*.activated.v1"
        );
    }

    #[test]
    fn prefixes_and_filters_are_validated() {
        assert!(JetStreamSettings::default()
            .with_prefixes("t1.panel", "T1_PANEL")
            .is_ok());
        assert!(JetStreamSettings::default()
            .with_prefixes("Panel", "PANEL")
            .is_err());
        assert!(JetStreamSettings::default()
            .with_prefixes("panel", "panel")
            .is_err());
        assert!(JetStreamSettings::default()
            .with_prefixes("panel..x", "PANEL")
            .is_err());
        let settings = JetStreamSettings::default();
        for pattern in ["", ">.config", "config.>.v1", "Config.x", "config x"] {
            assert!(settings.event_filter(pattern).is_err(), "{pattern:?}");
        }
    }
}
