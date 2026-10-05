//! Checks of how upstreams retry, break circuits and queue (ADR 0038):
//! bounds that keep retries, waits and trials from growing without limit,
//! and the capability snapshots using them require.

use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::{RuntimeSnapshot, UpstreamPoolSpec, UPSTREAM_RESILIENCE_CAPABILITY};

/// The most retries one request may make.
pub const MOST_RETRIES: u32 = 10;
/// The longest base delay between retries.
pub const MOST_BACKOFF_MS: u64 = 10_000;
/// The longest a circuit may stay open, an hour.
pub const MOST_OPEN_MS: u64 = 3_600_000;
/// The longest a request may wait in a queue, a minute.
pub const MOST_QUEUE_MS: u64 = 60_000;

pub(crate) fn validate_resilience(snapshot: &RuntimeSnapshot, diagnostics: &mut Vec<Diagnostic>) {
    let mut used = false;
    for pool in &snapshot.upstream_pools {
        used |= uses_resilience(pool);
        for problem in problems(pool) {
            diagnostics.push(
                Diagnostic::error(
                    ErrorCode::VALIDATION_FAILED,
                    format!("upstream {} {problem}", pool.id),
                )
                .with_resource(pool.id.as_str()),
            );
        }
    }
    let declared = snapshot
        .required_capabilities()
        .iter()
        .any(|capability| capability.name == UPSTREAM_RESILIENCE_CAPABILITY);
    if used && !declared {
        diagnostics.push(
            Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!(
                    "upstream retries, circuits, limits or h2c are used without requiring {UPSTREAM_RESILIENCE_CAPABILITY}"
                ),
            )
            .with_resource("upstream_pools"),
        );
    }
}

/// Whether `pool` uses a setting of ADR 0038.
pub fn uses_resilience(pool: &UpstreamPoolSpec) -> bool {
    let retry = &pool.retry_policy;
    retry.attempts > 0
        || !retry.retry_statuses.is_empty()
        || !retry.retry_on.is_empty()
        || retry.backoff_ms > 0
        || retry.budget.is_some()
        || pool.circuit_breaker.is_some()
        || pool.max_requests.is_some()
        || pool.queue.is_some()
        || pool.connection.h2c
}

/// What is wrong with `pool`'s retries, circuit and limits, one line each.
pub fn problems(pool: &UpstreamPoolSpec) -> Vec<String> {
    let mut found = Vec::new();
    let retry = &pool.retry_policy;
    if retry.attempts > MOST_RETRIES {
        found.push(format!(
            "retries {} times, more than {MOST_RETRIES}",
            retry.attempts
        ));
    }
    if retry.per_try_timeout_ms != 0 {
        found.push("limits single tries, which no gateway does yet; set a read timeout".into());
    }
    if retry.attempts == 0
        && (!retry.retry_statuses.is_empty()
            || !retry.retry_on.is_empty()
            || retry.backoff_ms > 0
            || retry.budget.is_some())
    {
        found.push("says what to retry but retries no times".into());
    }
    for status in retry
        .retry_statuses
        .iter()
        .filter(|status| !(400..=599).contains(*status))
    {
        found.push(format!("retries status {status}, which is not 400 to 599"));
    }
    if retry.backoff_ms > MOST_BACKOFF_MS {
        found.push(format!(
            "waits up to {} ms before a retry, more than {MOST_BACKOFF_MS}",
            retry.backoff_ms
        ));
    }
    if let Some(budget) = retry.budget {
        if !(1..=100).contains(&budget.percent) {
            found.push("budgets retries at a share that is not 1 to 100 percent".into());
        }
        if budget.min_per_second > 1000 {
            found.push("allows more than 1000 retries per second regardless of the budget".into());
        }
    }
    if let Some(breaker) = pool.circuit_breaker {
        if !(1..=100).contains(&breaker.failure_percent) {
            found.push("opens its circuit at a share that is not 1 to 100 percent".into());
        }
        if !(1..=100_000).contains(&breaker.min_requests) {
            found.push(
                "opens its circuit after a number of requests that is not 1 to 100000".into(),
            );
        }
        if !(1000..=MOST_OPEN_MS).contains(&breaker.open_ms) {
            found.push(format!(
                "keeps its circuit open for a time that is not 1000 to {MOST_OPEN_MS} ms"
            ));
        }
        if !(1..=100).contains(&breaker.half_open_requests) {
            found.push("lets a number of trial requests through that is not 1 to 100".into());
        }
    }
    if pool.max_requests == Some(0) {
        found.push("handles no requests at once".into());
    }
    if let Some(queue) = pool.queue {
        if pool.max_requests.is_none() {
            found.push("queues requests without a limit of requests at once".into());
        }
        if !(1..=100_000).contains(&queue.max_waiting) {
            found.push("queues a number of requests that is not 1 to 100000".into());
        }
        if !(1..=MOST_QUEUE_MS).contains(&queue.timeout_ms) {
            found.push(format!(
                "lets requests wait for a time that is not 1 to {MOST_QUEUE_MS} ms"
            ));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::UpstreamPoolId;
    use panel_ir::{CircuitBreaker, RetryBudget, RetryCondition, UpstreamQueue};

    fn pool() -> UpstreamPoolSpec {
        UpstreamPoolSpec::new(UpstreamPoolId::new("app").unwrap(), "app", Vec::new())
    }

    #[test]
    fn plain_pools_use_nothing_and_bounded_settings_pass() {
        assert!(!uses_resilience(&pool()));
        let mut bounded = pool();
        bounded.retry_policy.attempts = 2;
        bounded.retry_policy.retry_on = [RetryCondition::Timeout].into();
        bounded.retry_policy.retry_statuses = [503].into();
        bounded.retry_policy.backoff_ms = 25;
        bounded.retry_policy.budget = Some(RetryBudget {
            percent: 20,
            min_per_second: 3,
        });
        bounded.circuit_breaker = Some(CircuitBreaker {
            failure_percent: 50,
            min_requests: 20,
            open_ms: 30_000,
            half_open_requests: 1,
        });
        bounded.max_requests = Some(100);
        bounded.queue = Some(UpstreamQueue {
            max_waiting: 50,
            timeout_ms: 2000,
        });
        assert!(uses_resilience(&bounded));
        assert!(problems(&bounded).is_empty(), "{:?}", problems(&bounded));
    }

    #[test]
    fn unbounded_and_contradictory_settings_are_named() {
        let mut wrong = pool();
        wrong.retry_policy.per_try_timeout_ms = 100;
        wrong.retry_policy.retry_statuses = [302].into();
        wrong.retry_policy.backoff_ms = 60_000;
        wrong.circuit_breaker = Some(CircuitBreaker {
            failure_percent: 0,
            min_requests: 0,
            open_ms: 10,
            half_open_requests: 0,
        });
        wrong.max_requests = Some(0);
        wrong.queue = Some(UpstreamQueue {
            max_waiting: 0,
            timeout_ms: 0,
        });
        let found = problems(&wrong);
        for expected in [
            "limits single tries, which no gateway does yet; set a read timeout",
            "says what to retry but retries no times",
            "retries status 302, which is not 400 to 599",
            "opens its circuit at a share that is not 1 to 100 percent",
            "keeps its circuit open for a time that is not 1000 to 3600000 ms",
            "handles no requests at once",
            "queues a number of requests that is not 1 to 100000",
        ] {
            assert!(
                found.iter().any(|problem| problem == expected),
                "{expected}: {found:#?}"
            );
        }
        let mut queued = pool();
        queued.queue = Some(UpstreamQueue {
            max_waiting: 1,
            timeout_ms: 1,
        });
        assert_eq!(
            problems(&queued),
            ["queues requests without a limit of requests at once"]
        );
    }
}
