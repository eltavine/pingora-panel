//! Spans of time added to or taken from now.

use std::time::Duration;

/// The longest span used; a time a century from now neither overflows nor
/// leaves the four-digit years that RFC 3339 text compares in order.
const CENTURY: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

/// `value` as a chrono duration, at most a century.
pub(crate) fn span(value: Duration) -> chrono::Duration {
    chrono::Duration::from_std(value.min(CENTURY)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Utc};

    #[test]
    fn spans_stop_at_a_century() {
        let now = Utc::now();
        let latest = now + span(Duration::MAX);
        let earliest = now - span(Duration::MAX);
        assert!(latest.year() < 10_000 && earliest.year() > 0);
        assert_eq!(span(Duration::from_secs(90)), chrono::Duration::seconds(90));
    }
}
