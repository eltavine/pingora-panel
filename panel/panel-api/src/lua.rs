//! Lua scripts (ADR 0039): the script library of the draft or of a
//! revision, and tests that run the draft's handlers, or one script, on a
//! request described in full.

use crate::{
    configuration::{change, json, read, Precondition},
    contract::DiagnosticDetails,
    error::ApiError,
    request_context::{MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use panel_application::GatewayUseCases;
use panel_config_api::{LanguageQuery, LuaCommand, LuaTest, LuaTestResult};
use panel_config_dsl::LuaScriptInfo;
use panel_config_model::LuaSharedDict;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// The Lua of the draft or of a revision.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LuaScriptLibrary {
    /// `lua off`: the scripts are kept, but none runs.
    pub disabled: bool,
    /// Every script with where it runs and the SHA-256 that names its
    /// version.
    pub scripts: Vec<LuaScriptInfo>,
    pub shared_dicts: Vec<LuaSharedDict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_limit_bytes: Option<u64>,
    /// What checking found in the scripts and the Lua directives.
    pub diagnostics: Vec<DiagnosticDetails>,
    /// The draft version read.
    pub version: u64,
    /// The revision whose scripts these are, when one was asked for.
    pub revision: Option<u64>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct LuaLibraryQuery {
    /// Reads the scripts of this revision instead of the draft's.
    revision: Option<u64>,
}

/// The Lua scripts of the draft, or of a revision: each with where it runs,
/// the modules it loads and its version, the shared dictionaries, and what
/// checking found.
#[utoipa::path(get, path = "/api/v1/config/lua", params(LuaLibraryQuery, QueryHeaders),
    responses((status = 200, body = LuaScriptLibrary)), tag = "configuration")]
pub(crate) async fn lua_library<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(query): Query<LuaLibraryQuery>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        LanguageQuery::Lua {
            revision: query.revision,
        },
    )
    .await
}

/// Runs the draft's Lua handlers a request reaches, or one script in place
/// of a phase's handler, on a request described in full, with the
/// gateway's runtime and limits and without proxying it. Nothing changes:
/// the test opens no connections and runs no timers. It is recorded in the
/// audit trail.
#[utoipa::path(post, path = "/api/v1/config/lua/test", request_body = LuaTest,
    params(MutationHeaders), responses((status = 200, body = LuaTestResult)),
    tag = "configuration")]
pub(crate) async fn test_lua<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let test = json::<LuaTest>(&headers, &body)?;
    change(
        &state,
        &headers,
        LuaCommand::Test { test },
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}
