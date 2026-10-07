//! `GatewayRuntimePort` over the gateway runtime API.

use crate::{context, hash, query_context, response_error, status_error, GatewayGrpcClient};
use async_trait::async_trait;
use panel_application::{
    CachePurge, CachePurged, CacheStats, CommandContext, DataPlaneListener, DataPlaneState,
    EndpointHealth, EscapingLink, FileChecks, GatewayRuntimePort, PrivateKeyCheck, RequestScope,
    SiteCacheStats, StaticRootCheck, UpstreamHealth, UpstreamHealthReport,
};
use panel_contracts::gateway::v1::{self as wire, gateway_runtime_client::GatewayRuntimeClient};
use panel_errors::{PanelError, Result};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tonic::transport::Channel;

fn time(value: Option<prost_types::Timestamp>) -> Option<SystemTime> {
    let value = value?;
    UNIX_EPOCH.checked_add(Duration::new(
        u64::try_from(value.seconds).ok()?,
        u32::try_from(value.nanos).ok()?,
    ))
}

fn data_plane(value: Option<wire::DataPlane>) -> Result<DataPlaneState> {
    let value =
        value.ok_or_else(|| PanelError::internal("the gateway sent no data plane state"))?;
    let runtime = value.runtime.unwrap_or_default();
    let mut state = DataPlaneState::default();
    state.generation = value.generation;
    state.worker_count = value.worker_count;
    state.listeners = value
        .listeners
        .into_iter()
        .map(|listener| {
            let mut decoded = DataPlaneListener::new(listener.id, listener.address);
            decoded.tls = listener.tls;
            decoded.http1 = listener.http1;
            decoded.http2 = listener.http2;
            decoded
        })
        .collect();
    state.generation_started_at = time(value.generation_started_at);
    state.error = (!value.error.is_empty()).then_some(value.error);
    state.gateway_version = runtime.gateway_version;
    state.engine_version = runtime.data_plane_version;
    state.adapter_version = runtime.adapter_version;
    state.started_at = UNIX_EPOCH.checked_add(Duration::from_secs(runtime.started_at_unix_seconds));
    state.uptime_seconds = runtime.uptime_seconds;
    state.observed_at = time(value.observed_at);
    state.active_revision_id = (value.active_revision_id != 0).then_some(value.active_revision_id);
    state.active_hash = value
        .active_hash
        .map(|value| hash(Some(value)))
        .transpose()?;
    Ok(state)
}

fn upstream(value: wire::UpstreamHealth) -> UpstreamHealth {
    let mut decoded = UpstreamHealth::default();
    decoded.upstream_id = value.upstream_id;
    decoded.checked = value.checked;
    decoded.endpoints = value
        .endpoints
        .into_iter()
        .map(|endpoint| {
            let mut health = EndpointHealth::default();
            health.endpoint_id = endpoint.endpoint_id;
            health.address = endpoint.address;
            health.weight = endpoint.weight;
            health.enabled = endpoint.enabled;
            health.backup = endpoint.backup;
            health.healthy = endpoint.healthy;
            health.drained = endpoint.drained;
            health.ejected_until = time(endpoint.ejected_until);
            health.in_flight = endpoint.in_flight;
            health.requests = endpoint.requests;
            health.failures = endpoint.failures;
            health.latency_us = endpoint.latency_us;
            health
        })
        .collect();
    decoded
}

impl GatewayGrpcClient {
    fn runtime(&self) -> GatewayRuntimeClient<Channel> {
        GatewayRuntimeClient::new(self.channel.clone())
            .max_decoding_message_size(self.max_message_bytes)
            .max_encoding_message_size(self.max_message_bytes)
    }
}

#[async_trait]
impl GatewayRuntimePort for GatewayGrpcClient {
    async fn data_plane(&self, scope: RequestScope) -> Result<DataPlaneState> {
        let request = wire::GetDataPlaneRequest {
            context: Some(query_context(&scope)),
        };
        let response = self
            .runtime()
            .get_data_plane(self.request(request, scope.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        data_plane(response.data_plane)
    }

    async fn reload(&self, command: CommandContext) -> Result<DataPlaneState> {
        let request = wire::ReloadDataPlaneRequest {
            context: Some(context(&command)),
        };
        let response = self
            .runtime()
            .reload_data_plane(self.request(request, command.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        data_plane(response.data_plane)
    }

    async fn set_worker_count(
        &self,
        command: CommandContext,
        workers: u32,
    ) -> Result<DataPlaneState> {
        let request = wire::SetWorkerCountRequest {
            context: Some(context(&command)),
            worker_count: workers,
        };
        let response = self
            .runtime()
            .set_worker_count(self.request(request, command.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        data_plane(response.data_plane)
    }

    async fn shutdown(&self, command: CommandContext) -> Result<()> {
        let request = wire::ShutdownGatewayRequest {
            context: Some(context(&command)),
        };
        let response = self
            .runtime()
            .shutdown_gateway(self.request(request, command.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)
    }

    async fn upstream_health(&self, scope: RequestScope) -> Result<UpstreamHealthReport> {
        let request = wire::ListUpstreamHealthRequest {
            context: Some(query_context(&scope)),
        };
        let response = self
            .runtime()
            .list_upstream_health(self.request(request, scope.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        let mut report = UpstreamHealthReport::default();
        report.upstreams = response.upstreams.into_iter().map(upstream).collect();
        report.observed_at = time(response.observed_at);
        report.active_revision_id =
            (response.active_revision_id != 0).then_some(response.active_revision_id);
        report.active_hash = response
            .active_hash
            .map(|value| hash(Some(value)))
            .transpose()?;
        Ok(report)
    }

    async fn file_checks(&self, scope: RequestScope) -> Result<FileChecks> {
        let request = wire::CheckFilesRequest {
            context: Some(query_context(&scope)),
        };
        let response = self
            .runtime()
            .check_files(self.request(request, scope.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        let checks = response
            .checks
            .ok_or_else(|| PanelError::internal("the gateway sent no file checks"))?;
        let mut report = FileChecks::default();
        report.checked_at = time(checks.checked_at);
        report.active_revision_id =
            (checks.active_revision_id != 0).then_some(checks.active_revision_id);
        report.private_keys = checks
            .private_keys
            .into_iter()
            .map(|key| {
                let mut check = PrivateKeyCheck::new(key.file);
                check.tls_profile_ids = key.tls_profile_ids;
                check.mode = key.mode;
                check.owner_only = key.owner_only;
                check.error = Some(key.error).filter(|error| !error.is_empty());
                check
            })
            .collect();
        report.static_roots = checks
            .static_roots
            .into_iter()
            .map(|root| {
                let mut check = StaticRootCheck::new(root.id, root.root);
                check.inside = root.inside;
                check.escaping_links = root
                    .escaping_links
                    .into_iter()
                    .map(|link| EscapingLink::new(link.path, link.target))
                    .collect();
                check.entries_checked = root.entries_checked;
                check.truncated = root.truncated;
                check.error = Some(root.error).filter(|error| !error.is_empty());
                check
            })
            .collect();
        Ok(report)
    }

    async fn cache_stats(&self, scope: RequestScope) -> Result<CacheStats> {
        let request = wire::GetCacheStatsRequest {
            context: Some(query_context(&scope)),
        };
        let response = self
            .runtime()
            .get_cache_stats(self.request(request, scope.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        let stats = response
            .stats
            .ok_or_else(|| PanelError::internal("the gateway sent no cache statistics"))?;
        let mut report = CacheStats::default();
        report.observed_at = time(stats.observed_at);
        report.since = time(stats.since);
        report.bytes = stats.bytes;
        report.entries = stats.entries;
        report.max_bytes = stats.max_bytes;
        report.sites = stats
            .sites
            .into_iter()
            .map(|site| {
                let mut counts = SiteCacheStats::default();
                counts.site_id = site.site_id;
                counts.hits = site.hits;
                counts.stale = site.stale;
                counts.updating = site.updating;
                counts.misses = site.misses;
                counts.expired = site.expired;
                counts.revalidated = site.revalidated;
                counts.bypasses = site.bypasses;
                counts.uncacheable = site.uncacheable;
                counts
            })
            .collect();
        Ok(report)
    }

    async fn purge_cache(&self, command: CommandContext, purge: CachePurge) -> Result<CachePurged> {
        use wire::purge_cache_request::Target;
        let target = match purge {
            CachePurge::All => Target::All(true),
            CachePurge::Sites(site_ids) => Target::Sites(wire::CacheSites { site_ids }),
            CachePurge::Urls(urls) => Target::Urls(wire::CacheUrls { urls }),
            _ => {
                return Err(PanelError::invalid_argument(
                    "this purge is not known to the gateway",
                ))
            }
        };
        let request = wire::PurgeCacheRequest {
            context: Some(context(&command)),
            target: Some(target),
        };
        let response = self
            .runtime()
            .purge_cache(self.request(request, command.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        let mut purged = CachePurged::default();
        purged.keys = response.keys;
        Ok(purged)
    }

    async fn set_endpoint_drained(
        &self,
        command: CommandContext,
        upstream_id: String,
        endpoint_id: String,
        drained: bool,
    ) -> Result<UpstreamHealth> {
        let request = wire::SetEndpointDrainedRequest {
            context: Some(context(&command)),
            upstream_id,
            endpoint_id,
            drained,
        };
        let response = self
            .runtime()
            .set_endpoint_drained(self.request(request, command.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        response
            .upstream
            .map(upstream)
            .ok_or_else(|| PanelError::internal("the gateway sent no upstream"))
    }
}
