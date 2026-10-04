//! What runs the gateway: its container in the installation's Compose
//! project, found by the project's and the service's labels on an enabled
//! engine. The agent starts, stops and restarts that one container.

use crate::containers::{self, Engines, ACTION_TIMEOUT, COMPOSE_PROJECT, COMPOSE_SERVICE};
use bollard::{
    models::{ContainerSummary, HealthStatusEnum},
    query_parameters::{
        InspectContainerOptions, ListContainersOptionsBuilder, RestartContainerOptions,
        StartContainerOptions, StopContainerOptions,
    },
    Docker,
};
use chrono::DateTime;
use panel_contracts::ops::v1::{
    self as wire, gateway_service_server, gateway_service_status::Supervisor, AgentCapability,
    Capability, CapabilityState, GatewayServiceAction,
};
use panel_errors::PanelError;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::SystemTime};
use tonic::{Request, Response, Status};

/// Whether the capability is enabled: it needs an engine to reach the
/// gateway's container through.
pub(crate) fn capability(engines: &[(String, PathBuf)]) -> AgentCapability {
    let (state, detail) = if engines.is_empty() {
        (
            CapabilityState::NotEnabled,
            "install containers.conf, which gives the agent the engine that runs the gateway"
                .to_owned(),
        )
    } else {
        (CapabilityState::Available, String::new())
    };
    AgentCapability {
        capability: Capability::GatewayService.into(),
        state: state.into(),
        detail,
    }
}

/// The gateway's container to panel-api.
pub(crate) struct GatewayService {
    engines: Arc<Engines>,
    project: String,
    service: String,
}

/// The gateway's container and the engine it runs on.
struct Found {
    engine: String,
    client: Docker,
    summary: ContainerSummary,
}

impl GatewayService {
    /// The container of `service` in the Compose `project`.
    pub(crate) fn new(engines: Arc<Engines>, project: String, service: String) -> Self {
        Self {
            engines,
            project,
            service,
        }
    }

    /// The first enabled engine with the gateway's container. An engine that
    /// is disabled or does not answer is passed over, and is the reason
    /// given when no other engine has the container.
    async fn find(&self) -> Result<Found, PanelError> {
        let project = format!("{COMPOSE_PROJECT}={}", self.project);
        let service = format!("{COMPOSE_SERVICE}={}", self.service);
        let filters = HashMap::from([("label", vec![project.as_str(), service.as_str()])]);
        let mut passed_over = None;
        for engine in self.engines.ids() {
            let listed = match self.engines.enabled(engine) {
                Ok(client) => {
                    let options = ListContainersOptionsBuilder::default()
                        .all(true)
                        .filters(&filters)
                        .build();
                    client
                        .list_containers(Some(options))
                        .await
                        .map(|found| (client, found))
                        .map_err(|error| containers::failure(&error))
                }
                Err(error) => Err(error),
            };
            match listed {
                Ok((client, found)) => {
                    if let Some(summary) = found.into_iter().next() {
                        return Ok(Found {
                            engine: engine.to_owned(),
                            client,
                            summary,
                        });
                    }
                }
                Err(error) => {
                    passed_over.get_or_insert(error);
                }
            }
        }
        Err(passed_over.unwrap_or_else(|| {
            PanelError::not_found(format!(
                "no container runs the {} service of the {} Compose project",
                self.service, self.project
            ))
        }))
    }

    async fn read(&self) -> Result<wire::GatewayServiceStatus, PanelError> {
        let found = self.find().await?;
        let id = found.summary.id.clone().unwrap_or_default();
        let inspected = found
            .client
            .inspect_container(&id, None::<InspectContainerOptions>)
            .await
            .map_err(|error| containers::failure(&error))?;
        let state = inspected.state.unwrap_or_default();
        Ok(wire::GatewayServiceStatus {
            observed_at: Some(SystemTime::now().into()),
            supervisor: Some(Supervisor::Container(wire::GatewayContainer {
                engine: found.engine,
                container: Some(containers::container(found.summary)),
                started_at: time(state.started_at.as_deref()).map(Into::into),
                finished_at: time(state.finished_at.as_deref()).map(Into::into),
                exit_code: state.exit_code.unwrap_or(0),
                restarts: u32::try_from(inspected.restart_count.unwrap_or(0)).unwrap_or(0),
                health: state
                    .health
                    .and_then(|health| health.status)
                    .map(health)
                    .unwrap_or_default(),
            })),
        })
    }

    async fn apply(
        &self,
        action: GatewayServiceAction,
    ) -> Result<wire::GatewayServiceStatus, PanelError> {
        let verb = match action {
            GatewayServiceAction::Start => "start",
            GatewayServiceAction::Stop => "stop",
            GatewayServiceAction::Restart => "restart",
            GatewayServiceAction::Unspecified => {
                return Err(PanelError::invalid_argument("start, stop or restart"));
            }
        };
        let found = self.find().await?;
        let id = found.summary.id.unwrap_or_default();
        let client = found.client.with_timeout(ACTION_TIMEOUT);
        match action {
            GatewayServiceAction::Start => {
                client
                    .start_container(&id, None::<StartContainerOptions>)
                    .await
            }
            GatewayServiceAction::Stop => {
                client
                    .stop_container(&id, None::<StopContainerOptions>)
                    .await
            }
            _ => {
                client
                    .restart_container(&id, None::<RestartContainerOptions>)
                    .await
            }
        }
        .map_err(|error| containers::failure(&error))?;
        tracing::info!(event = "gateway_service_changed", container = %id, action = verb);
        self.read().await
    }
}

/// An engine's timestamp; it reports the zero time for one that never was.
fn time(value: Option<&str>) -> Option<SystemTime> {
    value
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .filter(|moment| moment.timestamp() > 0)
        .map(SystemTime::from)
}

fn health(status: HealthStatusEnum) -> String {
    match status {
        HealthStatusEnum::STARTING => "starting",
        HealthStatusEnum::HEALTHY => "healthy",
        HealthStatusEnum::UNHEALTHY => "unhealthy",
        _ => "",
    }
    .to_owned()
}

#[tonic::async_trait]
impl gateway_service_server::GatewayService for GatewayService {
    async fn status(
        &self,
        _: Request<wire::GatewayServiceStatusRequest>,
    ) -> Result<Response<wire::GatewayServiceStatusResponse>, Status> {
        let (status, error) = match self.read().await {
            Ok(status) => (Some(status), None),
            Err(error) => (None, Some((&error).into())),
        };
        Ok(Response::new(wire::GatewayServiceStatusResponse {
            status,
            error,
        }))
    }

    async fn change(
        &self,
        request: Request<wire::GatewayServiceChangeRequest>,
    ) -> Result<Response<wire::GatewayServiceChangeResponse>, Status> {
        let action = request.into_inner().action();
        let (status, error) = match self.apply(action).await {
            Ok(status) => (Some(status), None),
            Err(error) => {
                tracing::warn!(
                    event = "gateway_service_refused",
                    service = %self.service,
                    action = action.as_str_name(),
                    error = %error.message,
                );
                (None, Some((&error).into()))
            }
        };
        Ok(Response::new(wire::GatewayServiceChangeResponse {
            status,
            error,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Path as Segments, Query},
        http::StatusCode,
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use std::{path::Path, sync::Mutex};

    /// Whether the fake engine's gateway runs, and what it was asked to do.
    #[derive(Clone, Default)]
    struct Engine {
        running: Arc<Mutex<bool>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    const GATEWAY: &str = "g7";

    impl Engine {
        fn summary(&self) -> Value {
            let running = *self.running.lock().unwrap();
            json!({"Id": GATEWAY, "Names": ["/pingora-panel-gatewayd-1"],
                   "Image": "localhost/pingora-panel:dev", "ImageID": "sha256:cc",
                   "Created": 1_800_000_000,
                   "State": if running { "running" } else { "exited" },
                   "Status": if running { "Up 2 minutes" } else { "Exited (0) 1 second ago" },
                   "Ports": [],
                   "Labels": {COMPOSE_PROJECT: "pingora-panel", COMPOSE_SERVICE: "gatewayd"}})
        }

        fn inspected(&self) -> Value {
            let running = *self.running.lock().unwrap();
            json!({"Id": GATEWAY, "Name": "/pingora-panel-gatewayd-1", "RestartCount": 2,
                   "State": {"Status": if running { "running" } else { "exited" },
                             "Running": running, "ExitCode": if running { 0 } else { 137 },
                             "StartedAt": "2027-01-15T08:00:00.5Z",
                             "FinishedAt": if running { "0001-01-01T00:00:00Z" }
                                           else { "2027-01-15T09:00:00Z" },
                             "Health": {"Status": if running { "healthy" } else { "none" }}}})
        }

        /// Serves enough of the Engine API to answer the agent; it has the
        /// gateway only when `has_gateway`.
        async fn serve(&self, directory: &Path, has_gateway: bool) -> PathBuf {
            let socket = directory.join("engine.sock");
            let listener = tokio::net::UnixListener::bind(&socket).unwrap();
            let listing = self.clone();
            let inspecting = self.clone();
            let acting = self.clone();
            let router = Router::new()
                .route("/_ping", get(|| async { "OK" }))
                .route(
                    "/containers/json",
                    get(move |Query(query): Query<HashMap<String, String>>| async move {
                        let labels = query
                            .get("filters")
                            .and_then(|filters| {
                                serde_json::from_str::<HashMap<String, Vec<String>>>(filters).ok()
                            })
                            .and_then(|mut filters| filters.remove("label"))
                            .unwrap_or_default();
                        let wanted = [
                            format!("{COMPOSE_PROJECT}=pingora-panel"),
                            format!("{COMPOSE_SERVICE}=gatewayd"),
                        ];
                        let listed = if has_gateway
                            && labels.len() == 2
                            && wanted.iter().all(|label| labels.contains(label))
                        {
                            vec![listing.summary()]
                        } else {
                            Vec::new()
                        };
                        Json(listed)
                    }),
                )
                .route(
                    "/containers/{reference}/json",
                    get(move |Segments(reference): Segments<String>| async move {
                        if reference == GATEWAY {
                            Json(inspecting.inspected()).into_response()
                        } else {
                            StatusCode::NOT_FOUND.into_response()
                        }
                    }),
                )
                .route(
                    "/containers/{reference}/{action}",
                    post(
                        move |Segments((reference, action)): Segments<(String, String)>| async move {
                            acting.calls.lock().unwrap().push(format!("{action} {reference}"));
                            *acting.running.lock().unwrap() = action != "stop";
                            StatusCode::NO_CONTENT
                        },
                    ),
                );
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            socket
        }
    }

    fn service(socket: PathBuf, state: Option<PathBuf>) -> GatewayService {
        GatewayService::new(
            Arc::new(Engines::load(vec![("docker".into(), socket)], state)),
            "pingora-panel".into(),
            "gatewayd".into(),
        )
    }

    fn container(status: &wire::GatewayServiceStatus) -> &wire::GatewayContainer {
        match status.supervisor.as_ref() {
            Some(Supervisor::Container(container)) => container,
            None => panic!("no supervisor in {status:?}"),
        }
    }

    #[tokio::test]
    async fn the_gateway_container_is_found_by_its_compose_labels() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::default();
        *engine.running.lock().unwrap() = true;
        let socket = engine.serve(directory.path(), true).await;

        let status = service(socket, None).read().await.unwrap();
        assert!(status.observed_at.is_some());
        let gateway = container(&status);
        assert_eq!(gateway.engine, "docker");
        let summary = gateway.container.as_ref().unwrap();
        assert_eq!(summary.id, GATEWAY);
        assert_eq!(summary.names, vec!["pingora-panel-gatewayd-1".to_owned()]);
        assert_eq!(summary.state(), wire::ContainerState::Running);
        assert_eq!(summary.compose_project, "pingora-panel");
        assert_eq!(gateway.health, "healthy");
        assert_eq!(gateway.restarts, 2);
        assert_eq!(
            gateway
                .started_at
                .as_ref()
                .map(|moment| (moment.seconds, moment.nanos)),
            Some((1_800_000_000, 500_000_000))
        );
        assert_eq!(gateway.finished_at, None, "the zero time means never");
    }

    #[tokio::test]
    async fn the_gateway_is_stopped_started_and_restarted() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::default();
        *engine.running.lock().unwrap() = true;
        let socket = engine.serve(directory.path(), true).await;
        let service = service(socket, None);

        let stopped = service.apply(GatewayServiceAction::Stop).await.unwrap();
        let gateway = container(&stopped);
        assert_eq!(
            gateway.container.as_ref().unwrap().state(),
            wire::ContainerState::Exited
        );
        assert_eq!(gateway.exit_code, 137);
        assert_eq!(gateway.health, "", "no health without a running check");
        assert!(gateway.finished_at.is_some());

        let started = service.apply(GatewayServiceAction::Start).await.unwrap();
        assert_eq!(
            container(&started).container.as_ref().unwrap().state(),
            wire::ContainerState::Running
        );
        service.apply(GatewayServiceAction::Restart).await.unwrap();
        assert_eq!(
            *engine.calls.lock().unwrap(),
            vec![
                format!("stop {GATEWAY}"),
                format!("start {GATEWAY}"),
                format!("restart {GATEWAY}")
            ]
        );
        let refused = service
            .apply(GatewayServiceAction::Unspecified)
            .await
            .unwrap_err();
        assert_eq!(refused.code.as_str(), "INVALID_ARGUMENT");
    }

    #[tokio::test]
    async fn an_engine_without_the_gateway_says_which_service_it_looked_for() {
        let directory = tempfile::tempdir().unwrap();
        let socket = Engine::default().serve(directory.path(), false).await;

        let missing = service(socket, None).read().await.unwrap_err();
        assert_eq!(missing.code.as_str(), "NOT_FOUND");
        assert!(
            missing.message.contains("gatewayd") && missing.message.contains("pingora-panel"),
            "{missing}"
        );
    }

    #[tokio::test]
    async fn a_disabled_engine_is_not_used_and_says_so() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::default();
        let socket = engine.serve(directory.path(), true).await;
        let engines = Arc::new(Engines::load(
            vec![("docker".into(), socket)],
            Some(directory.path().to_owned()),
        ));
        engines.set("docker", false).unwrap();
        let service = GatewayService::new(engines, "pingora-panel".into(), "gatewayd".into());

        let refused = service
            .apply(GatewayServiceAction::Restart)
            .await
            .unwrap_err();
        assert_eq!(refused.code.as_str(), "PRECONDITION_FAILED");
        assert!(engine.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn the_capability_follows_the_configured_engines() {
        assert_eq!(capability(&[]).state(), CapabilityState::NotEnabled);
        let configured = capability(&[("docker".into(), "/run/docker.sock".into())]);
        assert_eq!(configured.state(), CapabilityState::Available);
        assert_eq!(configured.capability(), Capability::GatewayService);
    }
}
