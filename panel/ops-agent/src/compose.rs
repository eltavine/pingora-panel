//! The Compose projects on the engines the agent reaches (ADR 0031), known
//! by the labels Compose puts on their containers. Bringing a project up
//! starts its containers, down stops and removes them with its networks
//! and keeps its volumes, and restart restarts them; recreating a project
//! from a changed file stays with Compose.

use crate::{
    container_logs,
    containers::{answer, failure, Engines, ACTION_TIMEOUT, COMPOSE_PROJECT, COMPOSE_SERVICE},
};
use bollard::{
    models::{ContainerSummary, ContainerSummaryStateEnum},
    query_parameters::{
        ListContainersOptionsBuilder, ListNetworksOptionsBuilder, RemoveContainerOptionsBuilder,
        RestartContainerOptions, StartContainerOptions, StopContainerOptions,
    },
    Docker,
};
use futures_util::{stream, StreamExt};
use panel_contracts::ops::v1::{
    self as wire, compose_projects_server::ComposeProjects, ComposeAction,
};
use panel_errors::PanelError;
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tonic::{Request, Response, Status};

/// The labels Compose puts on a project's containers.
const WORKING_DIRECTORY: &str = "com.docker.compose.project.working_dir";
const CONFIG_FILES: &str = "com.docker.compose.project.config_files";
/// The largest Compose file the agent reads.
const MOST_FILE_BYTES: u64 = 256 * 1024;
/// How many containers an action changes at once.
const CONCURRENT: usize = 8;
/// The most lines a project's logs return, and how many by default.
const MOST_LINES: u32 = 5_000;
const DEFAULT_LINES: u32 = 200;
/// The most text a project's logs return.
const READ_BYTES: usize = 2 * 1024 * 1024;

/// A project's name as Compose allows it: lowercase letters, digits,
/// dashes and underscores, starting with a letter or digit.
fn project_name(value: &str) -> Result<&str, PanelError> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.len() <= 255
        && value.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'));
    if valid {
        Ok(value)
    } else {
        Err(PanelError::invalid_argument(format!(
            "`{value}` is not a Compose project's name"
        )))
    }
}

fn label<'a>(container: &'a ContainerSummary, name: &str) -> Option<&'a str> {
    container.labels.as_ref()?.get(name).map(String::as_str)
}

fn running(container: &ContainerSummary) -> bool {
    container.state == Some(ContainerSummaryStateEnum::RUNNING)
}

fn container_name(container: &ContainerSummary) -> String {
    container
        .names
        .iter()
        .flatten()
        .next()
        .map(|name| name.trim_start_matches('/').to_owned())
        .or_else(|| container.id.clone())
        .unwrap_or_default()
}

/// The containers of every project, or of one.
async fn containers(
    client: &Docker,
    project: Option<&str>,
) -> Result<Vec<ContainerSummary>, PanelError> {
    let wanted = project.map(|project| format!("{COMPOSE_PROJECT}={project}"));
    let filters: HashMap<&str, Vec<&str>> = match &wanted {
        Some(wanted) => HashMap::from([("label", vec![wanted.as_str()])]),
        None => HashMap::from([("label", vec![COMPOSE_PROJECT])]),
    };
    let options = ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    let listed = client
        .list_containers(Some(options))
        .await
        .map_err(|error| failure(&error))?;
    Ok(listed
        .into_iter()
        .filter(|container| {
            let named = label(container, COMPOSE_PROJECT);
            named.is_some() && project.is_none_or(|project| named == Some(project))
        })
        .collect())
}

/// Projects from their containers, by name.
fn projects(containers: &[ContainerSummary], installation: &str) -> Vec<wire::ComposeProject> {
    let mut projects: BTreeMap<&str, wire::ComposeProject> = BTreeMap::new();
    let mut services: BTreeMap<(&str, &str), wire::ComposeService> = BTreeMap::new();
    for container in containers {
        let Some(name) = label(container, COMPOSE_PROJECT) else {
            continue;
        };
        let project = projects
            .entry(name)
            .or_insert_with(|| wire::ComposeProject {
                name: name.to_owned(),
                working_directory: label(container, WORKING_DIRECTORY)
                    .unwrap_or_default()
                    .to_owned(),
                config_files: label(container, CONFIG_FILES)
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .filter(|file| !file.is_empty())
                    .map(str::to_owned)
                    .collect(),
                installation: name == installation,
                ..wire::ComposeProject::default()
            });
        project.containers += 1;
        project.running += u32::from(running(container));
        let service_name = label(container, COMPOSE_SERVICE).unwrap_or_default();
        let service =
            services
                .entry((name, service_name))
                .or_insert_with(|| wire::ComposeService {
                    name: service_name.to_owned(),
                    ..wire::ComposeService::default()
                });
        service.containers += 1;
        service.running += u32::from(running(container));
    }
    for ((project, _), service) in services {
        if let Some(project) = projects.get_mut(project) {
            project.services.push(service);
        }
    }
    projects.into_values().collect()
}

/// A Compose file a project's labels name: read only when it lies in the
/// project's working directory, ends in `.yml` or `.yaml`, is a regular
/// file and is at most 256 KiB. Links are followed before any of that is
/// checked.
fn read_compose_file(working_directory: &Path, file: &Path) -> Result<String, PanelError> {
    let unreadable = |what: &Path, error: std::io::Error| {
        PanelError::precondition_failed(format!(
            "the agent cannot read {}: {error}",
            what.display()
        ))
    };
    let directory = std::fs::canonicalize(working_directory)
        .map_err(|error| unreadable(working_directory, error))?;
    let resolved = std::fs::canonicalize(file).map_err(|error| unreadable(file, error))?;
    if !resolved.starts_with(&directory) {
        return Err(PanelError::precondition_failed(format!(
            "{} lies outside the project's working directory",
            file.display()
        )));
    }
    if !matches!(
        resolved
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("yml" | "yaml")
    ) {
        return Err(PanelError::precondition_failed(format!(
            "{} is not a .yml or .yaml file",
            file.display()
        )));
    }
    let opened = std::fs::File::open(&resolved).map_err(|error| unreadable(file, error))?;
    let metadata = opened.metadata().map_err(|error| unreadable(file, error))?;
    if !metadata.is_file() {
        return Err(PanelError::precondition_failed(format!(
            "{} is not a regular file",
            file.display()
        )));
    }
    let too_large =
        || PanelError::precondition_failed(format!("{} is larger than 256 KiB", file.display()));
    if metadata.len() > MOST_FILE_BYTES {
        return Err(too_large());
    }
    let mut content = String::new();
    opened
        .take(MOST_FILE_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|error| unreadable(file, error))?;
    if content.len() as u64 > MOST_FILE_BYTES {
        return Err(too_large());
    }
    Ok(content)
}

/// The Compose projects to panel-api.
pub(crate) struct ComposeService {
    engines: Arc<Engines>,
    /// The Compose project of the panel's own installation.
    installation: String,
}

impl ComposeService {
    pub(crate) fn new(engines: Arc<Engines>, installation: String) -> Self {
        Self {
            engines,
            installation,
        }
    }

    async fn act_on(
        &self,
        request: &wire::ComposeProjectsActRequest,
        action: ComposeAction,
    ) -> Result<wire::ComposeProjectsActResponse, PanelError> {
        if action == ComposeAction::Unspecified {
            return Err(PanelError::invalid_argument("up, down or restart"));
        }
        let name = project_name(&request.project)?;
        if action != ComposeAction::Up && name == self.installation {
            return Err(PanelError::precondition_failed(format!(
                "{name} is the panel's installation; manage it with Compose"
            )));
        }
        let client = self
            .engines
            .enabled(&request.engine)?
            .with_timeout(ACTION_TIMEOUT);
        let found = containers(&client, Some(name)).await?;
        if found.is_empty() {
            return Err(PanelError::not_found(format!(
                "no Compose project named {name}"
            )));
        }
        let targets: Vec<(String, String, bool)> = found
            .iter()
            .filter(|container| action != ComposeAction::Up || !running(container))
            .map(|container| {
                (
                    container.id.clone().unwrap_or_default(),
                    container_name(container),
                    running(container),
                )
            })
            .collect();
        let client = &client;
        let mut failures: Vec<wire::ComposeFailure> = stream::iter(targets.iter().cloned())
            .map(|(id, container, was_running)| async move {
                let done = match action {
                    ComposeAction::Up => {
                        client
                            .start_container(&id, None::<StartContainerOptions>)
                            .await
                    }
                    ComposeAction::Restart => {
                        client
                            .restart_container(&id, None::<RestartContainerOptions>)
                            .await
                    }
                    ComposeAction::Down => {
                        let stopped = if was_running {
                            client
                                .stop_container(&id, None::<StopContainerOptions>)
                                .await
                        } else {
                            Ok(())
                        };
                        match stopped {
                            Ok(()) => {
                                let options =
                                    RemoveContainerOptionsBuilder::default().force(true).build();
                                client.remove_container(&id, Some(options)).await
                            }
                            Err(error) => Err(error),
                        }
                    }
                    ComposeAction::Unspecified => Ok(()),
                };
                done.err().map(|error| wire::ComposeFailure {
                    name: container,
                    error: Some((&failure(&error)).into()),
                })
            })
            .buffer_unordered(CONCURRENT)
            .filter_map(|failed| async move { failed })
            .collect()
            .await;
        let changed = u32::try_from(targets.len() - failures.len()).unwrap_or(u32::MAX);
        if action == ComposeAction::Down {
            let label = format!("{COMPOSE_PROJECT}={name}");
            let filters = HashMap::from([("label", vec![label.as_str()])]);
            let networks = client
                .list_networks(Some(
                    ListNetworksOptionsBuilder::default()
                        .filters(&filters)
                        .build(),
                ))
                .await
                .map_err(|error| failure(&error))?;
            for network in networks {
                let ours = network
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get(COMPOSE_PROJECT))
                    .is_some_and(|project| project == name);
                let (Some(id), true) = (network.id, ours) else {
                    continue;
                };
                if let Err(error) = client.remove_network(&id).await {
                    failures.push(wire::ComposeFailure {
                        name: network.name.unwrap_or(id),
                        error: Some((&failure(&error)).into()),
                    });
                }
            }
        }
        failures.sort_by(|left, right| left.name.cmp(&right.name));
        let after = containers(client, Some(name)).await?;
        Ok(wire::ComposeProjectsActResponse {
            project: projects(&after, &self.installation).into_iter().next(),
            changed,
            failures,
            error: None,
        })
    }

    async fn read_logs(
        &self,
        request: &wire::ComposeProjectsLogsRequest,
    ) -> Result<(Vec<wire::ComposeLogLine>, bool), PanelError> {
        let name = project_name(&request.project)?;
        let client = self.engines.enabled(&request.engine)?;
        let found = containers(&client, Some(name)).await?;
        if found.is_empty() {
            return Err(PanelError::not_found(format!(
                "no Compose project named {name}"
            )));
        }
        let lines = match request.lines {
            0 => DEFAULT_LINES,
            lines => lines.min(MOST_LINES),
        };
        let since = request
            .since
            .and_then(|since| SystemTime::try_from(since).ok());
        let client = &client;
        let sources: Vec<(String, String, String)> = found
            .iter()
            .map(|container| {
                (
                    container.id.clone().unwrap_or_default(),
                    label(container, COMPOSE_SERVICE)
                        .unwrap_or_default()
                        .to_owned(),
                    container_name(container),
                )
            })
            .collect();
        let read: Vec<Result<(Vec<wire::ComposeLogLine>, bool), PanelError>> =
            stream::iter(sources)
                .map(|(id, service, name)| async move {
                    let (printed, truncated) =
                        container_logs::read(client, &id, lines, since).await?;
                    Ok((
                        printed
                            .into_iter()
                            .map(|line| wire::ComposeLogLine {
                                service: service.clone(),
                                container: name.clone(),
                                line: Some(line),
                            })
                            .collect(),
                        truncated,
                    ))
                })
                .buffer_unordered(CONCURRENT)
                .collect()
                .await;
        let mut merged = Vec::new();
        let mut truncated = false;
        for result in read {
            let (lines, cut) = result?;
            merged.extend(lines);
            truncated |= cut;
        }
        let time = |line: &wire::ComposeLogLine| {
            line.line
                .as_ref()
                .and_then(|line| line.time)
                .map_or((0, 0), |time| (time.seconds, time.nanos))
        };
        merged.sort_by_key(time);
        let excess = merged.len().saturating_sub(lines as usize);
        let mut kept: Vec<wire::ComposeLogLine> = merged.split_off(excess);
        let mut bytes = 0;
        let mut first = kept.len();
        for (at, line) in kept.iter().enumerate().rev() {
            bytes += line.line.as_ref().map_or(0, |line| line.text.len());
            if bytes > READ_BYTES {
                truncated = true;
                break;
            }
            first = at;
        }
        Ok((kept.split_off(first), truncated))
    }

    async fn read_files(
        &self,
        request: &wire::ComposeProjectsFilesRequest,
    ) -> Result<Vec<wire::ComposeFile>, PanelError> {
        let name = project_name(&request.project)?;
        let client = self.engines.enabled(&request.engine)?;
        let found = containers(&client, Some(name)).await?;
        let Some(project) = projects(&found, &self.installation).into_iter().next() else {
            return Err(PanelError::not_found(format!(
                "no Compose project named {name}"
            )));
        };
        let working_directory = PathBuf::from(&project.working_directory);
        tokio::task::spawn_blocking(move || {
            project
                .config_files
                .into_iter()
                .map(|path| {
                    let (content, error) =
                        match read_compose_file(&working_directory, Path::new(&path)) {
                            Ok(content) => (content, None),
                            Err(error) => (String::new(), Some((&error).into())),
                        };
                    wire::ComposeFile {
                        path,
                        content,
                        error,
                    }
                })
                .collect()
        })
        .await
        .map_err(|error| PanelError::internal(format!("reading the Compose files failed: {error}")))
    }
}

#[tonic::async_trait]
impl ComposeProjects for ComposeService {
    async fn list(
        &self,
        request: Request<wire::ComposeProjectsListRequest>,
    ) -> Result<Response<wire::ComposeProjectsListResponse>, Status> {
        let listed = async {
            let client = self.engines.enabled(&request.get_ref().engine)?;
            Ok(projects(
                &containers(&client, None).await?,
                &self.installation,
            ))
        };
        let (projects, error) = answer(listed.await);
        Ok(Response::new(wire::ComposeProjectsListResponse {
            observed_at: projects.as_ref().map(|_| SystemTime::now().into()),
            projects: projects.unwrap_or_default(),
            error,
        }))
    }

    async fn act(
        &self,
        request: Request<wire::ComposeProjectsActRequest>,
    ) -> Result<Response<wire::ComposeProjectsActResponse>, Status> {
        let request = request.into_inner();
        let action = ComposeAction::try_from(request.action).unwrap_or(ComposeAction::Unspecified);
        let result = tokio::time::timeout(ACTION_TIMEOUT, self.act_on(&request, action))
            .await
            .unwrap_or_else(|_| {
                Err(PanelError::deadline_exceeded(
                    "the engine did not finish in time",
                ))
            });
        Ok(Response::new(match result {
            Ok(done) => {
                tracing::info!(
                    event = "compose_action",
                    engine = %request.engine,
                    project = %request.project,
                    action = action.as_str_name(),
                    changed = done.changed,
                    failed = done.failures.len(),
                );
                done
            }
            Err(error) => {
                tracing::warn!(
                    event = "compose_action_refused",
                    engine = %request.engine,
                    project = %request.project,
                    action = action.as_str_name(),
                    error_code = %error.code,
                );
                wire::ComposeProjectsActResponse {
                    error: Some((&error).into()),
                    ..wire::ComposeProjectsActResponse::default()
                }
            }
        }))
    }

    async fn logs(
        &self,
        request: Request<wire::ComposeProjectsLogsRequest>,
    ) -> Result<Response<wire::ComposeProjectsLogsResponse>, Status> {
        let result = tokio::time::timeout(
            container_logs::READ_TIMEOUT,
            self.read_logs(request.get_ref()),
        )
        .await
        .unwrap_or_else(|_| {
            Err(PanelError::deadline_exceeded(
                "the engine did not send the logs in time",
            ))
        });
        let (read, error) = answer(result);
        let observed_at = read.as_ref().map(|_| SystemTime::now().into());
        let (lines, truncated) = read.unwrap_or_default();
        Ok(Response::new(wire::ComposeProjectsLogsResponse {
            observed_at,
            lines,
            truncated,
            error,
        }))
    }

    async fn files(
        &self,
        request: Request<wire::ComposeProjectsFilesRequest>,
    ) -> Result<Response<wire::ComposeProjectsFilesResponse>, Status> {
        let (files, error) = answer(self.read_files(request.get_ref()).await);
        Ok(Response::new(wire::ComposeProjectsFilesResponse {
            files: files.unwrap_or_default(),
            error,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_engine::{engine, engine_with, engines, Calls};

    fn service(socket: PathBuf, installation: &str) -> ComposeService {
        ComposeService::new(engines(socket, None), installation.into())
    }

    async fn act(
        service: &ComposeService,
        project: &str,
        action: ComposeAction,
    ) -> wire::ComposeProjectsActResponse {
        service
            .act(Request::new(wire::ComposeProjectsActRequest {
                context: None,
                engine: "docker".into(),
                project: project.into(),
                action: action.into(),
            }))
            .await
            .unwrap()
            .into_inner()
    }

    #[tokio::test]
    async fn projects_are_known_by_their_containers_labels() {
        let directory = tempfile::tempdir().unwrap();
        let compose = service(engine(directory.path()).await, "pingora-panel");
        let listed = compose
            .list(Request::new(wire::ComposeProjectsListRequest {
                context: None,
                engine: "docker".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(listed.error.is_none(), "{:?}", listed.error);
        let names: Vec<_> = listed
            .projects
            .iter()
            .map(|project| project.name.as_str())
            .collect();
        assert_eq!(names, ["cache", "shop"]);
        let shop = &listed.projects[1];
        assert_eq!(shop.working_directory, "/srv/shop");
        assert_eq!(shop.config_files, ["/srv/shop/compose.yaml"]);
        assert_eq!(
            (shop.containers, shop.running, shop.installation),
            (1, 1, false)
        );
        assert_eq!(shop.services[0].name, "web");
        assert_eq!(listed.projects[0].running, 0);
    }

    #[tokio::test]
    async fn projects_are_brought_up_down_and_restarted() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Calls::default();
        let socket = engine_with(directory.path(), calls.clone()).await;
        let compose = service(socket.clone(), "pingora-panel");

        let up = act(&compose, "cache", ComposeAction::Up).await;
        assert!(up.error.is_none(), "{:?}", up.error);
        assert_eq!(up.changed, 1);
        let restarted = act(&compose, "shop", ComposeAction::Restart).await;
        assert_eq!(restarted.changed, 1);
        let down = act(&compose, "shop", ComposeAction::Down).await;
        assert!(down.failures.is_empty(), "{:?}", down.failures);
        assert_eq!(down.changed, 1);
        assert_eq!(
            *calls.lock().unwrap(),
            [
                "start a1",
                "restart b2",
                "stop b2",
                "remove b2 force=true volumes=false",
                "remove-network n3"
            ],
            "down keeps volumes and leaves other projects' networks"
        );

        let installation = service(socket, "shop");
        let refused = act(&installation, "shop", ComposeAction::Down).await;
        assert_eq!(refused.error.unwrap().code, "PRECONDITION_FAILED");
        let missing = act(&compose, "ghost", ComposeAction::Up).await;
        assert_eq!(missing.error.unwrap().code, "NOT_FOUND");
        let unnamed = act(&compose, "Shop!", ComposeAction::Up).await;
        assert_eq!(unnamed.error.unwrap().code, "INVALID_ARGUMENT");
    }

    #[tokio::test]
    async fn a_projects_logs_say_which_service_printed_each_line() {
        let directory = tempfile::tempdir().unwrap();
        let compose = service(engine(directory.path()).await, "pingora-panel");
        let logs = compose
            .logs(Request::new(wire::ComposeProjectsLogsRequest {
                context: None,
                engine: "docker".into(),
                project: "shop".into(),
                lines: 2,
                since: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(logs.error.is_none(), "{:?}", logs.error);
        let lines: Vec<_> = logs
            .lines
            .iter()
            .map(|line| {
                (
                    line.service.as_str(),
                    line.line.as_ref().unwrap().text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            lines,
            [("web", "upstream timed out"), ("web", "GET /cart 200")]
        );
        assert_eq!(logs.lines[0].container, "shop-web-1");
    }

    #[tokio::test]
    async fn a_projects_files_are_read_where_its_labels_say() {
        let directory = tempfile::tempdir().unwrap();
        let compose = service(engine(directory.path()).await, "pingora-panel");
        let files = compose
            .files(Request::new(wire::ComposeProjectsFilesRequest {
                context: None,
                engine: "docker".into(),
                project: "shop".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(files.error.is_none(), "{:?}", files.error);
        assert_eq!(files.files[0].path, "/srv/shop/compose.yaml");
        assert_eq!(
            files.files[0].error.as_ref().unwrap().code,
            "PRECONDITION_FAILED",
            "a file the agent cannot reach says so"
        );
    }

    #[test]
    fn compose_files_are_read_only_within_the_working_directory() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("shop");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("compose.yaml"), "services:\n  web: {}\n").unwrap();
        std::fs::write(root.path().join("outside.yaml"), "secret: 1\n").unwrap();
        std::fs::write(project.join("notes.txt"), "x").unwrap();
        std::fs::create_dir(project.join("folder.yml")).unwrap();
        std::fs::write(project.join("huge.yml"), vec![b'#'; 256 * 1024 + 1]).unwrap();
        std::os::unix::fs::symlink(root.path().join("outside.yaml"), project.join("escape.yml"))
            .unwrap();

        assert_eq!(
            read_compose_file(&project, &project.join("compose.yaml")).unwrap(),
            "services:\n  web: {}\n"
        );
        for refused in [
            root.path().join("outside.yaml"),
            project.join("escape.yml"),
            project.join("notes.txt"),
            project.join("folder.yml"),
            project.join("huge.yml"),
            project.join("missing.yml"),
        ] {
            let error = read_compose_file(&project, &refused).unwrap_err();
            assert_eq!(
                error.code.as_str(),
                "PRECONDITION_FAILED",
                "{}",
                refused.display()
            );
        }
    }
}
