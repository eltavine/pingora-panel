//! What runs, whether an upgrade can start, and a bundle for diagnosis
//! with its secrets removed (ADR 0047).

mod redaction;

use crate::{
    alerts::{AlertRuleView, AlertStateName},
    audit::AuditEvent,
    backups::BackupDetails,
    error::ApiError,
    gateway_runtime::DataPlaneResponse,
    host::HostSummaryView,
    request_context::{request_scope, QueryHeaders},
    ApiState, GatewayStatusResponse, ServiceInstanceResponse, ServiceListingResponse,
};
use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{SecondsFormat, Utc};
use panel_application::{
    AuditFilter, Backup, BackupState, GatewayStatus, GatewayUseCases, RequestScope,
};
use panel_config_api::{ConfigurationQuery, ModelQuery, RevisionQuery};
use panel_config_model::SiteSummary;
use panel_errors::{ErrorCode, PanelError, Result as PanelResult};
use panel_health::{HealthStatus, HealthWatch, Impact};
use panel_identity::{Access as HeldAccess, Permission};
use panel_plugin_api::{PluginHealth, PluginList, PluginQuery, PluginState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};
use utoipa::ToSchema;

/// From this age the newest backup is a warning before an upgrade, which
/// takes another first.
const BACKUP_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Space kept free beyond a backup of the databases.
const SPACE_MARGIN: u64 = 256 * 1024 * 1024;
/// Revisions read for those still being applied or that did not run.
const REVISIONS_READ: u32 = 20;
const BUNDLE_BACKUPS: usize = 10;
const BUNDLE_AUDIT_EVENTS: u32 = 100;

/// What the composition knows of its release and of how it was deployed.
#[derive(Clone, Debug)]
pub struct SystemInfo {
    version: String,
    commit: String,
    deployment: Option<String>,
    data_directory: Option<PathBuf>,
}

impl Default for SystemInfo {
    fn default() -> Self {
        Self::new("dev", "unknown")
    }
}

impl SystemInfo {
    /// The release, such as `0.9.0`, and the commit it was built from.
    pub fn new(version: impl Into<String>, commit: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            commit: commit.into(),
            deployment: None,
            data_directory: None,
        }
    }

    /// The record of the deployment, as JSON, that the lifecycle tool
    /// passes on every install, upgrade, rollback and restore.
    pub fn with_deployment(mut self, record: impl Into<String>) -> Self {
        self.deployment = Some(record.into());
        self
    }

    /// The directory of the databases, whose free space an upgrade needs.
    pub fn with_data_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.data_directory = Some(directory.into());
        self
    }
}

/// The versions of what runs (ADR 0047).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct VersionManifest {
    /// RFC 3339 time at which it was read.
    pub observed_at: String,
    /// The release, such as `0.9.0`; `dev` for a build outside a release.
    pub release: String,
    /// The commit the release was built from; `unknown` when not recorded.
    pub commit: String,
    /// The version of this API, which its paths carry.
    pub api: String,
    /// The version of the configuration language.
    pub language: u32,
    /// The IR schema the gateway accepts; absent while it is unreachable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ir_schema: Option<String>,
    /// Each module of the control plane with its build, its database's
    /// schema and the protocol revisions it serves.
    pub modules: Vec<ServiceInstanceResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<GatewayVersions>,
    /// The host agent; absent without one or while it does not answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentVersions>,
    /// How the lifecycle tool deployed the installation; absent when it
    /// did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment: Option<DeploymentRecord>,
    /// What could not be read, such as an unreachable gateway.
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct GatewayVersions {
    /// The gateway's build.
    pub gateway: String,
    /// The proxy engine inside it.
    pub engine: String,
    /// The adapter that turns the IR into the engine's configuration.
    pub adapter: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentVersions {
    pub build: String,
    /// The revision of the agent protocol it speaks.
    pub protocol: String,
    pub hostname: String,
}

/// What the lifecycle tool records when it changes the installation.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct DeploymentRecord {
    /// RFC 3339 time of the change.
    pub changed_at: String,
    /// `install`, `upgrade`, `rollback` or `restore`.
    pub action: String,
    /// `docker` or `podman`.
    pub engine: String,
    /// The Compose project.
    pub project: String,
    /// The release deployed before; absent after an install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    pub images: Vec<DeployedImage>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct DeployedImage {
    /// The Compose service that runs it.
    pub service: String,
    /// The reference it was pulled by.
    pub image: String,
    /// `sha256:` and 64 hexadecimal digits; absent for a local build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// Whether an upgrade can start now (ADR 0047).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct UpgradeReadiness {
    /// RFC 3339 time at which it was checked.
    pub observed_at: String,
    /// No check failed; warnings do not hold an upgrade back.
    pub ready: bool,
    pub checks: Vec<ReadinessCheck>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ReadinessCheck {
    /// `modules`, `gateway`, `backup` or `space`.
    pub name: String,
    pub state: ReadinessState,
    /// What was found, and what to do when it failed.
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReadinessState {
    Pass,
    Warn,
    Fail,
}

impl ReadinessCheck {
    fn new(name: &str, state: ReadinessState, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state,
            detail: detail.into(),
        }
    }
}

/// A bundle for diagnosing the installation, every field of which passed
/// redaction (ADR 0047).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DiagnosticBundle {
    /// RFC 3339 time at which it was put together.
    pub generated_at: String,
    pub versions: VersionManifest,
    pub readiness: UpgradeReadiness,
    /// The health of the control plane and of what it depends on, as
    /// `application/health+json` reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub health: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<GatewayStatusResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_plane: Option<DataPlaneResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<HostSummaryView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration: Option<ConfigurationDiagnosis>,
    /// The newest backups.
    pub backups: Vec<BackupDetails>,
    /// Rules that are pending or firing, or whose evaluation failed.
    pub alerts: Vec<AlertRuleView>,
    pub plugins: Vec<PluginDiagnosis>,
    /// The newest audit events.
    pub audit: Vec<AuditEvent>,
    /// Sections left out for permissions the caller lacks, such as
    /// `audit: audit.read`.
    pub withheld: Vec<String>,
    /// What could not be read.
    pub problems: Vec<String>,
}

/// How much the configuration holds, and the recent revisions that are
/// being applied or did not run.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ConfigurationDiagnosis {
    pub draft_version: u64,
    /// The draft version the gateway runs, once one was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sites: Option<SiteSummary>,
    pub upstreams: usize,
    pub listeners: usize,
    pub tls_profiles: usize,
    /// Revisions being applied, failed or rejected, newest first.
    #[schema(value_type = Vec<Object>)]
    pub revisions: Vec<Value>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PluginDiagnosis {
    pub name: String,
    pub state: PluginState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<PluginHealth>,
    /// Why the plugin, though enabled, does not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The versions of the release, of each module, of the gateway and its
/// engine, of the host agent and of the deployed images.
#[utoipa::path(get, path = "/api/v1/system/versions", params(QueryHeaders),
    responses((status = 200, body = VersionManifest)), tag = "system")]
pub(crate) async fn system_versions<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<VersionManifest>, ApiError> {
    let scope = request_scope(&headers)?;
    Ok(Json(manifest(&state, &scope).await))
}

/// Whether an upgrade can start: every module healthy, nothing prepared
/// or being applied on the gateway, the newest backup and the space for
/// another.
#[utoipa::path(get, path = "/api/v1/system/preflight", params(QueryHeaders),
    responses((status = 200, body = UpgradeReadiness)), tag = "system")]
pub(crate) async fn system_preflight<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<UpgradeReadiness>, ApiError> {
    let scope = request_scope(&headers)?;
    Ok(Json(readiness(&state, &scope).await))
}

/// One JSON document for diagnosing the installation: versions,
/// readiness, health, the gateway and the host, the configuration's
/// counts, recent failures and audit events, with every secret removed.
/// Sections the caller may not read are left out and named.
#[utoipa::path(get, path = "/api/v1/system/diagnostics", params(QueryHeaders),
    responses((status = 200, body = DiagnosticBundle,
        headers(("Content-Disposition" = String, description = "Names the bundle as an attachment")))),
    tag = "system")]
pub(crate) async fn system_diagnostics<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    held: Option<Extension<HeldAccess>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = request_scope(&headers)?;
    let bundle = bundle(&state, held.as_ref().map(|Extension(held)| held), &scope).await;
    let mut document = serde_json::to_value(&bundle).map_err(|_| {
        ApiError::new(PanelError::internal(
            "the diagnostic bundle cannot be encoded",
        ))
    })?;
    redaction::redact(&mut document);
    let attachment = format!(
        "attachment; filename=\"pingora-panel-diagnostics-{}.json\"",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let attachment = HeaderValue::try_from(attachment)
        .map_err(|_| ApiError::new(PanelError::internal("the bundle cannot be named")))?;
    Ok((
        [
            (header::CONTENT_DISPOSITION, attachment),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        Json(document),
    )
        .into_response())
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn unsupported(error: &PanelError) -> bool {
    error.code.as_str() == ErrorCode::UNSUPPORTED_CAPABILITY
}

/// What `result` holds, or nothing with what failed noted in `problems`;
/// what this composition does not offer is no problem.
fn kept<T>(problems: &mut Vec<String>, what: &str, result: PanelResult<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) if unsupported(&error) => None,
        Err(error) => {
            problems.push(format!("{what} cannot be read: {}", error.message));
            None
        }
    }
}

async fn manifest<U: GatewayUseCases>(
    state: &ApiState<U>,
    scope: &RequestScope,
) -> VersionManifest {
    let (status, listing, plane, agent) = tokio::join!(
        state.use_cases.status_with_scope(scope.clone()),
        async {
            match &state.directory {
                Some(directory) => directory.list().await.map(Some),
                None => Ok(None),
            }
        },
        async {
            match &state.runtime {
                Some(runtime) => runtime.data_plane(scope.clone()).await.map(Some),
                None => Ok(None),
            }
        },
        state.host_agent.agent(scope.clone()),
    );
    let mut problems = Vec::new();
    let ir_schema = kept(&mut problems, "the gateway's status", status)
        .map(|status| status.schema_version().to_owned());
    let modules = kept(&mut problems, "the service directory", listing)
        .flatten()
        .map(|listing| ServiceListingResponse::from(listing).services)
        .unwrap_or_default();
    let gateway = kept(&mut problems, "the gateway's data plane", plane)
        .flatten()
        .map(|plane| GatewayVersions {
            gateway: plane.gateway_version,
            engine: plane.engine_version,
            adapter: plane.adapter_version,
        });
    let agent = kept(&mut problems, "the host agent", agent).map(|agent| AgentVersions {
        build: agent.build,
        protocol: agent.protocol,
        hostname: agent.hostname,
    });
    let deployment = state.system.deployment.as_deref().and_then(|record| {
        serde_json::from_str(record)
            .map_err(|error| {
                problems.push(format!("the deployment record cannot be read: {error}"))
            })
            .ok()
    });
    VersionManifest {
        observed_at: now(),
        release: state.system.version.clone(),
        commit: state.system.commit.clone(),
        api: "v1".into(),
        language: panel_config_dsl::LANGUAGE_VERSION,
        ir_schema,
        modules,
        gateway,
        agent,
        deployment,
        problems,
    }
}

async fn readiness<U: GatewayUseCases>(
    state: &ApiState<U>,
    scope: &RequestScope,
) -> UpgradeReadiness {
    let (status, revisions, backups, space) = tokio::join!(
        state.use_cases.status_with_scope(scope.clone()),
        recent_revisions(state, scope),
        state.backups.list(scope.clone()),
        data_space(state.system.data_directory.clone()),
    );
    let mut checks = vec![
        modules_check(state.health.as_ref()),
        gateway_check(status, revisions),
        backup_check(backups, SystemTime::now()),
    ];
    checks.extend(space.map(space_check));
    UpgradeReadiness {
        observed_at: now(),
        ready: checks
            .iter()
            .all(|check| check.state != ReadinessState::Fail),
        checks,
    }
}

fn modules_check(health: Option<&HealthWatch>) -> ReadinessCheck {
    let Some(health) = health else {
        return ReadinessCheck::new(
            "modules",
            ReadinessState::Warn,
            "the modules' health is not watched here",
        );
    };
    let report = health.current();
    // What the control plane serves or changes depends on: a failure holds
    // an upgrade back; a failure only reported, or a warning, does not.
    let with = |held_back: bool| -> Vec<&str> {
        report
            .checks()
            .iter()
            .filter(|(_, components)| {
                components.iter().any(|component| {
                    let failed = component.status() == HealthStatus::Fail;
                    let reported = matches!(component.impact(), Impact::Informational);
                    if held_back {
                        failed && !reported
                    } else {
                        (failed && reported) || component.status() == HealthStatus::Warn
                    }
                })
            })
            .map(|(name, _)| name.as_str())
            .collect()
    };
    let failing = with(true);
    if !failing.is_empty() {
        return ReadinessCheck::new(
            "modules",
            ReadinessState::Fail,
            format!("failing: {}; bring them back first", failing.join(", ")),
        );
    }
    let warning = with(false);
    if !warning.is_empty() {
        return ReadinessCheck::new(
            "modules",
            ReadinessState::Warn,
            format!("degraded: {}", warning.join(", ")),
        );
    }
    ReadinessCheck::new("modules", ReadinessState::Pass, "every module is healthy")
}

fn gateway_check(
    status: PanelResult<GatewayStatus>,
    revisions: Option<PanelResult<Vec<Value>>>,
) -> ReadinessCheck {
    let fail = |detail: String| ReadinessCheck::new("gateway", ReadinessState::Fail, detail);
    let status = match status {
        Ok(status) => status,
        Err(error) => return fail(format!("the gateway cannot be reached: {}", error.message)),
    };
    if !status.ready() {
        return fail(match status.message() {
            Some(message) => format!("the gateway is not ready: {message}"),
            None => "the gateway is not ready".into(),
        });
    }
    if status.prepared_count() > 0 {
        return fail(format!(
            "snapshots prepared and not activated: {}; activate or abort them first",
            status.prepared_count()
        ));
    }
    match revisions {
        Some(Ok(revisions)) => {
            let applying: Vec<String> = revisions
                .iter()
                .filter(|revision| revision["outcome"] == "applying")
                .map(|revision| revision["id"].to_string())
                .collect();
            if !applying.is_empty() {
                return fail(format!(
                    "revisions being applied: {}; wait for them to settle",
                    applying.join(", ")
                ));
            }
        }
        Some(Err(error)) => {
            return ReadinessCheck::new(
                "gateway",
                ReadinessState::Warn,
                format!("the revisions cannot be read: {}", error.message),
            );
        }
        None => {}
    }
    ReadinessCheck::new(
        "gateway",
        ReadinessState::Pass,
        "the gateway is ready and nothing is being applied",
    )
}

fn backup_check(backups: PanelResult<Vec<Backup>>, now: SystemTime) -> ReadinessCheck {
    let check = |state, detail: String| ReadinessCheck::new("backup", state, detail);
    let backups = match backups {
        Ok(backups) => backups,
        Err(error) if unsupported(&error) => {
            return check(
                ReadinessState::Warn,
                "backups are not available here; take one before upgrading".into(),
            );
        }
        Err(error) => {
            return check(
                ReadinessState::Fail,
                format!("backups cannot be listed: {}", error.message),
            );
        }
    };
    if backups
        .iter()
        .any(|backup| matches!(backup.state, BackupState::Pending | BackupState::Running))
    {
        return check(
            ReadinessState::Fail,
            "a backup is being taken; wait for it to finish".into(),
        );
    }
    let newest = backups
        .iter()
        .filter(|backup| matches!(backup.state, BackupState::Completed))
        .filter_map(|backup| backup.finished_at)
        .max();
    match newest {
        None => check(
            ReadinessState::Warn,
            "no backup was taken yet; the upgrade takes one first".into(),
        ),
        Some(finished) => {
            let age = now.duration_since(finished).unwrap_or_default();
            if age <= BACKUP_AGE {
                check(
                    ReadinessState::Pass,
                    format!("the newest backup is {} old", elapsed(age)),
                )
            } else {
                check(
                    ReadinessState::Warn,
                    format!(
                        "the newest backup is {} old; the upgrade takes one first",
                        elapsed(age)
                    ),
                )
            }
        }
    }
}

fn elapsed(age: Duration) -> String {
    let minutes = age.as_secs() / 60;
    match minutes {
        0..60 => format!("{minutes} min"),
        60..2880 => format!("{} h", minutes / 60),
        _ => format!("{} days", minutes / 1440),
    }
}

/// The free space where the databases are, and how much they take.
struct DataSpace {
    free: u64,
    databases: u64,
}

fn space_check(space: Result<DataSpace, String>) -> ReadinessCheck {
    let check = |state, detail: String| ReadinessCheck::new("space", state, detail);
    let DataSpace { free, databases } = match space {
        Ok(space) => space,
        Err(error) => {
            return check(
                ReadinessState::Warn,
                format!("the free space cannot be read: {error}"),
            )
        }
    };
    let detail = format!(
        "{} free, the databases take {}",
        size(free),
        size(databases)
    );
    if free >= databases.saturating_mul(2).saturating_add(SPACE_MARGIN) {
        check(ReadinessState::Pass, detail)
    } else if free >= databases {
        check(
            ReadinessState::Warn,
            format!("{detail}; a backup and the migrations may not both fit"),
        )
    } else {
        check(
            ReadinessState::Fail,
            format!("{detail}; a backup does not fit, free some space first"),
        )
    }
}

fn size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    #[allow(clippy::cast_precision_loss)]
    let mib = bytes as f64 / MIB;
    if mib >= 1024.0 {
        format!("{:.1} GiB", mib / 1024.0)
    } else {
        format!("{mib:.1} MiB")
    }
}

async fn data_space(directory: Option<PathBuf>) -> Option<Result<DataSpace, String>> {
    let directory = directory?;
    Some(
        tokio::task::spawn_blocking(move || measure(&directory))
            .await
            .unwrap_or_else(|error| Err(error.to_string())),
    )
}

#[cfg(unix)]
fn measure(directory: &std::path::Path) -> Result<DataSpace, String> {
    let stats = rustix::fs::statvfs(directory).map_err(|error| error.to_string())?;
    let mut databases = 0;
    for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".db") || name.ends_with(".db-wal") {
            databases += entry.metadata().map_or(0, |metadata| metadata.len());
        }
    }
    Ok(DataSpace {
        free: stats.f_bavail.saturating_mul(stats.f_frsize),
        databases,
    })
}

#[cfg(not(unix))]
fn measure(_: &std::path::Path) -> Result<DataSpace, String> {
    Err("this platform does not report free space".into())
}

/// The newest revisions; none without the configuration module.
async fn recent_revisions<U>(
    state: &ApiState<U>,
    scope: &RequestScope,
) -> Option<PanelResult<Vec<Value>>> {
    let configuration = state.configuration.as_ref()?;
    let query = ConfigurationQuery::Revision(RevisionQuery::Revisions {
        before: None,
        limit: Some(REVISIONS_READ),
    });
    Some(
        configuration
            .read(scope.clone(), query)
            .await
            .and_then(|output| items(&output.content, "items")),
    )
}

/// The array under `field` of a JSON document, or the document itself
/// when it is one.
fn items(content: &[u8], field: &str) -> PanelResult<Vec<Value>> {
    let document: Value = serde_json::from_slice(content)
        .map_err(|_| PanelError::internal("a module answered an unreadable document"))?;
    match document {
        Value::Array(items) => Ok(items),
        Value::Object(mut fields) => match fields.remove(field) {
            Some(Value::Array(items)) => Ok(items),
            _ => Ok(Vec::new()),
        },
        _ => Ok(Vec::new()),
    }
}

async fn bundle<U: GatewayUseCases>(
    state: &ApiState<U>,
    held: Option<&HeldAccess>,
    scope: &RequestScope,
) -> DiagnosticBundle {
    let may =
        |permission: Permission| held.is_none_or(|held| held.unrestricted.contains(permission));
    let mut withheld = Vec::new();
    let mut allow = |section: &str, permission: Permission| {
        let allowed = may(permission);
        if !allowed {
            withheld.push(format!("{section}: {}", permission.name()));
        }
        allowed
    };
    let gateway_allowed = allow("gateway", Permission::GatewayRead);
    let host_allowed = allow("host", Permission::HostRead);
    let configuration_allowed = allow("configuration", Permission::ConfigRead);
    let backups_allowed = allow("backups", Permission::BackupsRead);
    let alerts_allowed = allow("alerts", Permission::AlertsRead);
    let plugins_allowed = allow("plugins", Permission::PluginsRead);
    let audit_allowed = allow("audit", Permission::AuditRead);
    let (versions, readiness, gateway, plane, host, configuration, backups, alerts, plugins, audit) = tokio::join!(
        manifest(state, scope),
        readiness(state, scope),
        async {
            if !gateway_allowed {
                return Ok(None);
            }
            state
                .use_cases
                .status_with_scope(scope.clone())
                .await
                .map(Some)
        },
        async {
            match &state.runtime {
                Some(runtime) if gateway_allowed => {
                    runtime.data_plane(scope.clone()).await.map(Some)
                }
                _ => Ok(None),
            }
        },
        async {
            match &state.host {
                Some(host) if host_allowed => host.summary(scope.clone()).await.map(Some),
                _ => Ok(None),
            }
        },
        async {
            if !configuration_allowed {
                return Ok(None);
            }
            configuration_counts(state, scope).await
        },
        async {
            if !backups_allowed {
                return Ok(Vec::new());
            }
            state.backups.list(scope.clone()).await
        },
        async {
            match &state.alerts {
                Some(alerts) if alerts_allowed => alerts.rules(scope.clone()).await,
                _ => Ok(Vec::new()),
            }
        },
        async {
            match &state.plugins {
                Some(plugins) if plugins_allowed => plugins
                    .read(scope.clone(), PluginQuery::Plugins)
                    .await
                    .and_then(|output| {
                        serde_json::from_slice::<PluginList>(&output.content).map_err(|_| {
                            PanelError::internal("the plugins module answered an unreadable list")
                        })
                    })
                    .map(|list| list.plugins),
                _ => Ok(Vec::new()),
            }
        },
        async {
            match &state.audit {
                Some(audit) if audit_allowed => audit
                    .list(
                        scope.clone(),
                        AuditFilter {
                            limit: BUNDLE_AUDIT_EVENTS,
                            ..AuditFilter::default()
                        },
                    )
                    .await
                    .map(|page| page.records),
                _ => Ok(Vec::new()),
            }
        },
    );
    let mut problems = Vec::new();
    let gateway = kept(&mut problems, "the gateway's status", gateway)
        .flatten()
        .map(GatewayStatusResponse::from);
    let data_plane = kept(&mut problems, "the gateway's data plane", plane)
        .flatten()
        .map(DataPlaneResponse::from);
    let host = kept(&mut problems, "the host", host)
        .flatten()
        .map(HostSummaryView::from);
    let configuration = kept(&mut problems, "the configuration", configuration).flatten();
    let backups = kept(&mut problems, "the backups", backups)
        .unwrap_or_default()
        .into_iter()
        .take(BUNDLE_BACKUPS)
        .map(BackupDetails::from)
        .collect();
    let alerts = kept(&mut problems, "the alert rules", alerts)
        .unwrap_or_default()
        .into_iter()
        .map(AlertRuleView::from)
        .filter(|rule| {
            !matches!(rule.state, AlertStateName::Inactive) || rule.evaluation_error.is_some()
        })
        .collect();
    let plugins = kept(&mut problems, "the plugins", plugins)
        .unwrap_or_default()
        .into_iter()
        .map(|plugin| PluginDiagnosis {
            name: plugin.name,
            state: plugin.state,
            active_version: plugin.active_version,
            health: plugin.health,
            error: plugin.error,
        })
        .collect();
    let audit = kept(&mut problems, "the audit trail", audit)
        .unwrap_or_default()
        .into_iter()
        .map(AuditEvent::from)
        .collect();
    let health = state
        .health
        .as_ref()
        .and_then(|health| serde_json::to_value(health.current()).ok());
    DiagnosticBundle {
        generated_at: now(),
        versions,
        readiness,
        health,
        gateway,
        data_plane,
        host,
        configuration,
        backups,
        alerts,
        plugins,
        audit,
        withheld,
        problems,
    }
}

async fn configuration_counts<U>(
    state: &ApiState<U>,
    scope: &RequestScope,
) -> PanelResult<Option<ConfigurationDiagnosis>> {
    let Some(configuration) = &state.configuration else {
        return Ok(None);
    };
    let read =
        |query: ModelQuery| configuration.read(scope.clone(), ConfigurationQuery::Model(query));
    let (sites, upstreams, listeners, profiles, revisions) = tokio::join!(
        read(ModelQuery::SiteSummary),
        read(ModelQuery::Upstreams),
        read(ModelQuery::Listeners),
        read(ModelQuery::TlsProfiles),
        recent_revisions(state, scope),
    );
    let sites = sites?;
    let count = |output: PanelResult<panel_config_api::ConfigurationOutput>| -> PanelResult<usize> {
        Ok(items(&output?.content, "items")?.len())
    };
    Ok(Some(ConfigurationDiagnosis {
        draft_version: sites.draft.version,
        applied_version: sites.draft.applied_version,
        sites: serde_json::from_slice(&sites.content).ok(),
        upstreams: count(upstreams)?,
        listeners: count(listeners)?,
        tls_profiles: count(profiles)?,
        revisions: revisions
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .filter(|revision| {
                matches!(
                    revision["outcome"].as_str(),
                    Some("applying" | "failed" | "rejected")
                )
            })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backup(state: BackupState, finished: Option<SystemTime>) -> Backup {
        Backup {
            id: "b".into(),
            contents: Vec::new(),
            site_path: String::new(),
            state,
            requested_by: "admin".into(),
            requested_at: SystemTime::UNIX_EPOCH,
            finished_at: finished,
            size_bytes: 0,
            sha256: String::new(),
            files: 0,
            failure: None,
            product_version: "1.2.3".into(),
        }
    }

    #[test]
    fn a_backup_under_way_holds_an_upgrade_back_and_an_old_one_warns() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 24 * 3600);
        let hour_ago = now - Duration::from_secs(3600);
        let week_ago = now - Duration::from_secs(7 * 24 * 3600);
        let under_way = backup_check(
            Ok(vec![
                backup(BackupState::Running, None),
                backup(BackupState::Completed, Some(hour_ago)),
            ]),
            now,
        );
        assert_eq!(under_way.state, ReadinessState::Fail);
        let recent = backup_check(
            Ok(vec![backup(BackupState::Completed, Some(hour_ago))]),
            now,
        );
        assert_eq!(recent.state, ReadinessState::Pass);
        assert_eq!(recent.detail, "the newest backup is 1 h old");
        let old = backup_check(
            Ok(vec![
                backup(BackupState::Failed, Some(hour_ago)),
                backup(BackupState::Completed, Some(week_ago)),
            ]),
            now,
        );
        assert_eq!(old.state, ReadinessState::Warn);
        assert!(old.detail.starts_with("the newest backup is 7 days old"));
        assert_eq!(
            backup_check(Ok(Vec::new()), now).state,
            ReadinessState::Warn
        );
        assert_eq!(
            backup_check(Err(PanelError::unavailable("down")), now).state,
            ReadinessState::Fail
        );
        assert_eq!(
            backup_check(Err(PanelError::unsupported_capability("none")), now).state,
            ReadinessState::Warn
        );
    }

    #[test]
    fn space_for_a_backup_and_the_migrations_passes() {
        let gib = 1024 * 1024 * 1024;
        let check = |free, databases| space_check(Ok(DataSpace { free, databases })).state;
        assert_eq!(check(10 * gib, gib), ReadinessState::Pass);
        assert_eq!(check(gib + gib / 2, gib), ReadinessState::Warn);
        assert_eq!(check(gib / 2, gib), ReadinessState::Fail);
        assert_eq!(
            space_check(Ok(DataSpace {
                free: 10 * gib,
                databases: gib
            }))
            .detail,
            "10.0 GiB free, the databases take 1.0 GiB"
        );
        assert_eq!(space_check(Err("gone".into())).state, ReadinessState::Warn);
    }

    #[test]
    fn deployment_records_read_as_the_lifecycle_tool_writes_them() {
        let record: DeploymentRecord = serde_json::from_str(
            r#"{"changed_at":"2026-10-08T12:00:00Z","action":"upgrade","engine":"podman",
                "project":"pingora-panel","previous":"0.8.0","images":[
                {"service":"control","image":"ghcr.io/example/pingora-panel:0.9.0",
                 "digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}]}"#,
        )
        .unwrap();
        assert_eq!(record.engine, "podman");
        assert_eq!(record.previous.as_deref(), Some("0.8.0"));
        assert_eq!(record.images[0].service, "control");
    }

    #[test]
    fn ages_read_in_the_largest_whole_unit() {
        assert_eq!(elapsed(Duration::from_secs(59)), "0 min");
        assert_eq!(elapsed(Duration::from_secs(45 * 60)), "45 min");
        assert_eq!(elapsed(Duration::from_secs(30 * 3600)), "30 h");
        assert_eq!(elapsed(Duration::from_secs(3 * 24 * 3600)), "3 days");
    }
}
