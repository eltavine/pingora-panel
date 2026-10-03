//! How consecutive failed logins slow an account down.

use std::time::Duration;

/// No wait after the first `free` consecutive failures, then a wait that
/// doubles from `base` up to `max`; at `disable_after` the password stops
/// working until an Administrator unlocks it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FailurePolicy {
    pub free: u32,
    pub base: Duration,
    pub max: Duration,
    pub disable_after: u32,
}

impl Default for FailurePolicy {
    fn default() -> Self {
        Self {
            free: 4,
            base: Duration::from_secs(30),
            max: Duration::from_secs(3600),
            disable_after: 100,
        }
    }
}

impl FailurePolicy {
    /// How long the account waits after its `failures`-th consecutive
    /// failure before it accepts another attempt.
    pub fn wait_after(&self, failures: u32) -> Option<Duration> {
        let doublings = failures.checked_sub(self.free + 1)?;
        let factor = 1u32.checked_shl(doublings.min(31)).unwrap_or(u32::MAX);
        Some(self.base.saturating_mul(factor).min(self.max))
    }

    pub fn disables(&self, failures: u32) -> bool {
        failures >= self.disable_after
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_double_from_the_fifth_failure_up_to_an_hour() {
        let policy = FailurePolicy::default();
        let waits: Vec<_> = (1..=12)
            .map(|failures| policy.wait_after(failures).map(|wait| wait.as_secs()))
            .collect();
        assert_eq!(
            waits,
            [
                None,
                None,
                None,
                None,
                Some(30),
                Some(60),
                Some(120),
                Some(240),
                Some(480),
                Some(960),
                Some(1920),
                Some(3600)
            ]
        );
        assert_eq!(policy.wait_after(99), Some(Duration::from_secs(3600)));
        assert!(!policy.disables(99));
        assert!(policy.disables(100));
    }
}
