//! The gateway's systemd unit, over D-Bus. The agent acts on the one unit
//! its configuration names, and systemd's polkit rules decide whether it
//! may.

// Only Linux serves the capability; elsewhere the client is built for its
// tests alone.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use futures_util::StreamExt;
use panel_contracts::ops::v1::{
    self as wire, gateway_unit_server, AgentCapability, Capability, CapabilityState, UnitAction,
    UnitStatus,
};
use panel_errors::PanelError;
use std::time::{Duration, UNIX_EPOCH};
use tonic::{Request, Response, Status};
use zbus::{proxy::CacheProperties, zvariant::OwnedObjectPath, Connection};
use zbus_systemd::systemd1::{ManagerProxy, ServiceProxy, UnitProxy};

/// How long systemd may take to finish a start, stop or restart.
const JOB_TIMEOUT: Duration = Duration::from_secs(120);

/// The drop-in that enables the capability with its polkit rule.
const ENABLE: &str = "install gateway-unit.conf and the polkit rule beside it";

/// D-Bus errors that mean polkit refused the agent.
const REFUSED: [&str; 2] = [
    "org.freedesktop.DBus.Error.AccessDenied",
    "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired",
];

pub(crate) struct GatewayUnit {
    connection: Connection,
    name: String,
}

impl GatewayUnit {
    pub(crate) fn new(connection: Connection, name: String) -> Self {
        Self { connection, name }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) async fn status(&self) -> Result<UnitStatus, PanelError> {
        let manager = self.manager().await?;
        let path = manager
            .load_unit(self.name.clone())
            .await
            .map_err(|error| self.failure("load", &error))?;
        self.read(path).await
    }

    pub(crate) async fn change(&self, action: UnitAction) -> Result<UnitStatus, PanelError> {
        let verb = match action {
            UnitAction::Start => "start",
            UnitAction::Stop => "stop",
            UnitAction::Restart => "restart",
            UnitAction::Unspecified => {
                return Err(PanelError::invalid_argument("name an action"));
            }
        };
        let manager = self.manager().await?;
        // systemd announces jobs only to subscribed clients; subscribing
        // twice is refused and harmless.
        let _ = manager.subscribe().await;
        let mut removed = manager
            .receive_job_removed()
            .await
            .map_err(|error| self.failure(verb, &error))?;
        let name = self.name.clone();
        let mode = "replace".to_owned();
        let job = match action {
            UnitAction::Start => manager.start_unit(name, mode).await,
            UnitAction::Stop => manager.stop_unit(name, mode).await,
            _ => manager.restart_unit(name, mode).await,
        }
        .map_err(|error| self.failure(verb, &error))?;
        let ended = tokio::time::timeout(JOB_TIMEOUT, async {
            while let Some(signal) = removed.next().await {
                if let Ok(args) = signal.args() {
                    if *args.job() == job {
                        return Some(args.result().clone());
                    }
                }
            }
            None
        })
        .await;
        match ended {
            Ok(Some(result)) if result == "done" => {
                tracing::info!(event = "gateway_unit_changed", unit = %self.name, action = verb);
                self.status().await
            }
            Ok(Some(result)) => Err(PanelError::precondition_failed(format!(
                "systemd could not {verb} {}: its job ended as {result}",
                self.name
            ))),
            Ok(None) => Err(PanelError::unavailable("the system bus closed")),
            Err(_) => Err(PanelError::unavailable(format!(
                "systemd did not finish {verb}ing {} in {} seconds",
                self.name,
                JOB_TIMEOUT.as_secs()
            ))),
        }
    }

    async fn manager(&self) -> Result<ManagerProxy<'_>, PanelError> {
        ManagerProxy::new(&self.connection)
            .await
            .map_err(|error| PanelError::unavailable(format!("the system bus: {error}")))
    }

    async fn read(&self, path: OwnedObjectPath) -> Result<UnitStatus, PanelError> {
        let unit = UnitProxy::builder(&self.connection)
            .path(path.clone())
            .map(|builder| builder.cache_properties(CacheProperties::No))
            .map_err(|error| self.failure("read", &error))?
            .build()
            .await
            .map_err(|error| self.failure("read", &error))?;
        let service = ServiceProxy::builder(&self.connection)
            .path(path)
            .map(|builder| builder.cache_properties(CacheProperties::No))
            .map_err(|error| self.failure("read", &error))?
            .build()
            .await
            .map_err(|error| self.failure("read", &error))?;
        let read = |error: zbus::Error| self.failure("read", &error);
        let since = unit.active_enter_timestamp().await.map_err(read)?;
        Ok(UnitStatus {
            name: unit.id().await.map_err(read)?,
            description: unit.description().await.map_err(read)?,
            load_state: unit.load_state().await.map_err(read)?,
            active_state: unit.active_state().await.map_err(read)?,
            sub_state: unit.sub_state().await.map_err(read)?,
            unit_file_state: unit.unit_file_state().await.unwrap_or_default(),
            main_pid: service.main_pid().await.unwrap_or_default(),
            active_since: (since > 0).then(|| (UNIX_EPOCH + Duration::from_micros(since)).into()),
            restarts: service.n_restarts().await.unwrap_or_default(),
            result: service.result().await.unwrap_or_default(),
        })
    }

    fn failure(&self, verb: &str, error: &zbus::Error) -> PanelError {
        match error {
            zbus::Error::MethodError(name, _, _) if REFUSED.contains(&name.as_str()) => {
                PanelError::precondition_failed(format!(
                    "the agent may not {verb} {}; {ENABLE}",
                    self.name
                ))
            }
            zbus::Error::MethodError(name, _, _)
                if name.as_str() == "org.freedesktop.systemd1.NoSuchUnit" =>
            {
                PanelError::precondition_failed(format!("{} is not installed", self.name))
            }
            error => PanelError::unavailable(format!("systemd: {error}")),
        }
    }
}

fn capability(state: CapabilityState, detail: impl Into<String>) -> AgentCapability {
    AgentCapability {
        capability: Capability::GatewayUnit.into(),
        state: state.into(),
        detail: detail.into(),
    }
}

/// The configured unit on the system bus, when it is there to act on, and
/// the capability's state either way.
pub(crate) async fn open(name: Option<String>) -> (Option<GatewayUnit>, AgentCapability) {
    if !cfg!(target_os = "linux") {
        return (
            None,
            capability(
                CapabilityState::Unsupported,
                "the gateway's unit is a systemd unit, which only Linux has",
            ),
        );
    }
    let Some(name) = name else {
        return (None, capability(CapabilityState::NotEnabled, ENABLE));
    };
    match Connection::system().await {
        Ok(connection) => {
            let unit = GatewayUnit::new(connection, name);
            match reachable(&unit).await {
                Ok(()) => (Some(unit), capability(CapabilityState::Available, "")),
                Err(detail) => (None, capability(CapabilityState::Unreachable, detail)),
            }
        }
        Err(error) => (
            None,
            capability(
                CapabilityState::Unreachable,
                format!("the system bus does not answer: {error}"),
            ),
        ),
    }
}

/// Whether systemd has the unit loaded.
pub(crate) async fn reachable(unit: &GatewayUnit) -> Result<(), String> {
    match unit.status().await {
        Ok(status) if status.load_state == "loaded" => Ok(()),
        Ok(status) => Err(format!("{} is {}", unit.name(), status.load_state)),
        Err(error) => Err(error.message),
    }
}

/// The gateway's unit to panel-api.
pub(crate) struct GatewayUnitService {
    unit: GatewayUnit,
}

impl GatewayUnitService {
    pub(crate) fn new(unit: GatewayUnit) -> Self {
        Self { unit }
    }
}

#[tonic::async_trait]
impl gateway_unit_server::GatewayUnit for GatewayUnitService {
    async fn status(
        &self,
        _: Request<wire::GatewayUnitStatusRequest>,
    ) -> Result<Response<wire::GatewayUnitStatusResponse>, Status> {
        let (status, error) = match self.unit.status().await {
            Ok(status) => (Some(status), None),
            Err(error) => (None, Some((&error).into())),
        };
        Ok(Response::new(wire::GatewayUnitStatusResponse {
            status,
            error,
        }))
    }

    async fn change(
        &self,
        request: Request<wire::GatewayUnitChangeRequest>,
    ) -> Result<Response<wire::GatewayUnitChangeResponse>, Status> {
        let action = request.into_inner().action();
        let (status, error) = match self.unit.change(action).await {
            Ok(status) => (Some(status), None),
            Err(error) => {
                tracing::warn!(
                    event = "gateway_unit_refused",
                    unit = %self.unit.name(),
                    action = action.as_str_name(),
                    error = %error.message,
                );
                (None, Some((&error).into()))
            }
        };
        Ok(Response::new(wire::GatewayUnitChangeResponse {
            status,
            error,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        sync::{Arc, Mutex},
    };
    use zbus::{fdo, interface, object_server::SignalEmitter, zvariant::ObjectPath};

    const UNIT_PATH: &str = "/org/freedesktop/systemd1/unit/gateway_2eservice";

    #[derive(Clone, Default)]
    struct State(Arc<Mutex<Inner>>);

    #[derive(Default)]
    struct Inner {
        stopped: bool,
        restarts: u32,
        jobs: u32,
    }

    impl State {
        fn active(&self) -> bool {
            !self.0.lock().unwrap().stopped
        }
    }

    /// Enough of systemd's manager to start, stop and restart one unit,
    /// refusing `denied.service` and failing `broken.service`.
    struct Manager(State);

    impl Manager {
        async fn job(
            &self,
            name: String,
            stopped: Option<bool>,
            emitter: SignalEmitter<'_>,
        ) -> fdo::Result<OwnedObjectPath> {
            if name == "denied.service" {
                return Err(fdo::Error::AccessDenied("polkit said no".into()));
            }
            let id = {
                let mut state = self.0 .0.lock().unwrap();
                state.jobs += 1;
                match stopped {
                    Some(stopped) => state.stopped = stopped,
                    None => state.restarts += 1,
                }
                state.jobs
            };
            let path =
                OwnedObjectPath::try_from(format!("/org/freedesktop/systemd1/job/{id}")).unwrap();
            let result = if name == "broken.service" {
                "failed"
            } else {
                "done"
            };
            Self::job_removed(&emitter, id, path.as_ref(), &name, result).await?;
            Ok(path)
        }
    }

    #[interface(name = "org.freedesktop.systemd1.Manager")]
    impl Manager {
        async fn subscribe(&self) {}

        async fn load_unit(&self, _name: String) -> OwnedObjectPath {
            OwnedObjectPath::try_from(UNIT_PATH).unwrap()
        }

        async fn start_unit(
            &self,
            name: String,
            _mode: String,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        ) -> fdo::Result<OwnedObjectPath> {
            self.job(name, Some(false), emitter).await
        }

        async fn stop_unit(
            &self,
            name: String,
            _mode: String,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        ) -> fdo::Result<OwnedObjectPath> {
            self.job(name, Some(true), emitter).await
        }

        async fn restart_unit(
            &self,
            name: String,
            _mode: String,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        ) -> fdo::Result<OwnedObjectPath> {
            self.job(name, None, emitter).await
        }

        #[zbus(signal)]
        async fn job_removed(
            emitter: &SignalEmitter<'_>,
            id: u32,
            job: ObjectPath<'_>,
            unit: &str,
            result: &str,
        ) -> zbus::Result<()>;
    }

    struct Unit(State);

    #[interface(name = "org.freedesktop.systemd1.Unit")]
    impl Unit {
        #[zbus(property)]
        fn id(&self) -> String {
            "gateway.service".into()
        }

        #[zbus(property)]
        fn description(&self) -> String {
            "Pingora Panel gateway".into()
        }

        #[zbus(property)]
        fn load_state(&self) -> String {
            "loaded".into()
        }

        #[zbus(property)]
        fn active_state(&self) -> String {
            if self.0.active() {
                "active"
            } else {
                "inactive"
            }
            .into()
        }

        #[zbus(property)]
        fn sub_state(&self) -> String {
            if self.0.active() { "running" } else { "dead" }.into()
        }

        #[zbus(property)]
        fn unit_file_state(&self) -> String {
            "enabled".into()
        }

        #[zbus(property)]
        fn active_enter_timestamp(&self) -> u64 {
            1_800_000_000_000_000
        }
    }

    struct Service(State);

    #[interface(name = "org.freedesktop.systemd1.Service")]
    impl Service {
        #[zbus(property, name = "MainPID")]
        fn main_pid(&self) -> u32 {
            if self.0.active() {
                4242
            } else {
                0
            }
        }

        #[zbus(property, name = "NRestarts")]
        fn n_restarts(&self) -> u32 {
            self.0 .0.lock().unwrap().restarts
        }

        #[zbus(property)]
        fn result(&self) -> String {
            "success".into()
        }
    }

    /// A unit on a peer-to-peer connection to a fake systemd, with that
    /// connection kept open.
    async fn unit(name: &str) -> (GatewayUnit, Connection) {
        let state = State::default();
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let server = zbus::connection::Builder::unix_stream(server)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at("/org/freedesktop/systemd1", Manager(state.clone()))
            .unwrap()
            .serve_at(UNIT_PATH, Unit(state.clone()))
            .unwrap()
            .serve_at(UNIT_PATH, Service(state))
            .unwrap()
            .build();
        let client = zbus::connection::Builder::unix_stream(client).p2p().build();
        let (server, client) = tokio::try_join!(server, client).unwrap();
        (GatewayUnit::new(client, name.into()), server)
    }

    async fn soon<T>(future: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), future)
            .await
            .expect("the fake systemd answers at once")
    }

    #[tokio::test]
    async fn the_unit_is_read_from_its_properties() {
        let (unit, _systemd) = unit("gateway.service").await;
        assert_eq!(soon(reachable(&unit)).await, Ok(()));
        let status = soon(unit.status()).await.unwrap();
        assert_eq!(status.name, "gateway.service");
        assert_eq!(
            (status.active_state.as_str(), status.sub_state.as_str()),
            ("active", "running")
        );
        assert_eq!(status.unit_file_state, "enabled");
        assert_eq!(status.main_pid, 4242);
        assert_eq!(status.active_since.unwrap().seconds, 1_800_000_000);
    }

    #[tokio::test]
    async fn actions_finish_before_the_unit_is_read_again() {
        let (unit, _systemd) = unit("gateway.service").await;
        let stopped = soon(unit.change(UnitAction::Stop)).await.unwrap();
        assert_eq!(
            (stopped.active_state.as_str(), stopped.main_pid),
            ("inactive", 0)
        );
        let started = soon(unit.change(UnitAction::Start)).await.unwrap();
        assert_eq!(started.active_state, "active");
        let restarted = soon(unit.change(UnitAction::Restart)).await.unwrap();
        assert_eq!(restarted.restarts, 1);
        assert!(soon(unit.change(UnitAction::Unspecified)).await.is_err());
    }

    #[tokio::test]
    async fn failed_jobs_and_refusals_say_what_happened() {
        let (broken, _systemd) = unit("broken.service").await;
        let failed = soon(broken.change(UnitAction::Restart)).await.unwrap_err();
        assert_eq!(
            failed.code.as_str(),
            panel_errors::ErrorCode::PRECONDITION_FAILED
        );
        assert!(failed.message.contains("ended as failed"), "{failed}");

        let (denied, _systemd) = unit("denied.service").await;
        let refused = soon(denied.change(UnitAction::Stop)).await.unwrap_err();
        assert_eq!(
            refused.code.as_str(),
            panel_errors::ErrorCode::PRECONDITION_FAILED
        );
        assert!(refused.message.contains("may not stop"), "{refused}");
    }
}
