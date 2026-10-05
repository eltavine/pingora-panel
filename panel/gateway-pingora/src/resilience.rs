//! What an upstream does about failures and load (ADR 0038): which failures
//! it retries within its budget, its circuit, and how many requests it takes
//! at once.

use panel_ir::{CircuitBreaker, RetryBudget, RetryCondition, RetryPolicy, UpstreamQueue};
use parking_lot::Mutex;
use pingora_core::ErrorType;
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicU32, Ordering::Relaxed},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Budgets and circuits count the last this many seconds.
const WINDOW_SECONDS: u64 = 10;
/// Backoff never grows beyond this many times its base.
const BACKOFF_CAP: u64 = 10;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Bucket {
    second: u64,
    requests: u32,
    retries: u32,
    attempts: u32,
    failures: u32,
}

/// Per-second counts of the last [`WINDOW_SECONDS`].
#[derive(Debug, Default)]
struct Window {
    buckets: [Bucket; WINDOW_SECONDS as usize],
}

impl Window {
    fn bucket(&mut self, second: u64) -> &mut Bucket {
        let bucket = &mut self.buckets[(second % WINDOW_SECONDS) as usize];
        if bucket.second != second {
            *bucket = Bucket {
                second,
                ..Bucket::default()
            };
        }
        bucket
    }

    fn totals(&self, second: u64) -> Bucket {
        self.buckets
            .iter()
            .filter(|bucket| second.saturating_sub(bucket.second) < WINDOW_SECONDS)
            .fold(Bucket::default(), |total, bucket| Bucket {
                second,
                requests: total.requests.saturating_add(bucket.requests),
                retries: total.retries.saturating_add(bucket.retries),
                attempts: total.attempts.saturating_add(bucket.attempts),
                failures: total.failures.saturating_add(bucket.failures),
            })
    }

    fn clear_outcomes(&mut self) {
        for bucket in &mut self.buckets {
            bucket.attempts = 0;
            bucket.failures = 0;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Circuit {
    #[default]
    Closed,
    Open {
        until_ms: u64,
    },
    HalfOpen {
        since_ms: u64,
        admitted: u32,
        succeeded: u32,
    },
}

#[derive(Debug, Default)]
struct PoolInner {
    window: Window,
    circuit: Circuit,
}

/// What one upstream has seen in this process, kept across snapshots:
/// recent requests, retries and failures, and its circuit.
#[derive(Debug, Default)]
pub(crate) struct PoolState {
    inner: Mutex<PoolInner>,
}

impl PoolState {
    /// Counts a request toward the budget's share.
    pub(crate) fn request(&self, now_ms: u64) {
        self.inner.lock().window.bucket(now_ms / 1000).requests += 1;
    }

    /// Whether `budget` allows one more retry now; an allowed retry counts.
    pub(crate) fn may_retry(&self, budget: Option<RetryBudget>, now_ms: u64) -> bool {
        let mut inner = self.inner.lock();
        let second = now_ms / 1000;
        if let Some(budget) = budget {
            let totals = inner.window.totals(second);
            let share = u64::from(totals.requests) * u64::from(budget.percent) / 100;
            let floor = u64::from(budget.min_per_second) * WINDOW_SECONDS;
            if u64::from(totals.retries) >= share.max(floor) {
                return false;
            }
        }
        inner.window.bucket(second).retries += 1;
        true
    }

    /// Whether a request may go to the upstream: `Ok(true)` for a trial of a
    /// half-open circuit, or `Err` with the seconds until it may be tried.
    pub(crate) fn admit(&self, breaker: Option<&CircuitBreaker>, now_ms: u64) -> Result<bool, u64> {
        let Some(breaker) = breaker else {
            return Ok(false);
        };
        let mut inner = self.inner.lock();
        match inner.circuit {
            Circuit::Closed => Ok(false),
            Circuit::Open { until_ms } if now_ms < until_ms => {
                Err((until_ms - now_ms).div_ceil(1000).max(1))
            }
            Circuit::Open { .. } => {
                inner.circuit = Circuit::HalfOpen {
                    since_ms: now_ms,
                    admitted: 1,
                    succeeded: 0,
                };
                Ok(true)
            }
            Circuit::HalfOpen {
                since_ms,
                admitted,
                succeeded,
            } => {
                // Trials that never report back would hold the circuit half
                // open for good; after another open period new ones go.
                let stale = now_ms.saturating_sub(since_ms) >= breaker.open_ms;
                let admitted = if stale { succeeded } else { admitted };
                if admitted < breaker.half_open_requests {
                    inner.circuit = Circuit::HalfOpen {
                        since_ms: if stale { now_ms } else { since_ms },
                        admitted: admitted + 1,
                        succeeded,
                    };
                    Ok(true)
                } else {
                    Err(1)
                }
            }
        }
    }

    /// Gives back a trial that never reached the upstream.
    pub(crate) fn cancel_trial(&self) {
        let mut inner = self.inner.lock();
        if let Circuit::HalfOpen { admitted, .. } = &mut inner.circuit {
            *admitted = admitted.saturating_sub(1);
        }
    }

    /// Counts an attempt's outcome and moves the circuit on.
    pub(crate) fn outcome(
        &self,
        failed: bool,
        trial: bool,
        breaker: Option<&CircuitBreaker>,
        now_ms: u64,
    ) {
        let mut inner = self.inner.lock();
        let second = now_ms / 1000;
        let bucket = inner.window.bucket(second);
        bucket.attempts += 1;
        bucket.failures += u32::from(failed);
        let Some(breaker) = breaker else {
            return;
        };
        let open = Circuit::Open {
            until_ms: now_ms.saturating_add(breaker.open_ms),
        };
        match inner.circuit {
            Circuit::Closed => {
                let totals = inner.window.totals(second);
                if totals.attempts >= breaker.min_requests
                    && u64::from(totals.failures) * 100
                        >= u64::from(breaker.failure_percent) * u64::from(totals.attempts)
                {
                    inner.circuit = open;
                    inner.window.clear_outcomes();
                }
            }
            Circuit::HalfOpen {
                since_ms,
                admitted,
                succeeded,
            } if trial => {
                if failed {
                    inner.circuit = open;
                } else if succeeded + 1 >= breaker.half_open_requests {
                    inner.circuit = Circuit::Closed;
                    inner.window.clear_outcomes();
                } else {
                    inner.circuit = Circuit::HalfOpen {
                        since_ms,
                        admitted,
                        succeeded: succeeded + 1,
                    };
                }
            }
            Circuit::HalfOpen { .. } | Circuit::Open { .. } => {}
        }
    }
}

/// An upstream's retry policy as the proxy applies it.
#[derive(Clone, Debug, Default)]
pub(crate) struct RetryRules {
    pub attempts: u32,
    statuses: BTreeSet<u16>,
    timeout: bool,
    reset: bool,
    backoff_ms: u64,
    pub budget: Option<RetryBudget>,
}

impl RetryRules {
    pub(crate) fn compile(policy: &RetryPolicy) -> Self {
        Self {
            attempts: policy.attempts,
            statuses: policy.retry_statuses.clone(),
            timeout: policy.retry_on.contains(&RetryCondition::Timeout),
            reset: policy.retry_on.contains(&RetryCondition::Reset),
            backoff_ms: policy.backoff_ms,
            budget: policy.budget,
        }
    }

    pub(crate) fn retries_status(&self, status: u16) -> bool {
        self.statuses.contains(&status)
    }

    /// Whether a failure of this kind after the request was sent is retried.
    pub(crate) fn retries_error(&self, kind: &ErrorType) -> bool {
        match kind {
            ErrorType::ReadTimedout | ErrorType::WriteTimedout => self.timeout,
            ErrorType::ConnectionClosed
            | ErrorType::ReadError
            | ErrorType::WriteError
            | ErrorType::H2Error => self.reset,
            _ => false,
        }
    }

    /// The wait before retry `retry` (1 for the first): drawn at random up
    /// to the base doubled for each earlier retry, capped at ten times it.
    pub(crate) fn delay(&self, retry: u32) -> Duration {
        if self.backoff_ms == 0 || retry == 0 {
            return Duration::ZERO;
        }
        let ceiling = self
            .backoff_ms
            .saturating_mul(1 << (retry - 1).min(16))
            .min(self.backoff_ms.saturating_mul(BACKOFF_CAP));
        let mut bytes = [0; 8];
        let _ = getrandom::fill(&mut bytes);
        Duration::from_millis(u64::from_le_bytes(bytes) % (ceiling + 1))
    }
}

/// Why a request did not get one of an upstream's places.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Busy {
    /// Every place is taken and nobody may wait.
    Full,
    /// The request waited as long as the queue allows.
    Waited,
}

/// The requests an upstream takes at once, and the queue for more.
#[derive(Debug)]
pub(crate) struct Limit {
    places: Arc<Semaphore>,
    queue: Option<UpstreamQueue>,
    waiting: AtomicU32,
}

impl Limit {
    pub(crate) fn new(max_requests: u32, queue: Option<UpstreamQueue>) -> Self {
        Self {
            places: Arc::new(Semaphore::new(max_requests as usize)),
            queue,
            waiting: AtomicU32::new(0),
        }
    }

    /// A place for one request, waiting first in, first out when the
    /// upstream has a queue.
    pub(crate) async fn acquire(&self) -> Result<OwnedSemaphorePermit, Busy> {
        if let Ok(place) = Arc::clone(&self.places).try_acquire_owned() {
            return Ok(place);
        }
        let Some(queue) = self.queue else {
            return Err(Busy::Full);
        };
        let mut waiting = self.waiting.load(Relaxed);
        loop {
            if waiting >= queue.max_waiting {
                return Err(Busy::Full);
            }
            match self
                .waiting
                .compare_exchange_weak(waiting, waiting + 1, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(actual) => waiting = actual,
            }
        }
        let waited = tokio::time::timeout(
            Duration::from_millis(queue.timeout_ms),
            Arc::clone(&self.places).acquire_owned(),
        )
        .await;
        self.waiting.fetch_sub(1, Relaxed);
        match waited {
            Ok(Ok(place)) => Ok(place),
            Ok(Err(_)) | Err(_) => Err(Busy::Waited),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breaker() -> CircuitBreaker {
        CircuitBreaker {
            failure_percent: 50,
            min_requests: 4,
            open_ms: 5_000,
            half_open_requests: 2,
        }
    }

    #[test]
    fn budgets_cap_retries_at_a_share_with_a_floor() {
        let state = PoolState::default();
        let budget = Some(RetryBudget {
            percent: 20,
            min_per_second: 0,
        });
        for _ in 0..10 {
            state.request(1_000);
        }
        assert!(state.may_retry(budget, 1_000));
        assert!(state.may_retry(budget, 1_500));
        assert!(!state.may_retry(budget, 2_000), "two of ten requests");
        let floor = Some(RetryBudget {
            percent: 1,
            min_per_second: 1,
        });
        let quiet = PoolState::default();
        assert!((0..10).all(|_| quiet.may_retry(floor, 1_000)));
        assert!(!quiet.may_retry(floor, 1_000));
        assert!(quiet.may_retry(floor, 12_000), "the window moved on");
        assert!(state.may_retry(None, 2_000));
    }

    #[test]
    fn circuits_open_on_a_share_of_failures_and_close_after_trials() {
        let state = PoolState::default();
        let breaker = breaker();
        let rules = Some(&breaker);
        for failed in [false, true, false] {
            state.outcome(failed, false, rules, 1_000);
        }
        assert_eq!(state.admit(rules, 1_000), Ok(false), "too few requests");
        state.outcome(true, false, rules, 1_000);
        assert_eq!(state.admit(rules, 1_000), Err(5));
        assert_eq!(state.admit(rules, 4_500), Err(2));
        assert_eq!(state.admit(rules, 6_000), Ok(true));
        assert_eq!(state.admit(rules, 6_000), Ok(true));
        assert_eq!(state.admit(rules, 6_000), Err(1), "two trials at most");
        state.outcome(false, true, rules, 6_100);
        assert_eq!(state.admit(rules, 6_200), Err(1));
        state.outcome(false, true, rules, 6_300);
        assert_eq!(state.admit(rules, 6_400), Ok(false), "closed again");

        for _ in 0..4 {
            state.outcome(true, false, rules, 7_000);
        }
        assert_eq!(state.admit(rules, 12_000), Ok(true));
        state.outcome(true, true, rules, 12_100);
        assert_eq!(
            state.admit(rules, 12_200),
            Err(5),
            "a failed trial opens it"
        );
        assert_eq!(state.admit(None, 12_200), Ok(false));
    }

    #[test]
    fn trials_that_never_report_are_given_back() {
        let state = PoolState::default();
        let breaker = CircuitBreaker {
            half_open_requests: 1,
            ..breaker()
        };
        let rules = Some(&breaker);
        for _ in 0..4 {
            state.outcome(true, false, rules, 0);
        }
        assert_eq!(state.admit(rules, 5_000), Ok(true));
        state.cancel_trial();
        assert_eq!(state.admit(rules, 5_001), Ok(true));
        assert_eq!(state.admit(rules, 5_002), Err(1));
        assert_eq!(state.admit(rules, 10_002), Ok(true), "a stale trial");
    }

    #[test]
    fn retries_follow_their_rules_and_back_off_with_jitter() {
        let rules = RetryRules::compile(&RetryPolicy {
            attempts: 2,
            retry_statuses: [503].into(),
            retry_on: [RetryCondition::Reset].into(),
            backoff_ms: 100,
            ..RetryPolicy::none()
        });
        assert!(rules.retries_status(503) && !rules.retries_status(500));
        assert!(rules.retries_error(&ErrorType::ConnectionClosed));
        assert!(!rules.retries_error(&ErrorType::ReadTimedout));
        for retry in 1..=6 {
            let ceiling = (100 << (retry - 1)).min(1_000);
            assert!(rules.delay(retry) <= Duration::from_millis(ceiling));
        }
        assert_eq!(RetryRules::default().delay(3), Duration::ZERO);
    }

    #[tokio::test]
    async fn limits_queue_first_in_first_out_for_a_bounded_time() {
        let limit = Limit::new(
            1,
            Some(UpstreamQueue {
                max_waiting: 1,
                timeout_ms: 200,
            }),
        );
        let first = limit.acquire().await.unwrap();
        let waiting = {
            let limit = &limit;
            async move { limit.acquire().await }
        };
        let (second, third) = tokio::join!(waiting, async {
            tokio::task::yield_now().await;
            let refused = limit.acquire().await;
            drop(first);
            refused
        });
        assert!(second.is_ok(), "the waiting request gets the place");
        assert_eq!(third.unwrap_err(), Busy::Full, "the queue holds one");
        let held = second.unwrap();
        assert_eq!(limit.acquire().await.unwrap_err(), Busy::Waited);
        drop(held);
        assert!(Limit::new(1, None).acquire().await.is_ok());
    }
}
