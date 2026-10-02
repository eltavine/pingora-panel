use crate::{ComponentType, HealthStatus, Impact, ServiceIdentity};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{collections::BTreeMap, time::Duration};

/// What a service can currently serve, derived from its checks.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceMode {
    /// Reads and writes are served.
    Normal,
    /// Reads are served; writes are suspended until dependencies recover.
    Degraded,
    /// Nothing is served, including before the first evaluation.
    Unavailable,
}

impl ServiceMode {
    pub fn accepts_writes(self) -> bool {
        matches!(self, Self::Normal)
    }

    pub fn accepts_reads(self) -> bool {
        matches!(self, Self::Normal | Self::Degraded)
    }
}

/// One measured component in a health document.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentHealth {
    component_type: ComponentType,
    observed_value: u64,
    observed_unit: &'static str,
    status: HealthStatus,
    time: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip)]
    impact: Impact,
}

impl ComponentHealth {
    pub(crate) fn new(
        component_type: ComponentType,
        response_time: Duration,
        status: HealthStatus,
        output: Option<String>,
        time: DateTime<Utc>,
        impact: Impact,
    ) -> Self {
        Self {
            component_type,
            observed_value: u64::try_from(response_time.as_millis()).unwrap_or(u64::MAX),
            observed_unit: "ms",
            status,
            time,
            output,
            impact,
        }
    }

    pub fn component_type(&self) -> ComponentType {
        self.component_type
    }

    pub fn status(&self) -> HealthStatus {
        self.status
    }

    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    pub fn impact(&self) -> Impact {
        self.impact
    }

    /// Response time of the check in milliseconds.
    pub fn response_time_millis(&self) -> u64 {
        self.observed_value
    }
}

/// A health document for one service.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    status: HealthStatus,
    version: String,
    release_id: String,
    service_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    checks: BTreeMap<String, Vec<ComponentHealth>>,
    #[serde(skip)]
    mode: ServiceMode,
}

impl HealthReport {
    /// The report published before the first evaluation completes.
    pub fn starting(identity: &ServiceIdentity) -> Self {
        Self::summary(identity, HealthStatus::Fail, Some("starting"))
    }

    /// A liveness report: the process is able to answer.
    pub fn alive(identity: &ServiceIdentity) -> Self {
        Self::summary(identity, HealthStatus::Pass, None)
    }

    fn summary(identity: &ServiceIdentity, status: HealthStatus, output: Option<&str>) -> Self {
        Self {
            status,
            version: identity.version().into(),
            release_id: identity.release_id().into(),
            service_id: identity.service_id().into(),
            output: output.map(Into::into),
            checks: BTreeMap::new(),
            mode: if status == HealthStatus::Fail {
                ServiceMode::Unavailable
            } else {
                ServiceMode::Normal
            },
        }
    }

    pub(crate) fn aggregate(
        identity: &ServiceIdentity,
        components: impl IntoIterator<Item = (String, ComponentHealth)>,
    ) -> Self {
        let mut status = HealthStatus::Pass;
        let mut mode = ServiceMode::Normal;
        let mut checks = BTreeMap::<String, Vec<ComponentHealth>>::new();
        for (component, health) in components {
            let (component_status, component_mode) = match (health.status, health.impact) {
                (HealthStatus::Fail, Impact::Required) => {
                    (HealthStatus::Fail, ServiceMode::Unavailable)
                }
                (HealthStatus::Fail, Impact::Degrading) => {
                    (HealthStatus::Warn, ServiceMode::Degraded)
                }
                (HealthStatus::Pass, _) => (HealthStatus::Pass, ServiceMode::Normal),
                _ => (HealthStatus::Warn, ServiceMode::Normal),
            };
            status = status.max(component_status);
            mode = worse(mode, component_mode);
            checks
                .entry(format!("{component}:responseTime"))
                .or_default()
                .push(health);
        }
        let output = match mode {
            ServiceMode::Unavailable => Some("a required dependency is unavailable"),
            ServiceMode::Degraded => Some("writes are suspended until dependencies recover"),
            ServiceMode::Normal => {
                (status == HealthStatus::Warn).then_some("a dependency reported concerns")
            }
        };
        Self {
            status,
            version: identity.version().into(),
            release_id: identity.release_id().into(),
            service_id: identity.service_id().into(),
            output: output.map(Into::into),
            checks,
            mode,
        }
    }

    pub fn status(&self) -> HealthStatus {
        self.status
    }

    pub fn mode(&self) -> ServiceMode {
        self.mode
    }

    /// The liveness view of this report: the same service, passing, without
    /// dependency checks.
    pub fn liveness(&self) -> Self {
        Self {
            status: HealthStatus::Pass,
            version: self.version.clone(),
            release_id: self.release_id.clone(),
            service_id: self.service_id.clone(),
            output: None,
            checks: BTreeMap::new(),
            mode: ServiceMode::Normal,
        }
    }

    pub fn service_id(&self) -> &str {
        &self.service_id
    }

    pub fn release_id(&self) -> &str {
        &self.release_id
    }

    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    /// Components keyed `<component>:responseTime`.
    pub fn checks(&self) -> &BTreeMap<String, Vec<ComponentHealth>> {
        &self.checks
    }

    /// HTTP status for this document: `fail` is 503, otherwise 200.
    pub fn http_status(&self) -> u16 {
        if self.status == HealthStatus::Fail {
            503
        } else {
            200
        }
    }
}

fn worse(left: ServiceMode, right: ServiceMode) -> ServiceMode {
    let rank = |mode| match mode {
        ServiceMode::Normal => 0,
        ServiceMode::Degraded => 1,
        ServiceMode::Unavailable => 2,
    };
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}
