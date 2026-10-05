use super::*;
use panel_application::{
    CommandContext, ComposeAction, ComposeChange, ComposeFailure, ComposeFile, ComposeLogLine,
    ComposeLogs, ComposePort, ComposeProject, ComposeProjectList, ContainerLogLine,
    ContainerLogQuery, ContainerLogStream, ProjectService, RequestScope,
};
use serde_json::Value;
use std::time::{Duration, UNIX_EPOCH};

/// A shop with one service, and the panel's own installation.
struct Projects;

fn shop(running: u32) -> ComposeProject {
    ComposeProject {
        name: "shop".into(),
        working_directory: Some("/srv/shop".into()),
        config_files: vec!["/srv/shop/compose.yaml".into()],
        services: vec![ProjectService {
            name: "web".into(),
            containers: 2,
            running,
        }],
        containers: 2,
        running,
        installation: false,
    }
}

#[async_trait]
impl ComposePort for Projects {
    async fn projects(&self, _: RequestScope, _: String) -> Result<ComposeProjectList> {
        Ok(ComposeProjectList {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_010)),
            projects: vec![
                ComposeProject {
                    name: "pingora-panel".into(),
                    installation: true,
                    ..shop(1)
                },
                shop(1),
            ],
        })
    }

    async fn act_on_project(
        &self,
        context: CommandContext,
        _: String,
        project: String,
        action: ComposeAction,
    ) -> Result<ComposeChange> {
        assert_eq!(context.actor(), "ops");
        if project == "pingora-panel" {
            return Err(PanelError::precondition_failed(
                "the panel's own installation is only ever brought up",
            ));
        }
        Ok(match action {
            ComposeAction::Down => ComposeChange {
                project: None,
                changed: 2,
                failures: Vec::new(),
            },
            _ => ComposeChange {
                project: Some(shop(1)),
                changed: 1,
                failures: vec![ComposeFailure {
                    name: "shop-web-2".into(),
                    error: PanelError::precondition_failed("port 8080 is taken"),
                }],
            },
        })
    }

    async fn project_logs(
        &self,
        _: RequestScope,
        _: String,
        _: String,
        query: ContainerLogQuery,
    ) -> Result<ComposeLogs> {
        assert_eq!(query.lines, 200);
        Ok(ComposeLogs {
            observed_at: None,
            lines: vec![ComposeLogLine {
                service: "web".into(),
                container: "shop-web-1".into(),
                line: ContainerLogLine {
                    time: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
                    stream: ContainerLogStream::Stderr,
                    text: "listening".into(),
                },
            }],
            truncated: false,
        })
    }

    async fn project_files(
        &self,
        _: RequestScope,
        _: String,
        _: String,
    ) -> Result<Vec<ComposeFile>> {
        Ok(vec![
            ComposeFile {
                path: "/srv/shop/compose.yaml".into(),
                content: Ok("services:\n  web:\n    image: nginx\n".into()),
            },
            ComposeFile {
                path: "/srv/shop/compose.override.yaml".into(),
                content: Err(PanelError::permission_denied("it cannot be read")),
            },
        ])
    }
}

fn app(compose: bool) -> axum::Router {
    let state = ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    )));
    router(if compose {
        state.with_compose(Arc::new(Projects))
    } else {
        state
    })
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    send(app, Request::get(path).body(Body::empty()).unwrap()).await
}

async fn act(app: &axum::Router, project: &str, action: &str) -> (StatusCode, Value) {
    let request = Request::post(format!(
        "/api/v1/container-engines/docker/compose-projects/{project}/{action}"
    ))
    .header("x-actor", "ops")
    .header("idempotency-key", format!("{project}-{action}"))
    .header("x-deadline", "2099-01-01T00:00:00Z")
    .body(Body::empty())
    .unwrap();
    send(app, request).await
}

const PROJECTS: &str = "/api/v1/container-engines/docker/compose-projects";

#[tokio::test]
async fn projects_are_listed_with_their_services() {
    let (status, list) = get(&app(true), PROJECTS).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["observed_at"], "2027-01-15T08:00:10Z");
    assert_eq!(list["projects"][0]["installation"], true);
    let shop = &list["projects"][1];
    assert_eq!(shop["working_directory"], "/srv/shop");
    assert_eq!(shop["config_files"][0], "/srv/shop/compose.yaml");
    assert_eq!(shop["services"][0]["name"], "web");
    assert_eq!(shop["running"], 1);
}

#[tokio::test]
async fn projects_are_brought_up_down_and_restarted() {
    let app = app(true);
    let (status, up) = act(&app, "shop", "up").await;
    assert_eq!(status, StatusCode::OK, "{up}");
    assert_eq!(up["changed"], 1);
    assert_eq!(up["project"]["name"], "shop");
    assert_eq!(up["failures"][0]["name"], "shop-web-2");
    assert_eq!(up["failures"][0]["error"]["code"], "PRECONDITION_FAILED");

    let (status, down) = act(&app, "shop", "down").await;
    assert_eq!(status, StatusCode::OK, "{down}");
    assert_eq!(down["project"], Value::Null);
    assert_eq!(down["changed"], 2);

    let (status, refused) = act(&app, "pingora-panel", "restart").await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED, "{refused}");

    for (project, action) in [("shop", "pause"), ("Shop", "up"), ("-shop", "up")] {
        let (status, _) = act(&app, project, action).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{project} {action}");
    }
}

#[tokio::test]
async fn a_projects_logs_and_files_are_read() {
    let app = app(true);
    let (status, logs) = get(&app, &format!("{PROJECTS}/shop/logs")).await;
    assert_eq!(status, StatusCode::OK, "{logs}");
    let line = &logs["lines"][0];
    assert_eq!(line["service"], "web");
    assert_eq!(line["container"], "shop-web-1");
    assert_eq!(line["line"]["stream"], "stderr");
    assert_eq!(line["line"]["time"], "2027-01-15T08:00:00Z");
    for query in ["lines=0", "lines=5001", "since=yesterday"] {
        let (status, _) = get(&app, &format!("{PROJECTS}/shop/logs?{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }

    let (status, files) = get(&app, &format!("{PROJECTS}/shop/files")).await;
    assert_eq!(status, StatusCode::OK, "{files}");
    assert_eq!(files["files"][0]["path"], "/srv/shop/compose.yaml");
    assert!(files["files"][0]["content"]
        .as_str()
        .unwrap()
        .starts_with("services:"));
    assert_eq!(files["files"][0]["error"], Value::Null);
    assert_eq!(files["files"][1]["content"], Value::Null);
    assert_eq!(files["files"][1]["error"]["code"], "PERMISSION_DENIED");
}

#[tokio::test]
async fn without_the_agent_projects_are_unsupported() {
    let (status, problem) = get(&app(false), PROJECTS).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "UNSUPPORTED_CAPABILITY");
}
