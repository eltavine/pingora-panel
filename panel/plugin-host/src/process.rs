//! Plugin processes: started with go-plugin's handshake, bounded, checked
//! and stopped (ADR 0044).

use crate::{
    limits::{self, Limits},
    manifest::{self, HOST_PROTOCOL_VERSIONS},
};
use plugin_contracts::{
    v1::{plugin_client::PluginClient, ConfigureRequest, DescribeRequest, Manifest},
    HEALTH_SERVICE, MAGIC_COOKIE_KEY, MAGIC_COOKIE_VALUE,
};
use plugin_sdk::{DATA_DIR_ENV, LIFELINE_ENV, PROTOCOL_VERSIONS_ENV, SOCKET_DIR_ENV};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStdin, Command},
};
use tonic::transport::Channel;
use tonic_health::pb::{
    health_check_response::ServingStatus, health_client::HealthClient, HealthCheckRequest,
};

/// How long a starting plugin has to shake hands, and each later step.
pub const START_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a stopping plugin has after `SIGTERM`.
pub const STOP_GRACE: Duration = Duration::from_secs(5);
/// Unix sockets' paths hold at most 108 bytes on Linux; the plugin's file
/// name needs room.
const MOST_SOCKET_DIR_BYTES: usize = 80;

/// What starting a plugin version takes.
#[derive(Clone, Debug)]
pub struct Launch {
    pub manifest: Manifest,
    /// The version's directory, which the plugin runs in.
    pub directory: PathBuf,
    pub executable: PathBuf,
    /// The plugin's own directory.
    pub data: PathBuf,
    /// Where the socket directories of running plugins go.
    pub runtime: PathBuf,
    pub limits: Limits,
    /// The settings as JSON, secret references resolved.
    pub settings: String,
    pub granted: Vec<String>,
}

/// A plugin process the host started and talks to.
#[derive(Debug)]
pub struct Process {
    child: Child,
    pub channel: Channel,
    pub protocol_version: u32,
    socket_dir: PathBuf,
    /// The plugin's standard input, held open while it runs: the plugin
    /// stops once it closes, as it does when the host is killed.
    _lifeline: Option<ChildStdin>,
}

static STARTED: AtomicU64 = AtomicU64::new(0);

impl Process {
    /// Starts the version, shakes hands, checks that it is the version it
    /// should be and applies its settings; on any failure the process is
    /// gone again.
    pub async fn start(launch: &Launch) -> Result<Self, String> {
        let socket_dir = launch.runtime.join(format!(
            "{}.{}",
            std::process::id(),
            STARTED.fetch_add(1, Ordering::Relaxed)
        ));
        if socket_dir.as_os_str().len() > MOST_SOCKET_DIR_BYTES {
            return Err(format!(
                "the runtime directory {} is too long for Unix sockets",
                launch.runtime.display()
            ));
        }
        private_directory(&socket_dir)?;
        std::fs::create_dir_all(&launch.data).map_err(|error| {
            format!(
                "the plugin's directory {} cannot be made: {error}",
                launch.data.display()
            )
        })?;
        let mut child = match spawn(launch, &socket_dir) {
            Ok(child) => child,
            Err(problem) => {
                let _ = std::fs::remove_dir_all(&socket_dir);
                return Err(problem);
            }
        };
        let lifeline = child.stdin.take();
        match Self::shake_hands(&mut child, launch, &socket_dir).await {
            Ok((protocol_version, channel)) => Ok(Self {
                child,
                channel,
                protocol_version,
                socket_dir,
                _lifeline: lifeline,
            }),
            Err(problem) => {
                let _ = child.kill().await;
                let _ = std::fs::remove_dir_all(&socket_dir);
                Err(problem)
            }
        }
    }

    async fn shake_hands(
        child: &mut Child,
        launch: &Launch,
        socket_dir: &Path,
    ) -> Result<(u32, Channel), String> {
        let name = launch.manifest.name.clone();
        if let Some(stderr) = child.stderr.take() {
            let name = name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::info!(target: "plugin", plugin = %name, "{line}");
                }
            });
        }
        let stdout = child
            .stdout
            .take()
            .ok_or("the plugin has no standard output")?;
        let mut lines = BufReader::new(stdout).lines();
        let line = match tokio::time::timeout(START_TIMEOUT, lines.next_line()).await {
            Err(_) => return Err("the plugin did not shake hands in time".into()),
            Ok(Ok(Some(line))) => line,
            Ok(_) => return Err("the plugin exited before it shook hands".into()),
        };
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(target: "plugin", plugin = %name, "{line}");
            }
        });
        let (version, socket) = handshake(&line, socket_dir)?;
        let channel = tokio::time::timeout(START_TIMEOUT, plugin_sdk::connect(socket))
            .await
            .map_err(|_| "the plugin's socket did not answer in time".to_owned())?
            .map_err(|error| format!("the plugin's socket cannot be reached: {error}"))?;
        check(&channel, START_TIMEOUT).await?;
        let described = tokio::time::timeout(
            START_TIMEOUT,
            PluginClient::new(channel.clone()).describe(DescribeRequest {}),
        )
        .await
        .map_err(|_| "the plugin did not describe itself in time".to_owned())?
        .map_err(|status| format!("the plugin did not describe itself: {}", status.message()))?
        .into_inner()
        .manifest
        .ok_or("the plugin described itself without a manifest")?;
        manifest::describes(&launch.manifest, &described)?;
        configure(&channel, &launch.settings, &launch.granted, START_TIMEOUT).await?;
        Ok((version, channel))
    }

    /// Resolves when the process exits.
    pub async fn exited(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    /// Stops the process: `SIGTERM`, then `SIGKILL` after [`STOP_GRACE`].
    pub async fn stop(mut self) {
        #[cfg(unix)]
        if let Some(pid) = self
            .child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        if tokio::time::timeout(STOP_GRACE, self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
        let _ = std::fs::remove_dir_all(&self.socket_dir);
    }
}

fn spawn(launch: &Launch, socket_dir: &Path) -> Result<Child, String> {
    let versions: Vec<String> = HOST_PROTOCOL_VERSIONS.iter().map(u32::to_string).collect();
    let mut child = Command::new(&launch.executable)
        .current_dir(&launch.directory)
        .env_clear()
        .env(MAGIC_COOKIE_KEY, MAGIC_COOKIE_VALUE)
        .env(PROTOCOL_VERSIONS_ENV, versions.join(","))
        .env(SOCKET_DIR_ENV, socket_dir)
        .env(DATA_DIR_ENV, &launch.data)
        .env("HOME", &launch.data)
        .env("TMPDIR", socket_dir)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env(LIFELINE_ENV, "stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("the plugin cannot be started: {error}"))?;
    let Some(pid) = child.id() else {
        return Err("the plugin exited at once".into());
    };
    if let Err(error) = limits::apply(pid, &launch.limits) {
        let _ = child.start_kill();
        return Err(format!(
            "the plugin's resource limits cannot be set: {error}"
        ));
    }
    Ok(child)
}

fn private_directory(path: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|error| {
        format!(
            "the plugin's socket directory {} cannot be made: {error}",
            path.display()
        )
    })
}

/// The protocol version and socket of a handshake line,
/// `1|<version>|unix|<socket>|grpc`, when the host accepts them.
pub fn handshake(line: &str, socket_dir: &Path) -> Result<(u32, PathBuf), String> {
    let fields: Vec<&str> = line.trim_end().split('|').collect();
    let [core, version, network, address, protocol, ..] = fields.as_slice() else {
        return Err(format!("{line:?} is not a go-plugin handshake"));
    };
    if *core != "1" {
        return Err(format!(
            "the plugin speaks go-plugin's core protocol {core}, not 1"
        ));
    }
    let version: u32 = version
        .parse()
        .ok()
        .filter(|version| HOST_PROTOCOL_VERSIONS.contains(version))
        .ok_or_else(|| {
            format!("the plugin chose protocol version {version}, which the host does not speak")
        })?;
    if *network != "unix" {
        return Err(format!(
            "the plugin listens on {network}; the host accepts only Unix sockets"
        ));
    }
    if *protocol != "grpc" {
        return Err(format!("the plugin speaks {protocol}, not grpc"));
    }
    let socket = PathBuf::from(address);
    if socket.parent() != Some(socket_dir) {
        return Err("the plugin's socket is outside the directory the host gave it".into());
    }
    Ok((version, socket))
}

/// Whether the plugin reports itself serving within `timeout`.
pub async fn check(channel: &Channel, timeout: Duration) -> Result<(), String> {
    let response = tokio::time::timeout(
        timeout,
        HealthClient::new(channel.clone()).check(HealthCheckRequest {
            service: HEALTH_SERVICE.into(),
        }),
    )
    .await
    .map_err(|_| "the plugin did not answer its health check in time".to_owned())?
    .map_err(|status| format!("the plugin's health check failed: {}", status.message()))?;
    if response.into_inner().status == ServingStatus::Serving as i32 {
        Ok(())
    } else {
        Err("the plugin reports that it is not serving".into())
    }
}

/// Applies settings to the running plugin; a refusal leaves those before.
pub async fn configure(
    channel: &Channel,
    settings: &str,
    granted: &[String],
    timeout: Duration,
) -> Result<(), String> {
    tokio::time::timeout(
        timeout,
        PluginClient::new(channel.clone()).configure(ConfigureRequest {
            settings: settings.to_owned(),
            granted: granted.to_vec(),
        }),
    )
    .await
    .map_err(|_| "the plugin did not apply its settings in time".to_owned())?
    .map_err(|status| format!("the plugin refused its settings: {}", status.message()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshakes_name_a_unix_socket_in_the_given_directory() {
        let directory = Path::new("/run/plugins/1.0");
        assert_eq!(
            handshake("1|1|unix|/run/plugins/1.0/plugin-7.sock|grpc\n", directory).unwrap(),
            (1, directory.join("plugin-7.sock"))
        );
        assert_eq!(
            handshake("1|1|unix|/run/plugins/1.0/p.sock|grpc|MIIB|true", directory)
                .unwrap()
                .0,
            1
        );
        for (line, problem) in [
            ("hello", "not a go-plugin handshake"),
            ("2|1|unix|/run/plugins/1.0/p.sock|grpc", "core protocol 2"),
            (
                "1|9|unix|/run/plugins/1.0/p.sock|grpc",
                "protocol version 9",
            ),
            ("1|1|tcp|127.0.0.1:1234|grpc", "only Unix sockets"),
            ("1|1|unix|/run/plugins/1.0/p.sock|netrpc", "speaks netrpc"),
            ("1|1|unix|/tmp/p.sock|grpc", "outside the directory"),
        ] {
            let found = handshake(line, directory).unwrap_err();
            assert!(found.contains(problem), "{line}: {found}");
        }
    }
}
