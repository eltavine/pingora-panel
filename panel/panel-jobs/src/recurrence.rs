use crate::{JobKind, JobOrigin, JobSpec, ScheduleName};
use chrono::{DateTime, Utc};
use panel_context::{IdempotencyKey, RequestId};
use panel_errors::Result;
use panel_schedule::{Recurrence, Window};
use std::time::Duration;

/// Recurring periods during which jobs that require the window may start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceWindow {
    name: ScheduleName,
    window: Window,
}

impl MaintenanceWindow {
    pub const MAX_DURATION: Duration = Window::MAX_DURATION;

    pub fn new(name: ScheduleName, recurrence: Recurrence, duration: Duration) -> Result<Self> {
        Ok(Self {
            name,
            window: Window::new(recurrence, duration)?,
        })
    }

    pub fn name(&self) -> &ScheduleName {
        &self.name
    }

    pub fn recurrence(&self) -> &Recurrence {
        self.window.recurrence()
    }

    pub fn duration(&self) -> Duration {
        self.window.duration()
    }

    /// Whether an occurrence started at or before `at` and has not ended.
    pub fn is_open(&self, at: DateTime<Utc>) -> bool {
        self.window.contains(at)
    }

    /// When the window next opens after `after`.
    pub fn next_opening(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.window.next_opening(after)
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
