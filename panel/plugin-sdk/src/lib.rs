#![forbid(unsafe_code)]

//! Serves a Pingora Panel plugin (ADR 0044): the go-plugin handshake, the
//! gRPC Health Checking Protocol for the service `plugin` and
//! `pingora.panel.plugin.v1.Plugin`, beside the services of the ports the
//! plugin provides. [`connect`] reaches a plugin from its socket, for hosts
//! and tests.

use http::{Request, Response, Uri};
use hyper_util::rt::TokioIo;
use plugin_contracts::{
    v1::{
        plugin_server::{Plugin as PluginService, PluginServer},
        ConfigureRequest, ConfigureResponse, DescribeRequest, DescribeResponse, Manifest,
    },
    HEALTH_SERVICE, MAGIC_COOKIE_KEY, MAGIC_COOKIE_VALUE, PROTOCOL_VERSION,
};
use std::{
    convert::Infallible,
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{
    body::Body,
    server::NamedService,
    service::Routes,
    transport::{Channel, Endpoint, Server},
    Status,
};

/// The variables the host sets, as go-plugin names them.
pub const PROTOCOL_VERSIONS_ENV: &str = "PLUGIN_PROTOCOL_VERSIONS";
pub const SOCKET_DIR_ENV: &str = "PLUGIN_UNIX_SOCKET_DIR";

/// Settings the host applies, their secret references resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub values: serde_json::Value,
    pub granted: Vec<String>,
}

type Configure = dyn Fn(Settings) -> Result<(), Status> + Send + Sync;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("this program is a Pingora Panel plugin; the panel starts it")]
    NotStartedByHost,
    #[error("the host speaks protocol versions {host:?}, and the plugin {plugin:?}")]
    Incompatible { host: Vec<u32>, plugin: Vec<u32> },
    #[error("{PROTOCOL_VERSIONS_ENV} is not a list of versions: {0:?}")]
    Versions(String),
    #[error("cannot read the manifest {}: {reason}", path.display())]
    Manifest { path: PathBuf, reason: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),
}

/// What the host told the plugin when it started it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handshake {
    /// The application protocol versions the host speaks.
    pub versions: Vec<u32>,
    /// Where the plugin creates its socket.
    pub socket_dir: PathBuf,
}

impl Handshake {
    /// Reads the host's variables; `None` without the magic cookie, when the
    /// program was not started by a host.
    pub fn from_env() -> Result<Option<Self>, Error> {
        if std::env::var(MAGIC_COOKIE_KEY).as_deref() != Ok(MAGIC_COOKIE_VALUE) {
            return Ok(None);
        }
        let versions = match std::env::var(PROTOCOL_VERSIONS_ENV) {
            Ok(list) => versions(&list)?,
            Err(_) => vec![PROTOCOL_VERSION],
        };
        let socket_dir = std::env::var_os(SOCKET_DIR_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Ok(Some(Self {
            versions,
            socket_dir,
        }))
    }
}

fn versions(list: &str) -> Result<Vec<u32>, Error> {
    list.split(',')
        .map(|version| version.trim().parse::<u32>())
        .collect::<Result<_, _>>()
        .map_err(|_| Error::Versions(list.to_owned()))
}

/// The newest version both speak.
pub fn negotiate(host: &[u32], plugin: &[u32]) -> Result<u32, Error> {
    let plugin = if plugin.is_empty() {
        &[PROTOCOL_VERSION][..]
    } else {
        plugin
    };
    plugin
        .iter()
        .filter(|version| host.contains(version))
        .max()
        .copied()
        .ok_or_else(|| Error::Incompatible {
            host: host.to_vec(),
            plugin: plugin.to_vec(),
        })
}

/// A plugin: its manifest, what it does with settings and the services of
/// its ports.
pub struct Plugin {
    manifest: Manifest,
    routes: Routes,
    configure: Arc<Configure>,
}

impl Plugin {
    pub fn new(manifest: Manifest) -> Self {
        Self {
            manifest,
            routes: Routes::default(),
            configure: Arc::new(|_| Ok(())),
        }
    }

    /// Reads the manifest from `path`, such as the `plugin.json` of the
    /// version's directory the host starts the plugin in.
    pub fn from_manifest_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let manifest = std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<Manifest>(&bytes).map_err(|error| error.to_string())
            })
            .map_err(|reason| Error::Manifest {
                path: path.to_owned(),
                reason,
            })?;
        Ok(Self::new(manifest))
    }

    /// What the plugin does with the settings the host applies; an error
    /// refuses them, and the host keeps the settings before them.
    pub fn on_configure(
        mut self,
        configure: impl Fn(Settings) -> Result<(), Status> + Send + Sync + 'static,
    ) -> Self {
        self.configure = Arc::new(configure);
        self
    }

    /// Serves a port's gRPC service, such as a `Dns01ProviderServer`.
    pub fn with_service<S>(mut self, service: S) -> Self
    where
        S: tower::Service<Request<Body>, Response = Response<Body>, Error = Infallible>
            + NamedService
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
    {
        self.routes = self.routes.add_service(service);
        self
    }

    /// Serves until the process is told to stop, as the host started it;
    /// [`Error::NotStartedByHost`] when no host did.
    pub async fn serve(self) -> Result<(), Error> {
        let handshake = Handshake::from_env()?.ok_or(Error::NotStartedByHost)?;
        self.serve_with(handshake, tokio::io::stdout(), stopped())
            .await
    }

    /// Serves until `shutdown`, writing the handshake line to `out`.
    pub async fn serve_with(
        self,
        handshake: Handshake,
        mut out: impl AsyncWrite + Unpin,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), Error> {
        let version = negotiate(&handshake.versions, &self.manifest.protocol_versions)?;
        let socket = handshake
            .socket_dir
            .join(format!("plugin-{}.sock", std::process::id()));
        match std::fs::remove_file(&socket) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        let listener = UnixListener::bind(&socket)?;
        let (reporter, health) = tonic_health::server::health_reporter();
        reporter
            .set_service_status(HEALTH_SERVICE, tonic_health::ServingStatus::Serving)
            .await;
        let routes = self
            .routes
            .add_service(health)
            .add_service(PluginServer::new(Described {
                manifest: self.manifest,
                configure: self.configure,
            }));
        out.write_all(format!("1|{version}|unix|{}|grpc\n", socket.display()).as_bytes())
            .await?;
        out.flush().await?;
        let served = Server::builder()
            .add_routes(routes)
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), shutdown)
            .await;
        let _ = std::fs::remove_file(&socket);
        Ok(served?)
    }
}

/// Resolves when the process receives `SIGTERM` or `SIGINT`.
async fn stopped() {
    use tokio::signal::unix::{signal, SignalKind};
    let (Ok(mut terminate), Ok(mut interrupt)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
}

struct Described {
    manifest: Manifest,
    configure: Arc<Configure>,
}

#[tonic::async_trait]
impl PluginService for Described {
    async fn describe(
        &self,
        _: tonic::Request<DescribeRequest>,
    ) -> Result<tonic::Response<DescribeResponse>, Status> {
        Ok(tonic::Response::new(DescribeResponse {
            manifest: Some(self.manifest.clone()),
        }))
    }

    async fn configure(
        &self,
        request: tonic::Request<ConfigureRequest>,
    ) -> Result<tonic::Response<ConfigureResponse>, Status> {
        let request = request.into_inner();
        let values = serde_json::from_str(&request.settings).map_err(|error| {
            Status::invalid_argument(format!("the settings are not JSON: {error}"))
        })?;
        (self.configure)(Settings {
            values,
            granted: request.granted,
        })?;
        Ok(tonic::Response::new(ConfigureResponse {}))
    }
}

/// A channel to the plugin serving on the Unix socket at `path`.
pub async fn connect(path: impl Into<PathBuf>) -> Result<Channel, tonic::transport::Error> {
    let path = path.into();
    // The URI is required and never used: every connection is the socket.
    Endpoint::from_static("http://plugin")
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let path = path.clone();
            async move { UnixStream::connect(path).await.map(TokioIo::new) }
        }))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_contracts::v1::plugin_client::PluginClient;
    use std::sync::Mutex;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tonic_health::pb::{health_client::HealthClient, HealthCheckRequest};

    #[test]
    fn the_newest_version_both_speak_is_used() {
        assert_eq!(negotiate(&[1, 2], &[1, 2, 3]).unwrap(), 2);
        assert_eq!(negotiate(&[1], &[]).unwrap(), 1);
        assert!(matches!(
            negotiate(&[2], &[1]),
            Err(Error::Incompatible { .. })
        ));
        assert_eq!(versions("1, 2").unwrap(), vec![1, 2]);
        assert!(versions("one").is_err());
    }

    #[tokio::test]
    async fn a_plugin_shakes_hands_reports_health_and_takes_settings() {
        let directory = tempfile::tempdir().unwrap();
        let applied = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&applied);
        let plugin = Plugin::new(Manifest {
            name: "example".into(),
            version: "1.0.0".into(),
            protocol_versions: vec![1],
            ports: vec!["dns01".into()],
            ..Manifest::default()
        })
        .on_configure(move |settings| {
            if settings.values["zone"].is_null() {
                return Err(Status::invalid_argument("a zone is required"));
            }
            seen.lock().unwrap().push(settings);
            Ok(())
        });
        let (writer, reader) = tokio::io::duplex(256);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let handshake = Handshake {
            versions: vec![1],
            socket_dir: directory.path().to_owned(),
        };
        let served = tokio::spawn(plugin.serve_with(handshake, writer, async {
            let _ = stopped.await;
        }));

        let mut line = String::new();
        BufReader::new(reader).read_line(&mut line).await.unwrap();
        let fields: Vec<&str> = line.trim_end().split('|').collect();
        assert_eq!(fields[..3], ["1", "1", "unix"]);
        assert_eq!(fields[4], "grpc");
        let channel = connect(fields[3]).await.unwrap();

        let health = HealthClient::new(channel.clone())
            .check(HealthCheckRequest {
                service: HEALTH_SERVICE.into(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(health.status, 1, "SERVING");
        let mut client = PluginClient::new(channel);
        let manifest = client
            .describe(DescribeRequest {})
            .await
            .unwrap()
            .into_inner()
            .manifest
            .unwrap();
        assert_eq!(manifest.name, "example");
        let refused = client
            .configure(ConfigureRequest {
                settings: "{}".into(),
                granted: vec![],
            })
            .await
            .unwrap_err();
        assert_eq!(refused.code(), tonic::Code::InvalidArgument);
        client
            .configure(ConfigureRequest {
                settings: r#"{"zone": "shop.example."}"#.into(),
                granted: vec!["dns01".into()],
            })
            .await
            .unwrap();
        assert_eq!(applied.lock().unwrap()[0].granted, vec!["dns01".to_owned()]);

        stop.send(()).unwrap();
        served.await.unwrap().unwrap();
        assert!(!Path::new(fields[3]).exists(), "the socket is removed");
    }
}
