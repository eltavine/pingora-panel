use crate::BackgroundTaskFailure;
use std::future::Future;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ShutdownReason {
    ShutdownRequested,
    ProcessSignal,
    BackgroundTaskFailure,
    BackgroundTaskMonitorClosed,
}

impl ShutdownReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShutdownRequested => "shutdown_requested",
            Self::ProcessSignal => "process_signal",
            Self::BackgroundTaskFailure => "background_task_failure",
            Self::BackgroundTaskMonitorClosed => "background_task_monitor_closed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ShutdownTrigger {
    ProcessSignal,
    BackgroundTaskFailure(BackgroundTaskFailure),
    BackgroundTaskMonitorClosed,
}

impl ShutdownTrigger {
    pub const fn reason(&self) -> ShutdownReason {
        match self {
            Self::ProcessSignal => ShutdownReason::ProcessSignal,
            Self::BackgroundTaskFailure(_) => ShutdownReason::BackgroundTaskFailure,
            Self::BackgroundTaskMonitorClosed => ShutdownReason::BackgroundTaskMonitorClosed,
        }
    }

    pub fn background_failure(&self) -> Option<&BackgroundTaskFailure> {
        match self {
            Self::BackgroundTaskFailure(failure) => Some(failure),
            Self::ProcessSignal | Self::BackgroundTaskMonitorClosed => None,
        }
    }

    pub fn into_background_failure(self) -> Option<BackgroundTaskFailure> {
        match self {
            Self::BackgroundTaskFailure(failure) => Some(failure),
            Self::ProcessSignal | Self::BackgroundTaskMonitorClosed => None,
        }
    }
}

/// Selects exactly one process shutdown cause without owning signal or task APIs.
///
/// Failure provenance wins when both inputs are already ready. Callers inject
/// futures, so OS signals and task supervisors remain replaceable adapters.
pub struct ShutdownArbiter;

impl ShutdownArbiter {
    pub async fn wait(
        process_signal: impl Future<Output = ()>,
        background_failure: impl Future<Output = Option<BackgroundTaskFailure>>,
    ) -> ShutdownTrigger {
        tokio::pin!(process_signal);
        tokio::pin!(background_failure);
        tokio::select! {
            biased;
            failure = &mut background_failure => match failure {
                Some(failure) => ShutdownTrigger::BackgroundTaskFailure(failure),
                None => ShutdownTrigger::BackgroundTaskMonitorClosed,
            },
            _ = &mut process_signal => ShutdownTrigger::ProcessSignal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BackgroundTaskSupervisor;

    #[tokio::test]
    async fn process_signal_wins_while_failure_source_is_pending() {
        let trigger = ShutdownArbiter::wait(async {}, std::future::pending()).await;
        assert_eq!(trigger, ShutdownTrigger::ProcessSignal);
        assert_eq!(trigger.reason(), ShutdownReason::ProcessSignal);
    }

    #[tokio::test]
    async fn ready_background_failure_preserves_provenance() {
        let supervisor = BackgroundTaskSupervisor::new();
        let mut monitor = supervisor.failure_monitor();
        supervisor.spawn_critical("failed-health-adapter", async {});
        let failure = monitor.next().await.unwrap();

        let trigger = ShutdownArbiter::wait(async {}, async { Some(failure.clone()) }).await;

        assert_eq!(trigger.reason(), ShutdownReason::BackgroundTaskFailure);
        assert_eq!(
            trigger.background_failure().unwrap().task_name(),
            "failed-health-adapter"
        );
        assert_eq!(trigger.into_background_failure(), Some(failure));
        assert!(supervisor.shutdown_and_join().await.is_err());
    }

    #[tokio::test]
    async fn closed_failure_source_is_a_distinct_typed_trigger() {
        let trigger = ShutdownArbiter::wait(std::future::pending(), async { None }).await;
        assert_eq!(trigger, ShutdownTrigger::BackgroundTaskMonitorClosed);
        assert_eq!(
            trigger.reason(),
            ShutdownReason::BackgroundTaskMonitorClosed
        );
    }
}
