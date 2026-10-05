//! An upstream's `retry`, `circuit_breaker` and `queue` directives
//! (ADR 0038).

use super::Lowerer;
use crate::{
    codes,
    values::{self, Params},
};
use panel_config_model::UpstreamRetry;
use panel_dsl::{Argument, Directive};
use panel_ir::{CircuitBreaker, RetryBudget, RetryCondition, UpstreamQueue};

/// What `circuit_breaker` means when a parameter is not written.
pub const DEFAULT_BREAKER: CircuitBreaker = CircuitBreaker {
    failure_percent: 50,
    min_requests: 20,
    open_ms: 30_000,
    half_open_requests: 1,
};
/// What `queue` means when a parameter is not written.
pub const DEFAULT_QUEUE: UpstreamQueue = UpstreamQueue {
    max_waiting: 100,
    timeout_ms: 2_000,
};

impl Lowerer<'_> {
    pub(super) fn retry(&mut self, file: &str, directive: &Directive) -> Option<UpstreamRetry> {
        let params = Params::split(&directive.args);
        self.only_params(file, &params, &["on", "backoff", "budget", "budget_min"]);
        let [attempts_arg] = params.positional.as_slice() else {
            self.error_with_help(
                file,
                directive.span,
                codes::ARGUMENTS,
                "retry takes how many times to retry",
                "write it as `retry 2 on=timeout,reset,503 backoff=25ms budget=20%;`",
            );
            return None;
        };
        let attempts = self
            .value(file, attempts_arg)
            .and_then(|value| self.number(file, attempts_arg, &value, "a whole number"))?;
        let mut retry = UpstreamRetry {
            attempts,
            ..UpstreamRetry::default()
        };
        if let Some((value, arg)) = params.named.get("on") {
            for item in value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                match item {
                    "timeout" => {
                        retry.on.insert(RetryCondition::Timeout);
                    }
                    "reset" => {
                        retry.on.insert(RetryCondition::Reset);
                    }
                    status => match status.parse::<u16>() {
                        Ok(status) if status.to_string() == item => {
                            retry.statuses.insert(status);
                        }
                        _ => self.error_with_help(
                            file,
                            arg.span,
                            codes::TYPE,
                            format!("{item:?} is not timeout, reset or a status"),
                            "write on=timeout,reset,502,503,504",
                        ),
                    },
                }
            }
        }
        if let Some((value, arg)) = params.named.get("backoff") {
            retry.backoff_ms = self.duration(file, arg, value)?;
        }
        if let Some((value, arg)) = params.named.get("budget") {
            let percent = self.percent(file, arg, value)?;
            let min_per_second = match params.named.get("budget_min") {
                Some((value, arg)) => self.number(file, arg, value, "a whole number")?,
                None => 0,
            };
            retry.budget = Some(RetryBudget {
                percent,
                min_per_second,
            });
        } else if let Some((_, arg)) = params.named.get("budget_min") {
            self.error_with_help(
                file,
                arg.span,
                codes::ARGUMENTS,
                "budget_min goes with a budget",
                "write budget=20% budget_min=3",
            );
        }
        Some(retry)
    }

    pub(super) fn circuit_breaker(
        &mut self,
        file: &str,
        directive: &Directive,
    ) -> Option<CircuitBreaker> {
        let params = Params::split(&directive.args);
        self.only_params(
            file,
            &params,
            &["failures", "min_requests", "open", "trials"],
        );
        self.no_positional(
            file,
            &params,
            "circuit_breaker failures=50% min_requests=20 open=30s;",
        );
        let mut breaker = DEFAULT_BREAKER;
        for (key, (value, arg)) in &params.named {
            match *key {
                "failures" => breaker.failure_percent = self.percent(file, arg, value)?,
                "min_requests" => {
                    breaker.min_requests = self.number(file, arg, value, "a whole number")?;
                }
                "open" => breaker.open_ms = self.duration(file, arg, value)?,
                "trials" => {
                    breaker.half_open_requests = self.number(file, arg, value, "a whole number")?;
                }
                _ => {}
            }
        }
        Some(breaker)
    }

    pub(super) fn queue(&mut self, file: &str, directive: &Directive) -> Option<UpstreamQueue> {
        let params = Params::split(&directive.args);
        self.only_params(file, &params, &["size", "timeout"]);
        self.no_positional(file, &params, "queue size=100 timeout=2s;");
        let mut queue = DEFAULT_QUEUE;
        if let Some((value, arg)) = params.named.get("size") {
            queue.max_waiting = self.number(file, arg, value, "a whole number")?;
        }
        if let Some((value, arg)) = params.named.get("timeout") {
            queue.timeout_ms = self.duration(file, arg, value)?;
        }
        Some(queue)
    }

    fn no_positional(&mut self, file: &str, params: &Params<'_>, syntax: &str) {
        for arg in &params.positional {
            self.error_with_help(
                file,
                arg.span,
                codes::ARGUMENTS,
                format!("{:?} is not a parameter", arg.value),
                format!("write it as `{syntax}`"),
            );
        }
    }

    /// A share such as `20%`.
    fn percent(&mut self, file: &str, arg: &Argument, value: &str) -> Option<u32> {
        let parsed = value
            .strip_suffix('%')
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|digits| digits.parse().ok());
        if parsed.is_none() {
            self.error_with_help(
                file,
                arg.span,
                codes::TYPE,
                format!("{value:?} is not a percentage"),
                "write shares such as 20%",
            );
        }
        parsed
    }
}

/// `retry`'s arguments, the inverse of [`Lowerer::retry`].
pub(crate) fn print_retry(retry: &UpstreamRetry) -> Vec<String> {
    let mut args = vec![retry.attempts.to_string()];
    let on: Vec<String> = retry
        .on
        .iter()
        .map(|condition| match condition {
            RetryCondition::Timeout => "timeout".to_owned(),
            RetryCondition::Reset => "reset".to_owned(),
        })
        .chain(retry.statuses.iter().map(u16::to_string))
        .collect();
    if !on.is_empty() {
        args.push(format!("on={}", on.join(",")));
    }
    if retry.backoff_ms > 0 {
        args.push(format!(
            "backoff={}",
            values::print_duration_ms(retry.backoff_ms)
        ));
    }
    if let Some(budget) = retry.budget {
        args.push(format!("budget={}%", budget.percent));
        if budget.min_per_second > 0 {
            args.push(format!("budget_min={}", budget.min_per_second));
        }
    }
    args
}

/// `circuit_breaker`'s parameters other than the defaults.
pub(crate) fn print_breaker(breaker: &CircuitBreaker) -> Vec<String> {
    let mut args = Vec::new();
    if breaker.failure_percent != DEFAULT_BREAKER.failure_percent {
        args.push(format!("failures={}%", breaker.failure_percent));
    }
    if breaker.min_requests != DEFAULT_BREAKER.min_requests {
        args.push(format!("min_requests={}", breaker.min_requests));
    }
    if breaker.open_ms != DEFAULT_BREAKER.open_ms {
        args.push(format!(
            "open={}",
            values::print_duration_ms(breaker.open_ms)
        ));
    }
    if breaker.half_open_requests != DEFAULT_BREAKER.half_open_requests {
        args.push(format!("trials={}", breaker.half_open_requests));
    }
    args
}

/// `queue`'s parameters other than the defaults.
pub(crate) fn print_queue(queue: &UpstreamQueue) -> Vec<String> {
    let mut args = Vec::new();
    if queue.max_waiting != DEFAULT_QUEUE.max_waiting {
        args.push(format!("size={}", queue.max_waiting));
    }
    if queue.timeout_ms != DEFAULT_QUEUE.timeout_ms {
        args.push(format!(
            "timeout={}",
            values::print_duration_ms(queue.timeout_ms)
        ));
    }
    args
}
