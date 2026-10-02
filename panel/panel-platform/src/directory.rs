use crate::ServiceDescriptor;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_errors::Result;
use serde::Serialize;

/// The live service instances known at one moment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ServiceListing {
    observed_at: DateTime<Utc>,
    services: Vec<ServiceDescriptor>,
}

impl ServiceListing {
    /// Orders instances by service name, then by start time.
    pub fn new(observed_at: DateTime<Utc>, mut services: Vec<ServiceDescriptor>) -> Self {
        services.sort_by(|a, b| {
            (a.service(), a.started_at(), a.instance_id()).cmp(&(
                b.service(),
                b.started_at(),
                b.instance_id(),
            ))
        });
        Self {
            observed_at,
            services,
        }
    }

    pub fn observed_at(&self) -> DateTime<Utc> {
        self.observed_at
    }

    pub fn services(&self) -> &[ServiceDescriptor] {
        &self.services
    }

    /// Instances that provide the capability `name`.
    pub fn providers<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a ServiceDescriptor> {
        self.services
            .iter()
            .filter(move |descriptor| descriptor.provides(name))
    }
}

/// Lists the service instances that are currently registered.
#[async_trait]
pub trait ServiceDirectory: Send + Sync {
    async fn list(&self) -> Result<ServiceListing>;
}

/// Records a running instance until it deregisters or its registration
/// expires.
#[async_trait]
pub trait ServiceRegistrar: Send + Sync {
    /// Creates or refreshes the registration of `descriptor`.
    async fn register(&self, descriptor: &ServiceDescriptor) -> Result<()>;

    async fn deregister(&self, descriptor: &ServiceDescriptor) -> Result<()>;
}
