//! ACME accounts and automatic certificates: certificates `automation-service`
//! obtains from ACME CAs and renews into the inventory. Account keys never
//! leave it.

use crate::{
    certificates::{body, certificate_id, change, decoded, read},
    error::ApiError,
    request_context::{MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use panel_certificate_api::{
    AccountId, CertificateCommand, CertificateOutput, CertificateQuery, Challenge,
    DnsProviderChange as ProviderChange, DnsProviderKind as ProviderKind, ExternalAccount,
    NewAccount, NewAutomaticCertificate as NewCertificate, NewDnsProvider as NewProvider,
    Rfc2136Config, TsigAlgorithm as Tsig,
};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use zeroize::Zeroizing;

/// An account with an ACME CA.
#[derive(Deserialize, Serialize, ToSchema)]
pub(crate) struct AcmeAccount {
    id: String,
    /// The CA's directory URL.
    directory: String,
    /// PEM roots trusted for a private directory instead of the system's.
    ca_bundle: Option<String>,
    /// Email addresses the CA may write to.
    contact: Vec<String>,
    /// The key identifier of the external account binding it was
    /// registered with.
    external_account_key_id: Option<String>,
    /// The account's URL at the CA.
    url: String,
    version: u64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AcmeAccountView {
    #[serde(flatten)]
    account: AcmeAccount,
    /// For `If-Match` when deleting it.
    etag: String,
}

impl From<AcmeAccount> for AcmeAccountView {
    fn from(account: AcmeAccount) -> Self {
        Self {
            etag: format!("\"{}\"", account.version),
            account,
        }
    }
}

/// The key identifier and MAC key a CA hands out for registering, for CAs
/// that require an external account binding. The MAC key is used once and
/// not kept.
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExternalAccountBinding {
    key_id: String,
    /// Base64url-encoded, as the CA shows it.
    #[schema(value_type = String)]
    mac_key: Zeroizing<String>,
}

/// An account to register with an ACME CA.
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewAcmeAccount {
    /// Lowercase letters, digits and hyphens, such as `letsencrypt`.
    id: String,
    /// The CA's directory URL, such as
    /// `https://acme-v02.api.letsencrypt.org/directory`.
    directory: String,
    /// PEM roots to trust for a private directory instead of the system's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ca_bundle: Option<String>,
    /// Email addresses the CA may write to about the account and its
    /// certificates.
    #[serde(default)]
    contact: Vec<String>,
    /// Registering accepts the CA's terms of service, so this must be true.
    terms_of_service_agreed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    external_account: Option<ExternalAccountBinding>,
}

/// How a CA checks control of a certificate's names.
#[derive(Clone, Copy, Deserialize, Serialize, ToSchema)]
pub(crate) enum AcmeChallenge {
    /// A file the gateway serves on port 80 for every name; not for
    /// wildcard names.
    #[serde(rename = "http-01")]
    Http01,
    /// A TXT record in the DNS zone of every name.
    #[serde(rename = "dns-01")]
    Dns01,
}

/// Where an automatic certificate stands.
#[derive(Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IssuanceState {
    /// Not issued yet.
    Pending,
    /// Issued; renewed when due.
    Issued,
    /// The last attempt failed; another follows after a pause.
    Failing,
}

/// The last failed attempt, in the CA's words where it gave any.
#[derive(Deserialize, Serialize, ToSchema)]
pub(crate) struct IssuanceError {
    code: String,
    message: String,
    at: DateTime<Utc>,
}

/// A certificate the panel obtains from an ACME CA and renews.
#[derive(Deserialize, Serialize, ToSchema)]
pub(crate) struct AutomaticCertificate {
    /// The ID of the inventory certificate it produces.
    id: String,
    /// The ACME account that orders it.
    account: String,
    names: Vec<String>,
    challenge: AcmeChallenge,
    /// The DNS provider that publishes DNS-01 records.
    dns_provider: Option<String>,
    state: IssuanceState,
    /// When it is issued next.
    renew_after: DateTime<Utc>,
    /// The CA's explanation of an unusually early renewal window.
    renewal_explanation_url: Option<String>,
    /// Consecutive failed attempts.
    failures: u32,
    last_error: Option<IssuanceError>,
    version: u64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AutomaticCertificateView {
    #[serde(flatten)]
    certificate: AutomaticCertificate,
    /// For `If-Match` when deleting it.
    etag: String,
}

impl From<AutomaticCertificate> for AutomaticCertificateView {
    fn from(certificate: AutomaticCertificate) -> Self {
        Self {
            etag: format!("\"{}\"", certificate.version),
            certificate,
        }
    }
}

/// A certificate to obtain from an ACME CA and keep renewed.
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewAutomaticCertificate {
    /// The ID of the inventory certificate it produces, such as
    /// `example.com`. A certificate already there under this ID is replaced
    /// by the first issuance.
    id: String,
    /// The ACME account that orders it.
    account: String,
    /// DNS names, wildcards such as `*.example.com`, or IP addresses.
    names: Vec<String>,
    /// `http-01` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    challenge: Option<AcmeChallenge>,
    /// The DNS provider that publishes the records of `dns-01`, which
    /// requires one; wildcard names need `dns-01`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dns_provider: Option<String>,
}

/// The kind of a DNS provider.
#[derive(Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DnsProviderKind {
    /// Dynamic updates (RFC 2136) signed with TSIG (RFC 8945), which BIND,
    /// Knot DNS, PowerDNS and most authoritative servers accept.
    Rfc2136,
}

/// A TSIG algorithm (RFC 8945).
#[derive(Clone, Copy, Deserialize, Serialize, ToSchema)]
pub(crate) enum TsigAlgorithm {
    #[serde(rename = "hmac-sha256")]
    HmacSha256,
    #[serde(rename = "hmac-sha512")]
    HmacSha512,
}

/// Where an RFC 2136 provider sends updates and the key it signs them with.
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rfc2136Settings {
    /// The zones' primary server as `host:port`, usually port 53.
    server: String,
    /// The zones the key may update, such as `example.com`; a record goes
    /// to the longest zone that contains it.
    zones: Vec<String>,
    key_name: String,
    algorithm: TsigAlgorithm,
    /// The TTL of published records; 60 seconds unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ttl: Option<u32>,
}

/// A DNS provider that publishes DNS-01 records. Its secret is never
/// returned.
#[derive(Deserialize, Serialize, ToSchema)]
pub(crate) struct DnsProvider {
    id: String,
    kind: DnsProviderKind,
    rfc2136: Rfc2136Settings,
    /// Seconds to wait for a record to reach every authoritative server.
    propagation_seconds: u32,
    version: u64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct DnsProviderView {
    #[serde(flatten)]
    provider: DnsProvider,
    /// For `If-Match` when changing or deleting it.
    etag: String,
}

impl From<DnsProvider> for DnsProviderView {
    fn from(provider: DnsProvider) -> Self {
        Self {
            etag: format!("\"{}\"", provider.version),
            provider,
        }
    }
}

/// A DNS provider to keep.
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewDnsProvider {
    /// Lowercase letters, digits and hyphens, such as `primary-ns`.
    id: String,
    kind: DnsProviderKind,
    rfc2136: Rfc2136Settings,
    /// The TSIG secret, base64-encoded as in BIND key files.
    #[schema(value_type = String)]
    secret: Zeroizing<String>,
    /// 30 seconds unless given; at most 3600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    propagation_seconds: Option<u32>,
}

/// New settings for a DNS provider; its secret stays unless a new one is
/// given.
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DnsProviderChange {
    rfc2136: Rfc2136Settings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    secret: Option<Zeroizing<String>>,
    /// 30 seconds unless given; at most 3600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    propagation_seconds: Option<u32>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct AcmeAccountPath {
    /// ACME account identifier.
    id: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct AutomaticCertificatePath {
    /// Certificate identifier.
    id: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct DnsProviderPath {
    /// DNS provider identifier.
    id: String,
}

/// The `If-Match` entity tag changing and deleting need.
fn if_match(headers: &HeaderMap, what: &str) -> Result<String, ApiError> {
    let value = headers.get(header::IF_MATCH).ok_or_else(|| {
        ApiError::new(PanelError::precondition_required(format!(
            "send If-Match with the ETag of the {what} being changed"
        )))
    })?;
    value.to_str().map(str::to_owned).map_err(|_| {
        ApiError::new(PanelError::invalid_argument(
            "If-Match must be visible ASCII",
        ))
    })
}

fn tagged<T: Serialize>(status: StatusCode, view: T, output: CertificateOutput) -> Response {
    let mut response = (status, Json(view)).into_response();
    if let Some(etag) = output
        .etag
        .and_then(|etag| HeaderValue::from_str(&etag).ok())
    {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

fn account_id(id: &str) -> Result<AccountId, ApiError> {
    AccountId::new(id).map_err(ApiError::new)
}

impl NewAcmeAccount {
    fn into_account(self) -> Result<NewAccount, ApiError> {
        Ok(NewAccount {
            id: account_id(&self.id)?,
            directory: self.directory,
            ca_bundle: self.ca_bundle,
            contact: self.contact,
            terms_of_service_agreed: self.terms_of_service_agreed,
            external_account: self.external_account.map(|binding| ExternalAccount {
                key_id: binding.key_id,
                mac_key: binding.mac_key.into(),
            }),
        })
    }
}

impl NewAutomaticCertificate {
    fn into_certificate(self) -> Result<NewCertificate, ApiError> {
        Ok(NewCertificate {
            id: certificate_id(&self.id)?,
            account: account_id(&self.account)?,
            names: self.names,
            challenge: match self.challenge {
                None | Some(AcmeChallenge::Http01) => Challenge::Http01,
                Some(AcmeChallenge::Dns01) => Challenge::Dns01,
            },
            dns_provider: self.dns_provider,
        })
    }
}

impl Rfc2136Settings {
    fn into_config(self) -> Rfc2136Config {
        Rfc2136Config {
            server: self.server,
            zones: self.zones,
            key_name: self.key_name,
            algorithm: match self.algorithm {
                TsigAlgorithm::HmacSha256 => Tsig::HmacSha256,
                TsigAlgorithm::HmacSha512 => Tsig::HmacSha512,
            },
            ttl: self.ttl,
        }
    }
}

impl NewDnsProvider {
    fn into_provider(self) -> NewProvider {
        NewProvider {
            id: self.id,
            kind: match self.kind {
                DnsProviderKind::Rfc2136 => ProviderKind::Rfc2136,
            },
            rfc2136: self.rfc2136.into_config(),
            secret: self.secret.into(),
            propagation_seconds: self.propagation_seconds,
        }
    }
}

impl DnsProviderChange {
    fn into_change(self) -> ProviderChange {
        ProviderChange {
            rfc2136: self.rfc2136.into_config(),
            secret: self.secret.map(Into::into),
            propagation_seconds: self.propagation_seconds,
        }
    }
}

/// Every ACME account.
#[utoipa::path(get, path = "/api/v1/acme-accounts", params(QueryHeaders),
    responses((status = 200, body = Vec<AcmeAccountView>)), tag = "certificates")]
pub(crate) async fn list_acme_accounts<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<Vec<AcmeAccountView>>, ApiError> {
    let output = read(&state, &headers, CertificateQuery::Accounts).await?;
    Ok(Json(
        decoded::<Vec<AcmeAccount>>(&output)?
            .into_iter()
            .map(AcmeAccountView::from)
            .collect(),
    ))
}

/// Registers an account with an ACME CA.
#[utoipa::path(post, path = "/api/v1/acme-accounts", request_body = NewAcmeAccount, params(MutationHeaders),
    responses((status = 201, body = AcmeAccountView, headers(("ETag" = String)))), tag = "certificates")]
pub(crate) async fn create_acme_account<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<NewAcmeAccount>, JsonRejection>,
) -> Result<Response, ApiError> {
    let output = change(
        &state,
        &headers,
        CertificateCommand::CreateAccount {
            account: body(payload)?.into_account()?,
        },
        None,
    )
    .await?;
    let view = AcmeAccountView::from(decoded::<AcmeAccount>(&output)?);
    Ok(tagged(StatusCode::CREATED, view, output))
}

/// Reads an ACME account.
#[utoipa::path(get, path = "/api/v1/acme-accounts/{id}", params(QueryHeaders, AcmeAccountPath),
    responses((status = 200, body = AcmeAccountView, headers(("ETag" = String)))), tag = "certificates")]
pub(crate) async fn get_acme_account<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<AcmeAccountPath>,
) -> Result<Response, ApiError> {
    let output = read(
        &state,
        &headers,
        CertificateQuery::Account {
            id: account_id(&path.id)?,
        },
    )
    .await?;
    let view = AcmeAccountView::from(decoded::<AcmeAccount>(&output)?);
    Ok(tagged(StatusCode::OK, view, output))
}

/// Forgets an ACME account that no automatic certificate uses any more;
/// `If-Match` must carry its ETag.
#[utoipa::path(delete, path = "/api/v1/acme-accounts/{id}",
    params(MutationHeaders, AcmeAccountPath, ("If-Match" = String, Header, description = "ETag of the account")),
    responses((status = 204)), tag = "certificates")]
pub(crate) async fn delete_acme_account<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<AcmeAccountPath>,
) -> Result<StatusCode, ApiError> {
    let expected = if_match(&headers, "ACME account")?;
    change(
        &state,
        &headers,
        CertificateCommand::DeleteAccount {
            id: account_id(&path.id)?,
        },
        Some(expected),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every automatic certificate.
#[utoipa::path(get, path = "/api/v1/acme-certificates", params(QueryHeaders),
    responses((status = 200, body = Vec<AutomaticCertificateView>)), tag = "certificates")]
pub(crate) async fn list_automatic_certificates<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<Vec<AutomaticCertificateView>>, ApiError> {
    let output = read(&state, &headers, CertificateQuery::AutomaticCertificates).await?;
    Ok(Json(
        decoded::<Vec<AutomaticCertificate>>(&output)?
            .into_iter()
            .map(AutomaticCertificateView::from)
            .collect(),
    ))
}

/// Starts obtaining a certificate from an ACME CA and keeping it renewed.
/// The first issuance runs in the background; the certificate stays
/// `pending` until it is in the inventory.
#[utoipa::path(post, path = "/api/v1/acme-certificates", request_body = NewAutomaticCertificate, params(MutationHeaders),
    responses((status = 201, body = AutomaticCertificateView, headers(("ETag" = String)))), tag = "certificates")]
pub(crate) async fn create_automatic_certificate<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<NewAutomaticCertificate>, JsonRejection>,
) -> Result<Response, ApiError> {
    let output = change(
        &state,
        &headers,
        CertificateCommand::CreateAutomaticCertificate {
            certificate: body(payload)?.into_certificate()?,
        },
        None,
    )
    .await?;
    let view = AutomaticCertificateView::from(decoded::<AutomaticCertificate>(&output)?);
    Ok(tagged(StatusCode::CREATED, view, output))
}

/// Reads an automatic certificate.
#[utoipa::path(get, path = "/api/v1/acme-certificates/{id}", params(QueryHeaders, AutomaticCertificatePath),
    responses((status = 200, body = AutomaticCertificateView, headers(("ETag" = String)))), tag = "certificates")]
pub(crate) async fn get_automatic_certificate<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<AutomaticCertificatePath>,
) -> Result<Response, ApiError> {
    let output = read(
        &state,
        &headers,
        CertificateQuery::AutomaticCertificate {
            id: certificate_id(&path.id)?,
        },
    )
    .await?;
    let view = AutomaticCertificateView::from(decoded::<AutomaticCertificate>(&output)?);
    Ok(tagged(StatusCode::OK, view, output))
}

/// Issues an automatic certificate again now, in the background, for
/// example after its names' DNS was fixed.
#[utoipa::path(post, path = "/api/v1/acme-certificates/{id}/renewals", params(MutationHeaders, AutomaticCertificatePath),
    responses((status = 202, body = AutomaticCertificateView)), tag = "certificates")]
pub(crate) async fn renew_automatic_certificate<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<AutomaticCertificatePath>,
) -> Result<Response, ApiError> {
    let output = change(
        &state,
        &headers,
        CertificateCommand::RenewAutomaticCertificate {
            id: certificate_id(&path.id)?,
        },
        None,
    )
    .await?;
    let view = AutomaticCertificateView::from(decoded::<AutomaticCertificate>(&output)?);
    Ok(tagged(StatusCode::ACCEPTED, view, output))
}

/// Stops renewing a certificate; the certificate stays in the inventory.
/// `If-Match` must carry its ETag.
#[utoipa::path(delete, path = "/api/v1/acme-certificates/{id}",
    params(MutationHeaders, AutomaticCertificatePath, ("If-Match" = String, Header, description = "ETag of the automatic certificate")),
    responses((status = 204)), tag = "certificates")]
pub(crate) async fn delete_automatic_certificate<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<AutomaticCertificatePath>,
) -> Result<StatusCode, ApiError> {
    let expected = if_match(&headers, "automatic certificate")?;
    change(
        &state,
        &headers,
        CertificateCommand::DeleteAutomaticCertificate {
            id: certificate_id(&path.id)?,
        },
        Some(expected),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every DNS provider.
#[utoipa::path(get, path = "/api/v1/dns-providers", params(QueryHeaders),
    responses((status = 200, body = Vec<DnsProviderView>)), tag = "certificates")]
pub(crate) async fn list_dns_providers<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<Vec<DnsProviderView>>, ApiError> {
    let output = read(&state, &headers, CertificateQuery::DnsProviders).await?;
    Ok(Json(
        decoded::<Vec<DnsProvider>>(&output)?
            .into_iter()
            .map(DnsProviderView::from)
            .collect(),
    ))
}

/// Keeps a DNS provider, after checking its settings and secret.
#[utoipa::path(post, path = "/api/v1/dns-providers", request_body = NewDnsProvider, params(MutationHeaders),
    responses((status = 201, body = DnsProviderView, headers(("ETag" = String)))), tag = "certificates")]
pub(crate) async fn create_dns_provider<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<NewDnsProvider>, JsonRejection>,
) -> Result<Response, ApiError> {
    let output = change(
        &state,
        &headers,
        CertificateCommand::CreateDnsProvider {
            provider: body(payload)?.into_provider(),
        },
        None,
    )
    .await?;
    let view = DnsProviderView::from(decoded::<DnsProvider>(&output)?);
    Ok(tagged(StatusCode::CREATED, view, output))
}

/// Reads a DNS provider.
#[utoipa::path(get, path = "/api/v1/dns-providers/{id}", params(QueryHeaders, DnsProviderPath),
    responses((status = 200, body = DnsProviderView, headers(("ETag" = String)))), tag = "certificates")]
pub(crate) async fn get_dns_provider<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<DnsProviderPath>,
) -> Result<Response, ApiError> {
    let output = read(
        &state,
        &headers,
        CertificateQuery::DnsProvider { id: path.id },
    )
    .await?;
    let view = DnsProviderView::from(decoded::<DnsProvider>(&output)?);
    Ok(tagged(StatusCode::OK, view, output))
}

/// Changes a DNS provider's settings, and its secret when one is given;
/// `If-Match` must carry its ETag.
#[utoipa::path(put, path = "/api/v1/dns-providers/{id}", request_body = DnsProviderChange,
    params(MutationHeaders, DnsProviderPath, ("If-Match" = String, Header, description = "ETag of the DNS provider")),
    responses((status = 200, body = DnsProviderView, headers(("ETag" = String)))), tag = "certificates")]
pub(crate) async fn update_dns_provider<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<DnsProviderPath>,
    payload: Result<Json<DnsProviderChange>, JsonRejection>,
) -> Result<Response, ApiError> {
    let expected = if_match(&headers, "DNS provider")?;
    let output = change(
        &state,
        &headers,
        CertificateCommand::UpdateDnsProvider {
            id: path.id,
            change: body(payload)?.into_change(),
        },
        Some(expected),
    )
    .await?;
    let view = DnsProviderView::from(decoded::<DnsProvider>(&output)?);
    Ok(tagged(StatusCode::OK, view, output))
}

/// Forgets a DNS provider no automatic certificate uses; `If-Match` must
/// carry its ETag.
#[utoipa::path(delete, path = "/api/v1/dns-providers/{id}",
    params(MutationHeaders, DnsProviderPath, ("If-Match" = String, Header, description = "ETag of the DNS provider")),
    responses((status = 204)), tag = "certificates")]
pub(crate) async fn delete_dns_provider<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<DnsProviderPath>,
) -> Result<StatusCode, ApiError> {
    let expected = if_match(&headers, "DNS provider")?;
    change(
        &state,
        &headers,
        CertificateCommand::DeleteDnsProvider { id: path.id },
        Some(expected),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
