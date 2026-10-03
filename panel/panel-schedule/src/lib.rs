#![forbid(unsafe_code)]

//! Recurring time as RFC 5545 has it: recurrences, and the windows they
//! open, which schedules, maintenance windows, approval policies and grants
//! share.

use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use rrule::{Frequency, RRuleSet, Tz};
use serde::{Deserialize, Serialize};
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

/// Recurring periods, each opening at an occurrence of a recurrence that
/// repeats at most daily and lasting from a minute to a week.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WindowFields", into = "WindowFields")]
pub struct Window {
    recurrence: Recurrence,
    duration: Duration,
}

/// How a window is written.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
struct WindowFields {
    /// An RFC 5545 recurrence repeating at most daily, with `DTSTART` in UTC
    /// or with a `TZID`, such as
    /// `DTSTART;TZID=Europe/Berlin:20260105T090000\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR`.
    recurrence: String,
    /// How long each period lasts.
    #[cfg_attr(feature = "openapi", schema(minimum = 1, maximum = 10080))]
    minutes: u32,
}

impl Window {
    pub const MAX_DURATION: Duration = Duration::from_secs(7 * 24 * 3600);
    const MIN_DURATION: Duration = Duration::from_secs(60);

    pub fn new(recurrence: Recurrence, duration: Duration) -> Result<Self> {
        if !(Self::MIN_DURATION..=Self::MAX_DURATION).contains(&duration) {
            return Err(PanelError::invalid_argument(
                "a window lasts from one minute to seven days",
            ));
        }
        if recurrence
            .set
            .get_rrule()
            .iter()
            .any(|rule| rule.get_freq() > Frequency::Daily)
        {
            return Err(PanelError::invalid_argument(
                "a window repeats at most daily",
            ));
        }
        Ok(Self {
            recurrence,
            duration,
        })
    }

    pub fn recurrence(&self) -> &Recurrence {
        &self.recurrence
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Whether a period started at or before `at` and has not ended.
    pub fn contains(&self, at: DateTime<Utc>) -> bool {
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

impl TryFrom<WindowFields> for Window {
    type Error = PanelError;

    fn try_from(fields: WindowFields) -> Result<Self> {
        Self::new(
            Recurrence::parse(fields.recurrence)?,
            Duration::from_secs(u64::from(fields.minutes) * 60),
        )
    }
}

impl From<Window> for WindowFields {
    fn from(window: Window) -> Self {
        Self {
            recurrence: window.recurrence.text,
            minutes: u32::try_from(window.duration.as_secs() / 60).unwrap_or(u32::MAX),
        }
    }
}

#[cfg(feature = "openapi")]
impl utoipa::PartialSchema for Window {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        <WindowFields as utoipa::PartialSchema>::schema()
    }
}

#[cfg(feature = "openapi")]
impl utoipa::ToSchema for Window {
    fn name() -> std::borrow::Cow<'static, str> {
        "Window".into()
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
        let window = Window::new(
            Recurrence::parse("DTSTART:20260101T010000Z\nRRULE:FREQ=DAILY").unwrap(),
            Duration::from_secs(2 * 3600),
        )
        .unwrap();
        assert!(!window.contains(at("2026-03-01T00:59:59Z")));
        assert!(window.contains(at("2026-03-01T01:00:00Z")));
        assert!(window.contains(at("2026-03-01T02:59:59Z")));
        assert!(!window.contains(at("2026-03-01T03:00:00Z")));
        assert_eq!(
            window.next_opening(at("2026-03-01T03:00:00Z")),
            Some(at("2026-03-02T01:00:00Z"))
        );
        for duration in [
            Duration::ZERO,
            Duration::from_secs(59),
            Window::MAX_DURATION * 2,
        ] {
            assert!(Window::new(window.recurrence().clone(), duration).is_err());
        }
        let hourly = Recurrence::parse("DTSTART:20260101T000000Z\nRRULE:FREQ=HOURLY").unwrap();
        assert!(Window::new(hourly, Duration::from_secs(60)).is_err());
    }

    #[test]
    fn windows_keep_local_hours_and_may_cross_midnight() {
        let text = "DTSTART;TZID=Asia/Shanghai:20260105T220000\nRRULE:FREQ=WEEKLY;BYDAY=MO,FR";
        let window: Window =
            serde_json::from_value(serde_json::json!({"recurrence": text, "minutes": 240}))
                .unwrap();
        // 22:00 to 02:00 in Shanghai is 14:00 to 18:00 UTC.
        assert!(window.contains(at("2026-10-05T14:00:00Z")), "Monday");
        assert!(window.contains(at("2026-10-05T17:59:00Z")));
        assert!(!window.contains(at("2026-10-05T18:00:00Z")));
        assert!(!window.contains(at("2026-10-06T14:30:00Z")), "Tuesday");
        assert_eq!(
            serde_json::to_value(&window).unwrap(),
            serde_json::json!({"recurrence": text, "minutes": 240})
        );
        for invalid in [
            serde_json::json!({"recurrence": text, "minutes": 0}),
            serde_json::json!({"recurrence": "RRULE:FREQ=DAILY;BOGUS=1", "minutes": 60}),
            serde_json::json!({"recurrence": text, "minutes": 60, "days": ["mon"]}),
        ] {
            assert!(serde_json::from_value::<Window>(invalid).is_err());
        }
    }
}
