use panel_errors::{PanelError, Result};
use std::time::Duration;

/// Exponential backoff with equal jitter.
///
/// Attempt `n` may wait at most `min(max, initial × 2^(n−1))`. Half of that
/// ceiling is always waited and the other half is random, so retries of
/// failing jobs spread out without ever retrying immediately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    initial: Duration,
    max: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(2),
            max: Duration::from_secs(3600),
        }
    }
}

impl RetryPolicy {
    pub fn new(initial: Duration, max: Duration) -> Result<Self> {
        if initial.is_zero() || max < initial {
            return Err(PanelError::invalid_argument(
                "retry delays must satisfy 0 < initial <= max",
            ));
        }
        Ok(Self { initial, max })
    }

    /// The longest delay before attempt `attempt + 1`, for `attempt >= 1`.
    pub fn ceiling(&self, attempt: u32) -> Duration {
        let doublings = attempt.saturating_sub(1).min(32);
        self.initial
            .checked_mul(1u32 << doublings.min(31))
            .map_or(self.max, |delay| delay.min(self.max))
    }

    /// The delay before attempt `attempt + 1`.
    pub fn delay(&self, attempt: u32) -> Duration {
        let ceiling = self.ceiling(attempt);
        let half = ceiling / 2;
        let spread = u64::try_from((ceiling - half).as_millis()).unwrap_or(u64::MAX);
        let random = getrandom::u64().unwrap_or_default();
        half + Duration::from_millis(if spread == 0 {
            0
        } else {
            random % (spread + 1)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_double_up_to_the_maximum_with_bounded_jitter() {
        let policy = RetryPolicy::new(Duration::from_secs(1), Duration::from_secs(10)).unwrap();
        let ceilings: Vec<_> = (1..=6)
            .map(|attempt| policy.ceiling(attempt).as_secs())
            .collect();
        assert_eq!(ceilings, [1, 2, 4, 8, 10, 10]);
        assert_eq!(policy.ceiling(u32::MAX), Duration::from_secs(10));
        for attempt in 1..=6 {
            for _ in 0..50 {
                let delay = policy.delay(attempt);
                let ceiling = policy.ceiling(attempt);
                assert!(
                    delay >= ceiling / 2 && delay <= ceiling,
                    "{delay:?} for {attempt}"
                );
            }
        }
        assert!(RetryPolicy::new(Duration::ZERO, Duration::from_secs(1)).is_err());
        assert!(RetryPolicy::new(Duration::from_secs(2), Duration::from_secs(1)).is_err());
    }
}
