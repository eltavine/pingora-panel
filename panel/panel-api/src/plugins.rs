//! External plugins (ADR 0044): the versions found in the plugins
//! directory, the capabilities, settings and limits administrators give
//! them, the publisher keys the host trusts and the secrets settings name,
//! all kept by the plugins module.

use crate::{
    certificates::body,
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    body::Bytes,
    extract::{rejection::JsonRejection, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use panel_errors::PanelError;
use panel_plugin_api::{
    NewTrustedKey, PluginChange, PluginCommand, PluginLimits, PluginList, PluginOutput,
    PluginQuery, PluginView, PluginsPort, Secret, SecretView, TrustedKeyView,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

fn port<U>(state: &ApiState<U>) -> Result<Arc<dyn PluginsPort>, ApiError> {
    state
        .plugins
        .clone()
        .ok_or_else(|| ApiError::new(PanelError::unavailable("plugins are not available here")))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct PluginPath {
    /// The plugin's name.
    name: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct KeyPath {
    /// The trusted key's ID.
    id: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct SecretPath {
    /// The secret's name, which settings name as `vault:<name>`.
    name: String,
}

/// The capabilities to grant, in place of those granted before.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginGrants {
    /// Among those the plugin asks for: the ports it provides, and
    /// `secret-references` to receive the secrets its settings name.
    capabilities: Vec<String>,
}

/// The version to start; the newest validated one when absent.
#[derive(Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginEnable {
    #[serde(default)]
    version: Option<String>,
}

/// The version to run in place of the active one.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginUpgrade {
    version: String,
}

/// A secret's value; it is sealed and never returned.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SecretValue {
    #[schema(value_type = String)]
    value: Secret,
}

async fn read<U>(
    state: &ApiState<U>,
    headers: &HeaderMap,
    query: PluginQuery,
) -> Result<PluginOutput, ApiError> {
    Ok(port(state)?.read(request_scope(headers)?, query).await?)
}

async fn change<U>(
    state: &ApiState<U>,
    headers: &HeaderMap,
    command: PluginCommand,
) -> Result<PluginOutput, ApiError> {
    let if_match = headers
        .get(header::IF_MATCH)
        .map(|value| {
            value.to_str().map(str::to_owned).map_err(|_| {
                ApiError::new(PanelError::invalid_argument(
                    "If-Match must be visible ASCII",
                ))
            })
        })
        .transpose()?;
    Ok(port(state)?
        .change(
            command_context(headers)?,
            PluginChange { command, if_match },
        )
        .await?)
}

fn decoded<T: DeserializeOwned>(output: &PluginOutput) -> Result<T, ApiError> {
    serde_json::from_slice(&output.content).map_err(|_| {
        ApiError::new(PanelError::internal(
            "the plugins module answered with an unreadable document",
        ))
    })
}

/// A JSON body that may be left out, as `T`'s default.
fn optional<T: DeserializeOwned + Default>(bytes: &Bytes) -> Result<T, ApiError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes).map_err(|error| {
        ApiError::new(PanelError::invalid_argument(format!(
            "the request body cannot be read: {error}"
        )))
    })
}

fn plugin(output: PluginOutput) -> Result<Response, ApiError> {
    let view: PluginView = decoded(&output)?;
    let mut response = Json(view).into_response();
    if let Some(etag) = output
        .etag
        .and_then(|etag| HeaderValue::from_str(&etag).ok())
    {
        response.headers_mut().insert(header::ETAG, etag);
    }
    Ok(response)
}

async fn plugin_change<U>(
    state: &ApiState<U>,
    headers: &HeaderMap,
    command: PluginCommand,
) -> Result<Response, ApiError> {
    plugin(change(state, headers, command).await?)
}

/// Every plugin found or configured, with the protocol versions, ports and
/// capabilities the host offers and when the plugins directory was read.
#[utoipa::path(get, path = "/api/v1/plugins", params(QueryHeaders),
    responses((status = 200, body = PluginList)), tag = "plugins")]
pub(crate) async fn list_plugins<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<PluginList>, ApiError> {
    let output = read(&state, &headers, PluginQuery::Plugins).await?;
    Ok(Json(decoded(&output)?))
}

/// Reads the plugins directory again, checking every version's manifest,
/// signature and protocol versions.
#[utoipa::path(post, path = "/api/v1/plugins/discover", params(MutationHeaders),
    responses((status = 200, body = PluginList)), tag = "plugins")]
pub(crate) async fn discover_plugins<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<PluginList>, ApiError> {
    let output = change(&state, &headers, PluginCommand::Discover).await?;
    Ok(Json(decoded(&output)?))
}

/// Reads a plugin: its versions, grants, settings, limits and health.
#[utoipa::path(get, path = "/api/v1/plugins/{name}", params(QueryHeaders, PluginPath),
    responses((status = 200, body = PluginView, headers(("ETag" = String)))), tag = "plugins")]
pub(crate) async fn get_plugin<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PluginPath>,
) -> Result<Response, ApiError> {
    plugin(read(&state, &headers, PluginQuery::Plugin { name: path.name }).await?)
}

/// Grants capabilities in place of those granted before; a running plugin
/// restarts with them, and keeps running as it was when it does not start.
#[utoipa::path(put, path = "/api/v1/plugins/{name}/grants", request_body = PluginGrants,
    params(MutationHeaders, PluginPath, ("If-Match" = Option<String>, Header, description = "ETag of the plugin")),
    responses((status = 200, body = PluginView, headers(("ETag" = String)))), tag = "plugins")]
pub(crate) async fn grant_plugin<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PluginPath>,
    payload: Result<Json<PluginGrants>, JsonRejection>,
) -> Result<Response, ApiError> {
    let grants = body(payload)?;
    let command = PluginCommand::Grant {
        name: path.name,
        capabilities: grants.capabilities,
    };
    plugin_change(&state, &headers, command).await
}

/// Replaces the settings, which the version's JSON Schema must accept;
/// secrets are named by reference, as `vault:<name>` or `<plugin>:<path>`.
#[utoipa::path(put, path = "/api/v1/plugins/{name}/settings", request_body = Object,
    params(MutationHeaders, PluginPath, ("If-Match" = Option<String>, Header, description = "ETag of the plugin")),
    responses((status = 200, body = PluginView, headers(("ETag" = String)))), tag = "plugins")]
pub(crate) async fn configure_plugin<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PluginPath>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    let settings = body(payload)?;
    let command = PluginCommand::Configure {
        name: path.name,
        settings,
    };
    plugin_change(&state, &headers, command).await
}

/// Sets the plugin's resource limits; zero leaves one to its manifest or
/// the host's default.
#[utoipa::path(put, path = "/api/v1/plugins/{name}/limits", request_body = PluginLimits,
    params(MutationHeaders, PluginPath, ("If-Match" = Option<String>, Header, description = "ETag of the plugin")),
    responses((status = 200, body = PluginView, headers(("ETag" = String)))), tag = "plugins")]
pub(crate) async fn limit_plugin<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PluginPath>,
    payload: Result<Json<PluginLimits>, JsonRejection>,
) -> Result<Response, ApiError> {
    let limits = body(payload)?;
    let command = PluginCommand::Limit {
        name: path.name,
        limits,
    };
    plugin_change(&state, &headers, command).await
}

/// Starts the plugin at the version given, or its newest validated one.
#[utoipa::path(post, path = "/api/v1/plugins/{name}/enable", request_body(content = Option<PluginEnable>),
    params(MutationHeaders, PluginPath, ("If-Match" = Option<String>, Header, description = "ETag of the plugin")),
    responses((status = 200, body = PluginView, headers(("ETag" = String)))), tag = "plugins")]
pub(crate) async fn enable_plugin<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PluginPath>,
    payload: Bytes,
) -> Result<Response, ApiError> {
    let enable: PluginEnable = optional(&payload)?;
    let command = PluginCommand::Enable {
        name: path.name,
        version: enable.version,
    };
    plugin_change(&state, &headers, command).await
}

/// Stops the plugin; its grants, settings and versions are kept.
#[utoipa::path(post, path = "/api/v1/plugins/{name}/disable",
    params(MutationHeaders, PluginPath, ("If-Match" = Option<String>, Header, description = "ETag of the plugin")),
    responses((status = 200, body = PluginView, headers(("ETag" = String)))), tag = "plugins")]
pub(crate) async fn disable_plugin<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PluginPath>,
) -> Result<Response, ApiError> {
    plugin_change(&state, &headers, PluginCommand::Disable { name: path.name }).await
}

/// Runs another validated version in place of the active one, which a
/// rollback returns to; when it does not start, the active one keeps
/// running.
#[utoipa::path(post, path = "/api/v1/plugins/{name}/upgrade", request_body = PluginUpgrade,
    params(MutationHeaders, PluginPath, ("If-Match" = Option<String>, Header, description = "ETag of the plugin")),
    responses((status = 200, body = PluginView, headers(("ETag" = String)))), tag = "plugins")]
pub(crate) async fn upgrade_plugin<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PluginPath>,
    payload: Result<Json<PluginUpgrade>, JsonRejection>,
) -> Result<Response, ApiError> {
    let upgrade = body(payload)?;
    let command = PluginCommand::Upgrade {
        name: path.name,
        version: upgrade.version,
    };
    plugin_change(&state, &headers, command).await
}

/// Runs the version that ran before the active one.
#[utoipa::path(post, path = "/api/v1/plugins/{name}/rollback",
    params(MutationHeaders, PluginPath, ("If-Match" = Option<String>, Header, description = "ETag of the plugin")),
    responses((status = 200, body = PluginView, headers(("ETag" = String)))), tag = "plugins")]
pub(crate) async fn rollback_plugin<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PluginPath>,
) -> Result<Response, ApiError> {
    plugin_change(
        &state,
        &headers,
        PluginCommand::Rollback { name: path.name },
    )
    .await
}

/// The publisher keys whose signatures the host accepts.
#[utoipa::path(get, path = "/api/v1/plugin-keys", params(QueryHeaders),
    responses((status = 200, body = Vec<TrustedKeyView>)), tag = "plugins")]
pub(crate) async fn list_plugin_keys<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<Vec<TrustedKeyView>>, ApiError> {
    let output = read(&state, &headers, PluginQuery::Keys).await?;
    Ok(Json(decoded(&output)?))
}

/// Trusts a publisher's minisign key; the plugins directory is read again.
#[utoipa::path(post, path = "/api/v1/plugin-keys", request_body = NewTrustedKey, params(MutationHeaders),
    responses((status = 201, body = TrustedKeyView)), tag = "plugins")]
pub(crate) async fn trust_plugin_key<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<NewTrustedKey>, JsonRejection>,
) -> Result<Response, ApiError> {
    let key = body(payload)?;
    let output = change(&state, &headers, PluginCommand::PutKey { key }).await?;
    let trusted: TrustedKeyView = decoded(&output)?;
    Ok((StatusCode::CREATED, Json(trusted)).into_response())
}

/// No longer trusts a key; refused while it signs an enabled plugin.
#[utoipa::path(delete, path = "/api/v1/plugin-keys/{id}", params(MutationHeaders, KeyPath),
    responses((status = 204)), tag = "plugins")]
pub(crate) async fn delete_plugin_key<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<KeyPath>,
) -> Result<StatusCode, ApiError> {
    change(&state, &headers, PluginCommand::DeleteKey { id: path.id }).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The names of the secrets kept for plugins' settings.
#[utoipa::path(get, path = "/api/v1/plugin-secrets", params(QueryHeaders),
    responses((status = 200, body = Vec<SecretView>)), tag = "plugins")]
pub(crate) async fn list_plugin_secrets<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<Vec<SecretView>>, ApiError> {
    let output = read(&state, &headers, PluginQuery::Secrets).await?;
    Ok(Json(decoded(&output)?))
}

/// Seals a secret for settings to name as `vault:<name>`; plugins receive
/// a new value when they next start.
#[utoipa::path(put, path = "/api/v1/plugin-secrets/{name}", request_body = SecretValue,
    params(MutationHeaders, SecretPath), responses((status = 200, body = SecretView)), tag = "plugins")]
pub(crate) async fn put_plugin_secret<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SecretPath>,
    payload: Result<Json<SecretValue>, JsonRejection>,
) -> Result<Json<SecretView>, ApiError> {
    let secret = body(payload)?;
    let command = PluginCommand::PutSecret {
        name: path.name,
        value: secret.value,
    };
    let output = change(&state, &headers, command).await?;
    Ok(Json(decoded(&output)?))
}

/// Deletes a secret no plugin's settings name.
#[utoipa::path(delete, path = "/api/v1/plugin-secrets/{name}", params(MutationHeaders, SecretPath),
    responses((status = 204)), tag = "plugins")]
pub(crate) async fn delete_plugin_secret<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SecretPath>,
) -> Result<StatusCode, ApiError> {
    change(
        &state,
        &headers,
        PluginCommand::DeleteSecret { name: path.name },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
