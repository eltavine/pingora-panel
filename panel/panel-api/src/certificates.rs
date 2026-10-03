//! The certificate inventory: certificates uploaded or generated here, whose
//! private keys stay with `automation-service`, where they stand in their
//! validity period and which hosts they cover.

use crate::{
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use panel_application::{CertificateChange, CertificateOutput, CertificatePort, CertificateRead};
use panel_certificates::{accept, describe, Certificate, CertificateDetails, CertificateStatus};
use panel_domain::NormalizedHost;
use panel_errors::PanelError;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use zeroize::Zeroizing;

fn port<U>(state: &ApiState<U>) -> Result<Arc<dyn CertificatePort>, ApiError> {
    state.certificates.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "the certificate inventory is not available here",
        ))
    })
}

pub(crate) fn body<T>(payload: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    payload
        .map(|Json(value)| value)
        .map_err(ApiError::from_json)
}

/// A certificate of the inventory and where it stands in its validity period.
#[derive(Serialize, ToSchema)]
pub(crate) struct CertificateView {
    #[serde(flatten)]
    certificate: Certificate,
    status: CertificateStatus,
    /// For `If-Match` when replacing or deleting it.
    etag: String,
}

impl From<Certificate> for CertificateView {
    fn from(certificate: Certificate) -> Self {
        Self {
            status: certificate.details.status(Utc::now()),
            etag: certificate.etag(),
            certificate,
        }
    }
}

/// A new certificate, uploaded with its key or generated.
#[derive(Deserialize, ToSchema)]
#[serde(tag = "source", rename_all = "snake_case")]
pub(crate) enum NewCertificate {
    /// A certificate from elsewhere with its private key.
    Upload {
        /// Lowercase letters, digits and hyphens in labels separated by
        /// dots, such as `example.com`.
        id: String,
        /// PEM certificates, leaf first.
        chain: String,
        /// The leaf's unencrypted PEM private key: PKCS#8, PKCS#1 or SEC1.
        #[schema(value_type = String)]
        key: Zeroizing<String>,
    },
    /// A certificate signed by its own new ECDSA P-256 key.
    SelfSigned {
        id: String,
        /// DNS names, wildcards such as `*.example.com`, or IP addresses.
        names: Vec<String>,
        /// Days the certificate is valid, from 1 to 825.
        days: u32,
    },
}

/// A replacement chain and key, such as a renewed certificate.
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CertificateMaterial {
    /// PEM certificates, leaf first.
    chain: String,
    /// The leaf's unencrypted PEM private key: PKCS#8, PKCS#1 or SEC1.
    #[schema(value_type = String)]
    key: Zeroizing<String>,
}

/// A chain, and optionally its key, to check without storing anything.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CertificateInspection {
    /// PEM certificates, leaf first.
    chain: String,
    /// When given, it must belong to the leaf.
    #[schema(value_type = Option<String>)]
    key: Option<Zeroizing<String>>,
}

/// What a checked chain says, where it stands and whether its key matched.
#[derive(Serialize, ToSchema)]
pub(crate) struct InspectedCertificate {
    #[serde(flatten)]
    details: CertificateDetails,
    status: CertificateStatus,
    /// Whether a key was given and belongs to the leaf.
    key_matches: bool,
}

/// Which hosts a certificate covers.
#[derive(Serialize, ToSchema)]
pub(crate) struct CertificateCoverage {
    status: CertificateStatus,
    not_after: DateTime<Utc>,
    hosts: Vec<HostCoverage>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct HostCoverage {
    /// The host in its ASCII form.
    host: String,
    covered: bool,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct CertificatePath {
    /// Certificate identifier.
    id: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct CoverageQuery {
    /// Comma-separated host names, such as `example.com,www.example.com`.
    hosts: String,
}

pub(crate) async fn read<U>(
    state: &ApiState<U>,
    headers: &HeaderMap,
    operation: &str,
    resource: String,
) -> Result<CertificateOutput, ApiError> {
    Ok(port(state)?
        .read(
            request_scope(headers)?,
            CertificateRead {
                operation: operation.into(),
                resource,
                parameters: Vec::new(),
            },
        )
        .await?)
}

pub(crate) async fn change<U>(
    state: &ApiState<U>,
    headers: &HeaderMap,
    operation: &str,
    resource: String,
    if_match: Option<String>,
    content: Vec<u8>,
) -> Result<CertificateOutput, ApiError> {
    Ok(port(state)?
        .change(
            command_context(headers)?,
            CertificateChange {
                operation: operation.into(),
                resource,
                if_match,
                content,
            },
        )
        .await?)
}

/// The `If-Match` entity tag replacing and deleting need.
fn if_match(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers.get(header::IF_MATCH).ok_or_else(|| {
        ApiError::new(PanelError::precondition_required(
            "send If-Match with the ETag of the certificate being changed",
        ))
    })?;
    value.to_str().map(str::to_owned).map_err(|_| {
        ApiError::new(PanelError::invalid_argument(
            "If-Match must be visible ASCII",
        ))
    })
}

pub(crate) fn decoded<T: DeserializeOwned>(output: &CertificateOutput) -> Result<T, ApiError> {
    serde_json::from_slice(&output.content).map_err(|_| {
        ApiError::new(PanelError::internal(
            "the certificate inventory answered with an unreadable document",
        ))
    })
}

fn certificate(status: StatusCode, output: CertificateOutput) -> Result<Response, ApiError> {
    let view = CertificateView::from(decoded::<Certificate>(&output)?);
    let mut response = (status, Json(view)).into_response();
    if let Some(etag) = output
        .etag
        .and_then(|etag| HeaderValue::from_str(&etag).ok())
    {
        response.headers_mut().insert(header::ETAG, etag);
    }
    Ok(response)
}

fn resource(id: &str) -> String {
    format!("certificates/{id}")
}

/// Every certificate of the inventory.
#[utoipa::path(get, path = "/api/v1/certificates", params(QueryHeaders),
    responses((status = 200, body = Vec<CertificateView>)), tag = "certificates")]
pub(crate) async fn list_certificates<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<Vec<CertificateView>>, ApiError> {
    let output = read(&state, &headers, "certificates.list", "certificates".into()).await?;
    Ok(Json(
        decoded::<Vec<Certificate>>(&output)?
            .into_iter()
            .map(CertificateView::from)
            .collect(),
    ))
}

/// Adds a certificate, uploaded with its key or generated.
#[utoipa::path(post, path = "/api/v1/certificates", request_body = NewCertificate, params(MutationHeaders),
    responses((status = 201, body = CertificateView, headers(("ETag" = String)))), tag = "certificates")]
pub(crate) async fn create_certificate<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<NewCertificate>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (operation, content) = match body(payload)? {
        NewCertificate::Upload { id, chain, key } => (
            "certificates.upload",
            json!({ "id": id, "chain": chain, "key": *key }),
        ),
        NewCertificate::SelfSigned { id, names, days } => (
            "certificates.generate",
            json!({ "id": id, "names": names, "days": days }),
        ),
    };
    let content = Zeroizing::new(content.to_string().into_bytes());
    let output = change(
        &state,
        &headers,
        operation,
        "certificates".into(),
        None,
        content.to_vec(),
    )
    .await?;
    certificate(StatusCode::CREATED, output)
}

/// Reads a certificate.
#[utoipa::path(get, path = "/api/v1/certificates/{id}", params(QueryHeaders, CertificatePath),
    responses((status = 200, body = CertificateView, headers(("ETag" = String)))), tag = "certificates")]
pub(crate) async fn get_certificate<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<CertificatePath>,
) -> Result<Response, ApiError> {
    let output = read(&state, &headers, "certificates.get", resource(&path.id)).await?;
    certificate(StatusCode::OK, output)
}

/// Replaces a certificate's chain and key, for example with a renewed
/// certificate; references to it keep working. `If-Match` must carry its ETag.
#[utoipa::path(put, path = "/api/v1/certificates/{id}", request_body = CertificateMaterial,
    params(MutationHeaders, CertificatePath, ("If-Match" = String, Header, description = "ETag of the certificate")),
    responses((status = 200, body = CertificateView, headers(("ETag" = String)))), tag = "certificates")]
pub(crate) async fn replace_certificate<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<CertificatePath>,
    payload: Result<Json<CertificateMaterial>, JsonRejection>,
) -> Result<Response, ApiError> {
    let expected = if_match(&headers)?;
    let content = Zeroizing::new(
        serde_json::to_vec(&body(payload)?).expect("certificate material serializes"),
    );
    let output = change(
        &state,
        &headers,
        "certificates.replace",
        resource(&path.id),
        Some(expected),
        content.to_vec(),
    )
    .await?;
    certificate(StatusCode::OK, output)
}

/// Deletes a certificate and its files in the gateway's secret directory;
/// `If-Match` must carry its ETag.
#[utoipa::path(delete, path = "/api/v1/certificates/{id}",
    params(MutationHeaders, CertificatePath, ("If-Match" = String, Header, description = "ETag of the certificate")),
    responses((status = 204)), tag = "certificates")]
pub(crate) async fn delete_certificate<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<CertificatePath>,
) -> Result<StatusCode, ApiError> {
    let expected = if_match(&headers)?;
    change(
        &state,
        &headers,
        "certificates.delete",
        resource(&path.id),
        Some(expected),
        Vec::new(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Which of the given hosts a certificate covers, by its subject
/// alternative names as RFC 9525 matches them.
#[utoipa::path(get, path = "/api/v1/certificates/{id}/coverage", params(QueryHeaders, CertificatePath, CoverageQuery),
    responses((status = 200, body = CertificateCoverage)), tag = "certificates")]
pub(crate) async fn certificate_coverage<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<CertificatePath>,
    Query(query): Query<CoverageQuery>,
) -> Result<Json<CertificateCoverage>, ApiError> {
    let hosts = query
        .hosts
        .split(',')
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(|host| {
            NormalizedHost::new(host).map_err(|error| {
                ApiError::new(PanelError::invalid_argument(format!(
                    "{host:?} is not a host name: {error}"
                )))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if hosts.is_empty() {
        return Err(ApiError::new(PanelError::invalid_argument(
            "name at least one host",
        )));
    }
    let output = read(&state, &headers, "certificates.get", resource(&path.id)).await?;
    let certificate: Certificate = decoded(&output)?;
    Ok(Json(CertificateCoverage {
        status: certificate.details.status(Utc::now()),
        not_after: certificate.details.not_after,
        hosts: hosts
            .iter()
            .map(|host| HostCoverage {
                host: host.to_string(),
                covered: certificate.details.covers(host),
            })
            .collect(),
    }))
}

/// Checks a chain, and the key when one is given, as an upload would,
/// without storing anything.
#[utoipa::path(post, path = "/api/v1/certificate-inspections", request_body = CertificateInspection,
    responses((status = 200, body = InspectedCertificate)), tag = "certificates")]
pub(crate) async fn inspect_certificate(
    payload: Result<Json<CertificateInspection>, JsonRejection>,
) -> Result<Json<InspectedCertificate>, ApiError> {
    let inspection = body(payload)?;
    let now = Utc::now();
    let (details, key_matches) = match &inspection.key {
        Some(key) => (accept(&inspection.chain, key, now)?.details, true),
        None => (describe(&inspection.chain)?, false),
    };
    Ok(Json(InspectedCertificate {
        status: details.status(now),
        details,
        key_matches,
    }))
}
