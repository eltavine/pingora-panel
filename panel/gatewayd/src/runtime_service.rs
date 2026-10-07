//! The gateway runtime API: data plane state, reloads, worker counts,
//! shutdown, upstream health, endpoint drains, file checks and the proxy
//! cache.

use crate::{runtime_settings::RuntimeSettingsStore, MAX_GATEWAY_WORKERS};
use gateway_pingora::{
    CachePurge, CacheReport, DataPlane, DataPlaneStatus, FileChecks, PingoraGatewayAdapter,
    PoolHealth,
};
use gateway_proto_codec::encode_hash;
use panel_contracts::{
    common::v1 as common,
    gateway::v1::{self as wire, gateway_runtime_server::GatewayRuntime},
};
use panel_engine::{GatewayEngine, GatewayRuntimeInfoProvider};
use panel_errors::{PanelError, Result};
use std::{
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

pub(crate) struct GatewayRuntimeService {
    pub plane: Arc<DataPlane>,
    pub adapter: Arc<PingoraGatewayAdapter>,
    pub engine: Arc<dyn GatewayEngine>,
    pub runtime_info: Arc<dyn GatewayRuntimeInfoProvider>,
    pub settings: Arc<RuntimeSettingsStore>,
    pub shutdown: CancellationToken,
}

fn timestamp(time: SystemTime) -> prost_types::Timestamp {
    let elapsed = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    prost_types::Timestamp {
        seconds: i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        nanos: i32::try_from(elapsed.subsec_nanos()).unwrap_or(0),
    }
}

fn failure(error: &PanelError) -> Option<common::Error> {
    Some(error.into())
}

fn encode_cache(report: CacheReport) -> wire::CacheStats {
    wire::CacheStats {
        observed_at: Some(timestamp(SystemTime::now())),
        since: Some(timestamp(report.since)),
        bytes: report.bytes,
        entries: report.entries,
        max_bytes: report.max_bytes,
        sites: report
            .sites
            .into_iter()
            .map(|site| {
                let count = |name: &str| {
                    site.outcomes
                        .iter()
                        .find(|(outcome, _)| *outcome == name)
                        .map_or(0, |(_, count)| *count)
                };
                wire::SiteCacheStats {
                    hits: count("hit"),
                    stale: count("stale"),
                    updating: count("updating"),
                    misses: count("miss"),
                    expired: count("expired"),
                    revalidated: count("revalidated"),
                    bypasses: count("bypass"),
                    uncacheable: count("uncacheable"),
                    site_id: site.site_id,
                }
            })
            .collect(),
    }
}

fn encode_checks(checks: FileChecks) -> wire::FileChecks {
    wire::FileChecks {
        checked_at: checks.checked_at.map(timestamp),
        active_revision_id: checks.active_revision_id.unwrap_or(0),
        private_keys: checks
            .private_keys
            .into_iter()
            .map(|key| wire::PrivateKeyCheck {
                file: key.file,
                tls_profile_ids: key.tls_profile_ids,
                mode: key.mode,
                owner_only: key.owner_only,
                error: key.error.unwrap_or_default(),
            })
            .collect(),
        static_roots: checks
            .static_roots
            .into_iter()
            .map(|root| wire::StaticRootCheck {
                id: root.id,
                root: root.root,
                inside: root.inside,
                escaping_links: root
                    .escaping_links
                    .into_iter()
                    .map(|link| wire::EscapingLink {
                        path: link.path,
                        target: link.target,
                    })
                    .collect(),
                entries_checked: root.entries_checked,
                truncated: root.truncated,
                error: root.error.unwrap_or_default(),
            })
            .collect(),
    }
}

pub(crate) fn encode_pool(pool: &PoolHealth) -> wire::UpstreamHealth {
    wire::UpstreamHealth {
        upstream_id: pool.pool_id.clone(),
        checked: pool.checked,
        endpoints: pool
            .endpoints
            .iter()
            .map(|endpoint| wire::EndpointHealth {
                endpoint_id: endpoint.endpoint_id.clone(),
                address: endpoint.address.clone(),
                weight: endpoint.weight,
                enabled: endpoint.enabled,
                backup: endpoint.backup,
                healthy: endpoint.healthy,
                drained: endpoint.drained,
                ejected_until: endpoint
                    .ejected_until_ms
                    .map(|millis| timestamp(UNIX_EPOCH + Duration::from_millis(millis))),
                in_flight: endpoint.in_flight,
                requests: endpoint.requests,
                failures: endpoint.failures,
                latency_us: endpoint.latency_us,
            })
            .collect(),
    }
}

impl GatewayRuntimeService {
    async fn data_plane(&self, status: DataPlaneStatus) -> Result<wire::DataPlane> {
        let engine = self.engine.status().await?;
        let info = self.runtime_info.snapshot();
        Ok(wire::DataPlane {
            generation: status.generation,
            worker_count: u32::try_from(status.workers).unwrap_or(u32::MAX),
            listeners: status
                .listeners
                .iter()
                .map(|listener| wire::DataPlaneListener {
                    id: listener.id.clone(),
                    address: listener.address.clone(),
                    tls: listener.tls,
                    http1: listener.http1,
                    http2: listener.http2,
                })
                .collect(),
            generation_started_at: status.started_at.map(timestamp),
            error: status.error.unwrap_or_default(),
            runtime: Some(wire::GatewayRuntimeInfo {
                gateway_version: info.gateway_version,
                data_plane_version: self.adapter.pingora_package_version().into(),
                adapter_version: self.adapter.adapter_version().into(),
                started_at_unix_seconds: info.started_at_unix_seconds,
                uptime_seconds: info.uptime_seconds,
                worker_count: u32::try_from(status.workers).unwrap_or(u32::MAX),
            }),
            observed_at: Some(timestamp(SystemTime::now())),
            active_revision_id: engine
                .active_revision_id
                .map_or(0, |revision| revision.get()),
            active_hash: engine.active_hash.as_ref().map(encode_hash),
        })
    }
}

#[tonic::async_trait]
impl GatewayRuntime for GatewayRuntimeService {
    async fn get_data_plane(
        &self,
        _request: Request<wire::GetDataPlaneRequest>,
    ) -> std::result::Result<Response<wire::GetDataPlaneResponse>, Status> {
        Ok(Response::new(
            match self.data_plane(self.plane.status()).await {
                Ok(data_plane) => wire::GetDataPlaneResponse {
                    data_plane: Some(data_plane),
                    error: None,
                },
                Err(error) => wire::GetDataPlaneResponse {
                    data_plane: None,
                    error: failure(&error),
                },
            },
        ))
    }

    async fn reload_data_plane(
        &self,
        _request: Request<wire::ReloadDataPlaneRequest>,
    ) -> std::result::Result<Response<wire::ReloadDataPlaneResponse>, Status> {
        let result = async {
            let status = self.plane.reload().await?;
            tracing::info!(
                event = "data_plane_reloaded",
                generation = status.generation
            );
            self.data_plane(status).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(data_plane) => wire::ReloadDataPlaneResponse {
                data_plane: Some(data_plane),
                error: None,
            },
            Err(error) => wire::ReloadDataPlaneResponse {
                data_plane: None,
                error: failure(&error),
            },
        }))
    }

    async fn set_worker_count(
        &self,
        request: Request<wire::SetWorkerCountRequest>,
    ) -> std::result::Result<Response<wire::SetWorkerCountResponse>, Status> {
        let workers = request.into_inner().worker_count;
        let result = async {
            let count = NonZeroUsize::new(workers as usize)
                .filter(|_| workers <= MAX_GATEWAY_WORKERS)
                .ok_or_else(|| {
                    PanelError::invalid_argument(format!(
                        "the worker count must be between 1 and {MAX_GATEWAY_WORKERS}"
                    ))
                })?;
            let status = self.plane.set_workers(count).await?;
            self.settings
                .update(|settings| settings.worker_count = Some(workers))
                .await?;
            tracing::info!(event = "data_plane_workers_changed", workers);
            self.data_plane(status).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(data_plane) => wire::SetWorkerCountResponse {
                data_plane: Some(data_plane),
                error: None,
            },
            Err(error) => wire::SetWorkerCountResponse {
                data_plane: None,
                error: failure(&error),
            },
        }))
    }

    async fn shutdown_gateway(
        &self,
        _request: Request<wire::ShutdownGatewayRequest>,
    ) -> std::result::Result<Response<wire::ShutdownGatewayResponse>, Status> {
        tracing::warn!(event = "gateway_shutdown_requested");
        self.shutdown.cancel();
        Ok(Response::new(wire::ShutdownGatewayResponse {
            accepted: true,
            error: None,
        }))
    }

    async fn list_upstream_health(
        &self,
        _request: Request<wire::ListUpstreamHealthRequest>,
    ) -> std::result::Result<Response<wire::ListUpstreamHealthResponse>, Status> {
        let result = self.engine.status().await;
        Ok(Response::new(match result {
            Ok(engine) => wire::ListUpstreamHealthResponse {
                upstreams: self
                    .adapter
                    .upstream_health()
                    .iter()
                    .map(encode_pool)
                    .collect(),
                observed_at: Some(timestamp(SystemTime::now())),
                active_revision_id: engine
                    .active_revision_id
                    .map_or(0, |revision| revision.get()),
                active_hash: engine.active_hash.as_ref().map(encode_hash),
                error: None,
            },
            Err(error) => wire::ListUpstreamHealthResponse {
                error: failure(&error),
                ..wire::ListUpstreamHealthResponse::default()
            },
        }))
    }

    async fn check_files(
        &self,
        _request: Request<wire::CheckFilesRequest>,
    ) -> std::result::Result<Response<wire::CheckFilesResponse>, Status> {
        let adapter = Arc::clone(&self.adapter);
        let checked = tokio::task::spawn_blocking(move || adapter.check_files())
            .await
            .map_err(|error| PanelError::internal(format!("the file check stopped: {error}")));
        Ok(Response::new(match checked {
            Ok(checks) => wire::CheckFilesResponse {
                checks: Some(encode_checks(checks)),
                error: None,
            },
            Err(error) => wire::CheckFilesResponse {
                checks: None,
                error: failure(&error),
            },
        }))
    }

    async fn get_cache_stats(
        &self,
        _request: Request<wire::GetCacheStatsRequest>,
    ) -> std::result::Result<Response<wire::GetCacheStatsResponse>, Status> {
        Ok(Response::new(wire::GetCacheStatsResponse {
            stats: Some(encode_cache(self.adapter.cache_report())),
            error: None,
        }))
    }

    async fn purge_cache(
        &self,
        request: Request<wire::PurgeCacheRequest>,
    ) -> std::result::Result<Response<wire::PurgeCacheResponse>, Status> {
        let purge = match request.into_inner().target {
            Some(wire::purge_cache_request::Target::All(true)) => Ok(CachePurge::All),
            Some(wire::purge_cache_request::Target::Sites(sites)) if !sites.site_ids.is_empty() => {
                Ok(CachePurge::Sites(sites.site_ids))
            }
            Some(wire::purge_cache_request::Target::Urls(urls)) if !urls.urls.is_empty() => {
                Ok(CachePurge::Urls(urls.urls))
            }
            _ => Err(PanelError::invalid_argument(
                "a purge names everything, sites or URLs",
            )),
        };
        let result = purge.and_then(|purge| {
            let keys = self.adapter.purge_cache(&purge)?;
            tracing::info!(event = "proxy_cache_purged", target = ?purge, keys);
            Ok(keys)
        });
        Ok(Response::new(match result {
            Ok(keys) => wire::PurgeCacheResponse {
                keys: u64::try_from(keys).unwrap_or(u64::MAX),
                error: None,
            },
            Err(error) => wire::PurgeCacheResponse {
                keys: 0,
                error: failure(&error),
            },
        }))
    }

    async fn set_endpoint_drained(
        &self,
        request: Request<wire::SetEndpointDrainedRequest>,
    ) -> std::result::Result<Response<wire::SetEndpointDrainedResponse>, Status> {
        let request = request.into_inner();
        let result = async {
            self.adapter.set_endpoint_drained(
                &request.upstream_id,
                &request.endpoint_id,
                request.drained,
            )?;
            let drained = self.adapter.drained_endpoints();
            self.settings
                .update(|settings| settings.drained = drained)
                .await?;
            tracing::info!(
                event = "upstream_endpoint_drain_changed",
                upstream = %request.upstream_id,
                endpoint = %request.endpoint_id,
                drained = request.drained
            );
            self.adapter
                .upstream_health()
                .into_iter()
                .find(|pool| pool.pool_id == request.upstream_id)
                .map(|pool| encode_pool(&pool))
                .ok_or_else(|| PanelError::not_found("the upstream is no longer active"))
        }
        .await;
        Ok(Response::new(match result {
            Ok(upstream) => wire::SetEndpointDrainedResponse {
                upstream: Some(upstream),
                error: None,
            },
            Err(error) => wire::SetEndpointDrainedResponse {
                upstream: None,
                error: failure(&error),
            },
        }))
    }
}
