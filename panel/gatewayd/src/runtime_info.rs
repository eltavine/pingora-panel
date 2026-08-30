use panel_engine::{GatewayRuntimeInfo, GatewayRuntimeInfoProvider};
use std::{
    num::NonZeroU32,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub struct ProcessRuntimeInfo {
    gateway_version: String,
    started_at_unix_seconds: u64,
    started_at: Instant,
    worker_count: NonZeroU32,
}

impl ProcessRuntimeInfo {
    pub fn new(gateway_version: impl Into<String>, worker_count: NonZeroU32) -> Self {
        let started_at_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        Self {
            gateway_version: gateway_version.into(),
            started_at_unix_seconds,
            started_at: Instant::now(),
            worker_count,
        }
    }
}

impl GatewayRuntimeInfoProvider for ProcessRuntimeInfo {
    fn snapshot(&self) -> GatewayRuntimeInfo {
        GatewayRuntimeInfo {
            gateway_version: self.gateway_version.clone(),
            started_at_unix_seconds: self.started_at_unix_seconds,
            uptime_seconds: self.started_at.elapsed().as_secs(),
            worker_count: self.worker_count.get(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reports_stable_process_facts() {
        let source = ProcessRuntimeInfo::new("1.2.3", NonZeroU32::new(4).unwrap());
        let snapshot = source.snapshot();

        assert_eq!(snapshot.gateway_version, "1.2.3");
        assert_ne!(snapshot.started_at_unix_seconds, 0);
        assert_eq!(snapshot.worker_count, 4);
    }
}
