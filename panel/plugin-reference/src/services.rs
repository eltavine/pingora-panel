//! The reference plugin's ports. What it is given goes to its own
//! directory: TXT records to `records.json`, notifications to
//! `notifications.jsonl` and archives to `archives/`. Its settings can slow
//! its calls (`delay_ms`), refuse themselves (`refuse`) or end the process
//! (`exit_after_ms`), so hosts can be tested against them.

use gateway_grpc::GatewayGrpcService;
use panel_contracts::pingora::panel::ops::v1::{
    self as ops,
    containers_server::{Containers, ContainersServer},
};
use panel_engine::{EngineCapability, FakeGatewayEngine};
use plugin_contracts::v1::{
    backup_target_put_request::Part,
    backup_target_server::{BackupTarget, BackupTargetServer},
    dns01_provider_server::{Dns01Provider, Dns01ProviderServer},
    notification_provider_server::{NotificationProvider, NotificationProviderServer},
    secret_provider_server::{SecretProvider, SecretProviderServer},
    AddTxtRequest, AddTxtResponse, ArchiveInfo, BackupTargetDeleteRequest,
    BackupTargetDeleteResponse, BackupTargetGetRequest, BackupTargetGetResponse,
    BackupTargetListRequest, BackupTargetListResponse, BackupTargetPutRequest,
    BackupTargetPutResponse, NotifyRequest, NotifyResponse, RemoveTxtRequest, RemoveTxtResponse,
    ResolveRequest, ResolveResponse,
};
use plugin_sdk::{Plugin, Settings};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};

/// Capabilities the reference gateway engine claims.
const GATEWAY_CAPABILITIES: [&str; 6] = [
    "activation.cas",
    "action.respond",
    "listener.http",
    "route.exact-path",
    "route.path-prefix",
    "upstream.http",
];

#[derive(Clone, Debug)]
struct FakeContainer {
    id: String,
    name: String,
    image: String,
    running: bool,
}

/// What the plugin keeps while it runs.
pub struct State {
    data: PathBuf,
    settings: RwLock<Value>,
    containers: Mutex<Vec<FakeContainer>>,
    files: tokio::sync::Mutex<()>,
}

impl State {
    pub fn new(data: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            data,
            settings: RwLock::new(json!({})),
            containers: Mutex::new(Vec::new()),
            files: tokio::sync::Mutex::new(()),
        })
    }

    fn configure(&self, settings: Settings) -> Result<(), Status> {
        let values = settings.values;
        if values["refuse"] == true {
            return Err(Status::invalid_argument(
                "the settings ask the plugin to refuse them",
            ));
        }
        let containers = values["containers"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, container)| FakeContainer {
                id: format!("{:064x}", index + 1),
                name: container["name"].as_str().unwrap_or_default().to_owned(),
                image: container["image"].as_str().unwrap_or_default().to_owned(),
                running: container["running"].as_bool().unwrap_or(true),
            })
            .collect();
        *self
            .containers
            .lock()
            .expect("containers are never poisoned") = containers;
        if let Some(after) = values["exit_after_ms"].as_u64() {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(after)).await;
                std::process::exit(3);
            });
        }
        *self.settings.write().expect("settings are never poisoned") = values;
        Ok(())
    }

    fn setting(&self, name: &str) -> Value {
        self.settings.read().expect("settings are never poisoned")[name].clone()
    }

    async fn delay(&self) {
        if let Some(delay) = self.setting("delay_ms").as_u64() {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
    }

    async fn records(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, Vec<String>>),
    ) -> Result<(), Status> {
        let _guard = self.files.lock().await;
        let path = self.data.join("records.json");
        let mut records: BTreeMap<String, Vec<String>> = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(internal)?,
            Err(_) => BTreeMap::new(),
        };
        change(&mut records);
        records.retain(|_, values| !values.is_empty());
        tokio::fs::write(
            &path,
            serde_json::to_vec_pretty(&records).map_err(internal)?,
        )
        .await
        .map_err(internal)
    }

    fn archive(&self, name: &str) -> Result<PathBuf, Status> {
        if name.is_empty() || name.contains('/') || name.starts_with('.') {
            return Err(Status::invalid_argument(format!(
                "{name:?} is not an archive name"
            )));
        }
        Ok(self.data.join("archives").join(name))
    }
}

fn internal(error: impl std::fmt::Display) -> Status {
    Status::internal(error.to_string())
}

/// The plugin with every port's service.
pub fn register(plugin: Plugin, state: Arc<State>) -> Plugin {
    let configured = Arc::clone(&state);
    let engine = GatewayGrpcService::new(Arc::new(FakeGatewayEngine::new(
        GATEWAY_CAPABILITIES.map(|name| EngineCapability::new(name, "1")),
    )));
    let gateway = engine.transport_policy().gateway_server(engine);
    plugin
        .on_configure(move |settings| configured.configure(settings))
        .with_service(Dns01ProviderServer::from_arc(Arc::clone(&state)))
        .with_service(SecretProviderServer::from_arc(Arc::clone(&state)))
        .with_service(NotificationProviderServer::from_arc(Arc::clone(&state)))
        .with_service(BackupTargetServer::from_arc(Arc::clone(&state)))
        .with_service(ContainersServer::from_arc(state))
        .with_service(gateway)
}

#[tonic::async_trait]
impl Dns01Provider for State {
    async fn add_txt(
        &self,
        request: Request<AddTxtRequest>,
    ) -> Result<Response<AddTxtResponse>, Status> {
        self.delay().await;
        let AddTxtRequest { name, value } = request.into_inner();
        self.records(|records| {
            let values = records.entry(name).or_default();
            if !values.contains(&value) {
                values.push(value);
            }
        })
        .await?;
        Ok(Response::new(AddTxtResponse {}))
    }

    async fn remove_txt(
        &self,
        request: Request<RemoveTxtRequest>,
    ) -> Result<Response<RemoveTxtResponse>, Status> {
        self.delay().await;
        let RemoveTxtRequest { name, value } = request.into_inner();
        self.records(|records| {
            if let Some(values) = records.get_mut(&name) {
                values.retain(|kept| *kept != value);
            }
        })
        .await?;
        Ok(Response::new(RemoveTxtResponse {}))
    }
}

#[tonic::async_trait]
impl SecretProvider for State {
    /// `token` is the setting the host resolved and `process-limits` what
    /// the process may use, where the system says; other paths are keys of
    /// the `secrets` setting.
    async fn resolve(
        &self,
        request: Request<ResolveRequest>,
    ) -> Result<Response<ResolveResponse>, Status> {
        let path = request.into_inner().path;
        if path == "process-limits" {
            let limits = tokio::fs::read("/proc/self/limits")
                .await
                .map_err(|_| Status::not_found("the system does not say"))?;
            return Ok(Response::new(ResolveResponse { value: limits }));
        }
        let value = if path == "token" {
            self.setting("token")
        } else {
            self.setting("secrets")[&path].clone()
        };
        let value = value
            .as_str()
            .ok_or_else(|| Status::not_found(format!("there is no secret at {path}")))?;
        Ok(Response::new(ResolveResponse {
            value: value.as_bytes().to_vec(),
        }))
    }
}

#[tonic::async_trait]
impl NotificationProvider for State {
    async fn notify(
        &self,
        request: Request<NotifyRequest>,
    ) -> Result<Response<NotifyResponse>, Status> {
        self.delay().await;
        let request = request.into_inner();
        let alert = request.alert.unwrap_or_default();
        let line = json!({
            "channel": request.channel,
            "status": alert.status,
            "labels": alert.labels,
            "annotations": alert.annotations,
            "starts_at": alert.starts_at,
            "ends_at": alert.ends_at,
        });
        let _guard = self.files.lock().await;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.data.join("notifications.jsonl"))
            .await
            .map_err(internal)?;
        file.write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(internal)?;
        Ok(Response::new(NotifyResponse {}))
    }
}

type Chunks = Pin<Box<dyn Stream<Item = Result<BackupTargetGetResponse, Status>> + Send>>;

#[tonic::async_trait]
impl BackupTarget for State {
    async fn put(
        &self,
        request: Request<Streaming<BackupTargetPutRequest>>,
    ) -> Result<Response<BackupTargetPutResponse>, Status> {
        let mut parts = request.into_inner();
        let Some(Ok(BackupTargetPutRequest {
            part: Some(Part::Archive(info)),
        })) = parts.next().await
        else {
            return Err(Status::invalid_argument(
                "the first message names the archive",
            ));
        };
        let path = self.archive(&info.name)?;
        tokio::fs::create_dir_all(path.parent().expect("archives have a directory"))
            .await
            .map_err(internal)?;
        let partial = path.with_extension("partial");
        let mut file = tokio::fs::File::create(&partial).await.map_err(internal)?;
        while let Some(part) = parts.next().await {
            match part?.part {
                Some(Part::Chunk(chunk)) => file.write_all(&chunk).await.map_err(internal)?,
                _ => {
                    return Err(Status::invalid_argument(
                        "only the first message names the archive",
                    ))
                }
            }
        }
        file.sync_all().await.map_err(internal)?;
        let bytes = tokio::fs::read(&partial).await.map_err(internal)?;
        let digest = {
            use sha2::Digest as _;
            hex::encode(sha2::Sha256::digest(&bytes))
        };
        if (!info.sha256.is_empty() && info.sha256 != digest)
            || (info.size != 0 && info.size != bytes.len() as u64)
        {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(Status::data_loss("the archive arrived damaged"));
        }
        tokio::fs::rename(&partial, &path).await.map_err(internal)?;
        let stored = ArchiveInfo {
            sha256: digest,
            size: bytes.len() as u64,
            ..info
        };
        tokio::fs::write(
            path.with_extension("json"),
            serde_json::to_vec(&json!({
                "name": stored.name, "size": stored.size,
                "sha256": stored.sha256, "created_at": stored.created_at,
            }))
            .map_err(internal)?,
        )
        .await
        .map_err(internal)?;
        Ok(Response::new(BackupTargetPutResponse {}))
    }

    async fn list(
        &self,
        _: Request<BackupTargetListRequest>,
    ) -> Result<Response<BackupTargetListResponse>, Status> {
        let mut archives = Vec::new();
        if let Ok(mut entries) = tokio::fs::read_dir(self.data.join("archives")).await {
            while let Some(entry) = entries.next_entry().await.map_err(internal)? {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    let info: Value =
                        serde_json::from_slice(&tokio::fs::read(&path).await.map_err(internal)?)
                            .map_err(internal)?;
                    archives.push(ArchiveInfo {
                        name: info["name"].as_str().unwrap_or_default().to_owned(),
                        size: info["size"].as_u64().unwrap_or_default(),
                        sha256: info["sha256"].as_str().unwrap_or_default().to_owned(),
                        created_at: info["created_at"].as_str().unwrap_or_default().to_owned(),
                    });
                }
            }
        }
        archives.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(Response::new(BackupTargetListResponse { archives }))
    }

    type GetStream = Chunks;

    async fn get(
        &self,
        request: Request<BackupTargetGetRequest>,
    ) -> Result<Response<Chunks>, Status> {
        let name = request.into_inner().name;
        let bytes = tokio::fs::read(self.archive(&name)?)
            .await
            .map_err(|_| Status::not_found(format!("there is no archive {name}")))?;
        let chunks: Vec<Result<BackupTargetGetResponse, Status>> = bytes
            .chunks(64 * 1024)
            .map(|chunk| {
                Ok(BackupTargetGetResponse {
                    chunk: chunk.to_vec(),
                })
            })
            .collect();
        Ok(Response::new(Box::pin(tokio_stream::iter(chunks))))
    }

    async fn delete(
        &self,
        request: Request<BackupTargetDeleteRequest>,
    ) -> Result<Response<BackupTargetDeleteResponse>, Status> {
        let path = self.archive(&request.into_inner().name)?;
        let _ = tokio::fs::remove_file(&path).await;
        let _ = tokio::fs::remove_file(path.with_extension("json")).await;
        Ok(Response::new(BackupTargetDeleteResponse {}))
    }
}

fn container(container: &FakeContainer) -> ops::Container {
    let state = if container.running {
        ops::ContainerState::Running
    } else {
        ops::ContainerState::Exited
    };
    ops::Container {
        id: container.id.clone(),
        names: vec![container.name.clone()],
        image: container.image.clone(),
        state: state as i32,
        status: if container.running {
            "Up"
        } else {
            "Exited (0)"
        }
        .into(),
        ..ops::Container::default()
    }
}

type Lines = Pin<Box<dyn Stream<Item = Result<ops::ContainersFollowLogsResponse, Status>> + Send>>;

/// The one engine the reference plugin provides.
const ENGINE: &str = "main";

fn unoffered(operation: &str) -> Status {
    Status::unimplemented(format!("the reference engine does not offer {operation}"))
}

#[tonic::async_trait]
impl Containers for State {
    async fn engines(
        &self,
        _: Request<ops::ContainersEnginesRequest>,
    ) -> Result<Response<ops::ContainersEnginesResponse>, Status> {
        let (containers, running) = {
            let kept = self
                .containers
                .lock()
                .expect("containers are never poisoned");
            let running = kept.iter().filter(|container| container.running).count();
            (kept.len(), running)
        };
        let count = |count: usize| u32::try_from(count).unwrap_or(u32::MAX);
        Ok(Response::new(ops::ContainersEnginesResponse {
            engines: vec![ops::Engine {
                id: ENGINE.into(),
                socket: String::new(),
                enabled: true,
                reachable: true,
                detail: String::new(),
                version: Some(ops::EngineVersion {
                    version: env!("CARGO_PKG_VERSION").into(),
                    api_version: "1".into(),
                    os: std::env::consts::OS.into(),
                    architecture: std::env::consts::ARCH.into(),
                    ..ops::EngineVersion::default()
                }),
                info: Some(ops::EngineInfo {
                    containers: count(containers),
                    running: count(running),
                    stopped: count(containers - running),
                    name: "reference".into(),
                    ..ops::EngineInfo::default()
                }),
            }],
            error: None,
        }))
    }

    async fn set_engine(
        &self,
        _: Request<ops::ContainersSetEngineRequest>,
    ) -> Result<Response<ops::ContainersSetEngineResponse>, Status> {
        Err(unoffered("enabling engines"))
    }

    async fn list(
        &self,
        request: Request<ops::ContainersListRequest>,
    ) -> Result<Response<ops::ContainersListResponse>, Status> {
        let search = request.into_inner().search.to_lowercase();
        let containers = self
            .containers
            .lock()
            .expect("containers are never poisoned")
            .iter()
            .filter(|kept| {
                search.is_empty()
                    || kept.name.to_lowercase().contains(&search)
                    || kept.image.to_lowercase().contains(&search)
            })
            .map(container)
            .collect();
        Ok(Response::new(ops::ContainersListResponse {
            observed_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
            containers,
            error: None,
        }))
    }

    async fn act(
        &self,
        request: Request<ops::ContainersActRequest>,
    ) -> Result<Response<ops::ContainersActResponse>, Status> {
        let request = request.into_inner();
        let action = ops::ContainerAction::try_from(request.action)
            .map_err(|_| Status::invalid_argument("not a container action"))?;
        let mut containers = self
            .containers
            .lock()
            .expect("containers are never poisoned");
        let index = containers
            .iter()
            .position(|kept| {
                kept.name == request.container || kept.id.starts_with(&request.container)
            })
            .ok_or_else(|| {
                Status::not_found(format!("there is no container {}", request.container))
            })?;
        let (id, name) = (containers[index].id.clone(), containers[index].name.clone());
        let after = match action {
            ops::ContainerAction::Start | ops::ContainerAction::Restart => {
                containers[index].running = true;
                Some(container(&containers[index]))
            }
            ops::ContainerAction::Stop | ops::ContainerAction::Kill => {
                containers[index].running = false;
                Some(container(&containers[index]))
            }
            ops::ContainerAction::Remove => {
                if containers[index].running && !request.force {
                    return Err(Status::failed_precondition(format!("{name} is running")));
                }
                containers.remove(index);
                None
            }
            ops::ContainerAction::Unspecified => {
                return Err(Status::invalid_argument("name an action"))
            }
        };
        Ok(Response::new(ops::ContainersActResponse {
            id,
            name,
            container: after,
            error: None,
        }))
    }

    async fn inspect(
        &self,
        _: Request<ops::ContainersInspectRequest>,
    ) -> Result<Response<ops::ContainersInspectResponse>, Status> {
        Err(unoffered("inspecting containers"))
    }

    async fn logs(
        &self,
        _: Request<ops::ContainersLogsRequest>,
    ) -> Result<Response<ops::ContainersLogsResponse>, Status> {
        Err(unoffered("logs"))
    }

    type FollowLogsStream = Lines;

    async fn follow_logs(
        &self,
        _: Request<ops::ContainersFollowLogsRequest>,
    ) -> Result<Response<Lines>, Status> {
        Err(unoffered("following logs"))
    }

    async fn stats(
        &self,
        _: Request<ops::ContainersStatsRequest>,
    ) -> Result<Response<ops::ContainersStatsResponse>, Status> {
        Err(unoffered("statistics"))
    }
}
