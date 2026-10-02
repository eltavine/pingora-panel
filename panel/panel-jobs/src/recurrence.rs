use crate::{JobKind, JobOrigin, JobSpec, ScheduleName};
use chrono::{DateTime, Utc};
use panel_context::{IdempotencyKey, RequestId};
use panel_errors::{PanelError, Result};
use rrule::{RRuleSet, Tz};
use std::{fmt, str::FromStr, time::Duration};

/// An RFC 5545 recurrence: a `DTSTART` in UTC or with a `TZID`, and
/// `RRULE` lines, such as
/// `DTSTART;TZID=Asia/Shanghai:20260101T030000\nRRULE:FREQ=DAILY`.
///
/// Rules with a time zone keep their wall-clock time across offset changes.
#[derive(Clone)]
pub struct Recurrence {
    text: String,
    set: RRuleSet,
}

impl Recurrence {
    pub fn parse(text: impl Into<String>) -> Result<Self> {
        let text = text.into();
        let set = RRuleSet::from_str(&text).map_err(|error| {
            PanelError::invalid_argument(format!("invalid RFC 5545 recurrence: {error}"))
        })?;
        if set.get_rrule().is_empty() {
            return Err(PanelError::invalid_argument(
                "a recurrence needs at least one RRULE",
            ));
        }
        Ok(Self { text, set })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The first occurrence strictly after `after`.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.set
            .clone()
            .after(after.with_timezone(&Tz::UTC))
            .all(2)
            .dates
            .into_iter()
            .map(|date| date.with_timezone(&Utc))
            .find(|date| *date > after)
    }

    /// The latest occurrence in `(from, to]`.
    fn latest_in(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.set
            .clone()
            .after(from.with_timezone(&Tz::UTC))
            .before(to.with_timezone(&Tz::UTC))
            .all(u16::MAX)
            .dates
            .into_iter()
            .map(|date| date.with_timezone(&Utc))
            .rfind(|date| *date > from)
    }
}

impl fmt::Debug for Recurrence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Recurrence").field(&self.text).finish()
    }
}

impl PartialEq for Recurrence {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for Recurrence {}

/// Recurring periods during which jobs that require the window may start,
/// each beginning at an occurrence and lasting `duration`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceWindow {
    name: ScheduleName,
    recurrence: Recurrence,
    duration: Duration,
}

impl MaintenanceWindow {
    pub const MAX_DURATION: Duration = Duration::from_secs(7 * 24 * 3600);

    pub fn new(name: ScheduleName, recurrence: Recurrence, duration: Duration) -> Result<Self> {
        if duration.is_zero() || duration > Self::MAX_DURATION {
            return Err(PanelError::invalid_argument(
                "maintenance windows last more than zero and at most seven days",
            ));
        }
        Ok(Self {
            name,
            recurrence,
            duration,
        })
    }

    pub fn name(&self) -> &ScheduleName {
        &self.name
    }

    pub fn recurrence(&self) -> &Recurrence {
        &self.recurrence
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Whether an occurrence started at or before `at` and has not ended.
    pub fn is_open(&self, at: DateTime<Utc>) -> bool {
        let Ok(duration) = chrono::Duration::from_std(self.duration) else {
            return false;
        };
        self.recurrence.latest_in(at - duration, at).is_some()
    }

    /// When the window next opens after `after`.
    pub fn next_opening(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.recurrence.next_after(after)
    }
}

/// What a schedule enqueues at each occurrence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobTemplate {
    pub kind: JobKind,
    pub media_type: String,
    pub payload: Vec<u8>,
    pub max_attempts: u32,
    pub priority: i16,
    pub maintenance_window: Option<ScheduleName>,
}

/// A job enqueued once for every occurrence of a recurrence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Schedule {
    pub name: ScheduleName,
    pub recurrence: Recurrence,
    pub template: JobTemplate,
    pub enabled: bool,
}

impl Schedule {
    /// The job for the occurrence at `at`. Its idempotency key names the
    /// schedule and occurrence, so every store enqueues it at most once.
    pub fn occurrence(&self, at: DateTime<Utc>) -> Result<JobSpec> {
        let stamp = at.format("%Y%m%dT%H%M%SZ");
        let request = RequestId::new(format!("schedule-{}-{stamp}", self.name))?;
        Ok(JobSpec {
            kind: self.template.kind.clone(),
            idempotency_key: IdempotencyKey::new(format!("schedule:{}:{stamp}", self.name))?,
            media_type: self.template.media_type.clone(),
            payload: self.template.payload.clone(),
            max_attempts: self.template.max_attempts,
            priority: self.template.priority,
            not_before: Some(at),
            maintenance_window: self.template.maintenance_window.clone(),
            origin: JobOrigin {
                correlation_id: request.clone(),
                causation_id: request,
            },
        })
    }
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
    fn occurrences_follow_rfc_5545_with_wall_clock_time_zones() {
        let daily = Recurrence::parse("DTSTART:20260101T030000Z\nRRULE:FREQ=DAILY").unwrap();
        assert_eq!(
            daily.next_after(at("2026-03-01T02:00:00Z")),
            Some(at("2026-03-01T03:00:00Z"))
        );
        assert_eq!(
            daily.next_after(at("2026-03-01T03:00:00Z")),
            Some(at("2026-03-02T03:00:00Z")),
            "the next occurrence is strictly later"
        );

        // 02:30 New York time is 07:30 UTC in winter and 06:30 UTC in summer.
        let local = Recurrence::parse(
            "DTSTART;TZID=America/New_York:20260101T023000\nRRULE:FREQ=WEEKLY;BYDAY=SU",
        )
        .unwrap();
        assert_eq!(
            local.next_after(at("2026-01-03T00:00:00Z")),
            Some(at("2026-01-04T07:30:00Z"))
        );
        assert_eq!(
            local.next_after(at("2026-06-01T00:00:00Z")),
            Some(at("2026-06-07T06:30:00Z"))
        );

        let finite =
            Recurrence::parse("DTSTART:20260101T000000Z\nRRULE:FREQ=DAILY;COUNT=2").unwrap();
        assert_eq!(finite.next_after(at("2026-01-05T00:00:00Z")), None);

        for invalid in ["", "RRULE:FREQ=DAILY;BOGUS=1", "DTSTART:20260101T000000Z"] {
            assert!(Recurrence::parse(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn windows_open_for_their_duration_after_each_occurrence() {
        let window = MaintenanceWindow::new(
            ScheduleName::new("nightly").unwrap(),
            Recurrence::parse("DTSTART:20260101T010000Z\nRRULE:FREQ=DAILY").unwrap(),
            Duration::from_secs(2 * 3600),
        )
        .unwrap();
        assert!(!window.is_open(at("2026-03-01T00:59:59Z")));
        assert!(window.is_open(at("2026-03-01T01:00:00Z")));
        assert!(window.is_open(at("2026-03-01T02:59:59Z")));
        assert!(!window.is_open(at("2026-03-01T03:00:00Z")));
        assert_eq!(
            window.next_opening(at("2026-03-01T03:00:00Z")),
            Some(at("2026-03-02T01:00:00Z"))
        );
        assert!(MaintenanceWindow::new(
            ScheduleName::new("never").unwrap(),
            window.recurrence().clone(),
            Duration::ZERO
        )
        .is_err());
    }
}
