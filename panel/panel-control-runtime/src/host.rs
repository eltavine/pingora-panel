use crate::{
    ControlPlaneProcess, DefaultAddresses, InProcessHub, ProcessSettings, RunningProcess,
    TlsSettings, TRUST_DOMAIN_ENV,
};
use panel_errors::Result;
use panel_pki::TrustDomain;
use panel_platform::ServiceName;
use panel_service::{init_logging, probe_http, shutdown_signal, Environment, READINESS_PATH};
use std::{future::Future, path::PathBuf, process::ExitCode, time::Duration};

/// Argument that turns the binary into its own container health check.
pub const HEALTHCHECK_ARGUMENT: &str = "healthcheck";
/// Directory with each module's mutual TLS credentials, in a subdirectory
/// named after the module's service.
pub const CREDENTIALS_DIR_ENV: &str = "PINGORA_PANEL_CREDENTIALS_DIR";
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Builds a module's process from the environment and its settings.
pub type BuildModule = fn(&mut Environment<'_>, ProcessSettings) -> Result<ControlPlaneProcess>;

/// A module of the control plane.
#[derive(Clone, Copy)]
pub struct Module {
    service: &'static str,
    defaults: DefaultAddresses,
    build: BuildModule,
}

impl Module {
    /// The module of `service`, whose operational listener binds
    /// `defaults.ops`, built by `build`.
    pub fn new(service: &'static str, defaults: DefaultAddresses, build: BuildModule) -> Self {
        Self {
            service,
            defaults,
            build,
        }
    }
}

/// Modules started in one process; they reach each other in process and
/// other services over the network.
pub struct ControlPlane {
    running: Vec<RunningProcess>,
}

impl ControlPlane {
    /// Builds and starts `modules` in order. Each keeps its own operational
    /// listener and, under [`CREDENTIALS_DIR_ENV`], its own credentials. When
    /// one fails to start, those already started are stopped.
    pub async fn start(env: &mut Environment<'_>, modules: &[Module]) -> Result<Self> {
        let hub = InProcessHub::new(
            modules
                .iter()
                .map(|module| ServiceName::new(module.service))
                .collect::<Result<Vec<_>>>()?,
        );
        let credentials = env.string(CREDENTIALS_DIR_ENV)?.map(PathBuf::from);
        let trust_domain = env
            .string(TRUST_DOMAIN_ENV)?
            .map(TrustDomain::new)
            .transpose()?
            .unwrap_or_default();
        let mut control_plane = Self {
            running: Vec::with_capacity(modules.len()),
        };
        for module in modules {
            let started = async {
                let settings = ProcessSettings::read(env, module.defaults)?
                    .with_listeners(module.defaults.ops, module.defaults.grpc)
                    .with_tls(credentials.as_ref().map(|directory| TlsSettings {
                        directory: directory.join(module.service),
                        trust_domain: trust_domain.clone(),
                    }));
                (module.build)(env, settings)?
                    .in_process(hub.clone())
                    .start()
                    .await
            }
            .await;
            match started {
                Ok(process) => control_plane.running.push(process),
                Err(error) => {
                    control_plane.stop().await;
                    return Err(error);
                }
            }
        }
        Ok(control_plane)
    }

    pub fn modules(&self) -> &[RunningProcess] {
        &self.running
    }

    pub async fn run_until(self, shutdown: impl Future<Output = ()>) {
        shutdown.await;
        self.stop().await;
    }

    /// Stops the modules in the reverse of the order they started in.
    pub async fn stop(self) {
        for process in self.running.into_iter().rev() {
            process.stop().await;
        }
    }
}

/// The entry point of the control-plane binary.
///
/// Started with `healthcheck`, it probes the readiness endpoint of every
/// module and exits accordingly. Otherwise it starts the modules, runs them
/// until SIGINT or SIGTERM and stops them gracefully.
pub fn control_plane_main(modules: &[Module]) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    if std::env::args().nth(1).as_deref() == Some(HEALTHCHECK_ARGUMENT) {
        let ready = runtime.block_on(async {
            for module in modules {
                if !probe_http(module.defaults.ops, READINESS_PATH, PROBE_TIMEOUT).await {
                    return false;
                }
            }
            true
        });
        return if ready {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    init_logging();
    let result = runtime.block_on(async {
        let control_plane = ControlPlane::start(&mut Environment::process(), modules).await?;
        control_plane.run_until(shutdown_signal()).await;
        Ok::<_, panel_errors::PanelError>(())
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error_code = %error.code, error = %error.message, "control plane failed to start");
            ExitCode::FAILURE
        }
    }
}
