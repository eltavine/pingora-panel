//! The certificate inventory, ACME accounts and automatic certificates over
//! `pingora.panel.automation.v1.Certificates`.

use crate::{
    acme::{AccountId, AcmeAutomation, NewAutomaticCertificate},
    certificates::{Cause, CertificateInventory},
    dns::{DnsProviderChange, DnsProviders, NewDnsProvider},
};
use panel_certificates::CertificateId;
use panel_contracts::{
    automation::v1::{self as wire, certificates_server::Certificates},
    common::v1 as common,
};
use panel_errors::{PanelError, Result};
use panel_events::{RequestId, RequestScope};
use panel_postgres::EventLog;
use panel_service::trace_context;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tonic::{Request, Response, Status};
use zeroize::Zeroizing;

const COLLECTION: &str = "certificates";
const ACCOUNTS: &str = "acme-accounts";
const AUTOMATIC: &str = "acme-certificates";
const DNS_PROVIDERS: &str = "dns-providers";

pub struct CertificateService {
    inventory: CertificateInventory,
    acme: AcmeAutomation,
    dns: DnsProviders,
}

impl CertificateService {
    pub fn new(inventory: CertificateInventory, acme: AcmeAutomation, dns: DnsProviders) -> Self {
        Self {
            inventory,
            acme,
            dns,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadBody {
    id: CertificateId,
    chain: String,
    key: Zeroizing<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceBody {
    chain: String,
    key: Zeroizing<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerateBody {
    id: CertificateId,
    names: Vec<String>,
    days: u32,
}

/// What a request names: the inventory, ACME accounts or automatic
/// certificates, or one of them.
enum Target {
    Collection,
    Certificate(CertificateId),
    Accounts,
    Account(AccountId),
    Automatic,
    AutomaticCertificate(CertificateId),
    DnsProviders,
    DnsProvider(String),
}

fn certificate_id(id: &str) -> Result<CertificateId> {
    CertificateId::new(id).map_err(|error| PanelError::invalid_argument(error.to_string()))
}

fn target(resource: &str) -> Result<Target> {
    match resource.split_once('/') {
        None if resource == COLLECTION => Ok(Target::Collection),
        None if resource == ACCOUNTS => Ok(Target::Accounts),
        None if resource == AUTOMATIC => Ok(Target::Automatic),
        None if resource == DNS_PROVIDERS => Ok(Target::DnsProviders),
        Some((COLLECTION, id)) => certificate_id(id).map(Target::Certificate),
        Some((ACCOUNTS, id)) => AccountId::new(id).map(Target::Account),
        Some((AUTOMATIC, id)) => certificate_id(id).map(Target::AutomaticCertificate),
        Some((DNS_PROVIDERS, id)) => Ok(Target::DnsProvider(id.to_owned())),
        _ => Err(PanelError::invalid_argument(format!(
            "unknown resource {resource:?}"
        ))),
    }
}

fn unknown(operation: &str) -> PanelError {
    PanelError::invalid_argument(format!("unknown operation {operation:?} for this resource"))
}

fn scope(context: Option<common::RequestContext>) -> Result<(RequestScope, String)> {
    let context =
        context.ok_or_else(|| PanelError::invalid_argument("request context is required"))?;
    let scope = RequestScope::new(RequestId::new(context.request_id)?);
    let scope = if context.correlation_id.is_empty() {
        scope
    } else {
        scope.with_correlation_id(RequestId::new(context.correlation_id)?)
    };
    Ok((scope, context.actor))
}

fn decode<T: DeserializeOwned>(content: &[u8]) -> Result<T> {
    serde_json::from_slice(content)
        .map_err(|error| PanelError::invalid_argument(format!("invalid request body: {error}")))
}

fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("API values serialize")
}

/// The version an `If-Match` entity tag names; empty means unconditional.
fn expected(if_match: &str) -> Result<Option<u64>> {
    if if_match.is_empty() {
        return Ok(None);
    }
    if_match
        .trim_matches('"')
        .parse()
        .map(Some)
        .map_err(|_| PanelError::precondition_failed(format!("unknown entity tag {if_match}")))
}

#[tonic::async_trait]
impl Certificates for CertificateService {
    async fn read(
        &self,
        request: Request<wire::ReadRequest>,
    ) -> std::result::Result<Response<wire::ReadResponse>, Status> {
        let request = request.into_inner();
        let result: Result<(Vec<u8>, String)> = async {
            scope(request.context)?;
            match (request.operation.as_str(), target(&request.resource)?) {
                ("certificates.list", Target::Collection) => {
                    Ok((encode(&self.inventory.list().await?), String::new()))
                }
                ("certificates.get", Target::Certificate(id)) => {
                    let certificate = self.inventory.get(&id).await?;
                    Ok((encode(&certificate), certificate.etag()))
                }
                ("acme.accounts.list", Target::Accounts) => {
                    Ok((encode(&self.acme.accounts().await?), String::new()))
                }
                ("acme.accounts.get", Target::Account(id)) => {
                    let account = self.acme.account(&id).await?;
                    Ok((encode(&account), account.etag()))
                }
                ("acme.certificates.list", Target::Automatic) => {
                    Ok((encode(&self.acme.certificates().await?), String::new()))
                }
                ("acme.certificates.get", Target::AutomaticCertificate(id)) => {
                    let automatic = self.acme.certificate(&id).await?;
                    Ok((encode(&automatic), automatic.etag()))
                }
                ("acme.dns_providers.list", Target::DnsProviders) => {
                    Ok((encode(&self.dns.list().await?), String::new()))
                }
                ("acme.dns_providers.get", Target::DnsProvider(id)) => {
                    let provider = self.dns.get(&id).await?;
                    Ok((encode(&provider), provider.etag()))
                }
                (operation, _) => Err(unknown(operation)),
            }
        }
        .await;
        Ok(Response::new(match result {
            Ok((content, etag)) => wire::ReadResponse {
                content,
                etag,
                error: None,
            },
            Err(error) => wire::ReadResponse {
                error: Some((&error).into()),
                ..wire::ReadResponse::default()
            },
        }))
    }

    async fn change(
        &self,
        request: Request<wire::ChangeRequest>,
    ) -> std::result::Result<Response<wire::ChangeResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let content = Zeroizing::new(request.content);
        let result: Result<(Vec<u8>, String)> = async {
            let (scope, actor) = scope(request.context)?;
            let scope = scope.with_trace_context(trace);
            let principal = EventLog::user(&actor);
            let cause = Cause {
                scope: &scope,
                principal: &principal,
            };
            let created = |certificate: panel_certificates::Certificate| {
                (encode(&certificate), certificate.etag())
            };
            match (request.operation.as_str(), target(&request.resource)?) {
                ("certificates.upload", Target::Collection) => {
                    let body: UploadBody = decode(&content)?;
                    Ok(created(
                        self.inventory
                            .upload(cause, body.id, &body.chain, &body.key)
                            .await?,
                    ))
                }
                ("certificates.generate", Target::Collection) => {
                    let body: GenerateBody = decode(&content)?;
                    Ok(created(
                        self.inventory
                            .generate(cause, body.id, &body.names, body.days)
                            .await?,
                    ))
                }
                ("certificates.replace", Target::Certificate(id)) => {
                    let body: ReplaceBody = decode(&content)?;
                    Ok(created(
                        self.inventory
                            .replace(
                                cause,
                                id,
                                expected(&request.if_match)?,
                                &body.chain,
                                &body.key,
                            )
                            .await?,
                    ))
                }
                ("certificates.delete", Target::Certificate(id)) => {
                    self.inventory
                        .delete(cause, id, expected(&request.if_match)?)
                        .await?;
                    Ok((Vec::new(), String::new()))
                }
                ("acme.accounts.create", Target::Accounts) => {
                    let account = self.acme.create_account(cause, decode(&content)?).await?;
                    Ok((encode(&account), account.etag()))
                }
                ("acme.accounts.delete", Target::Account(id)) => {
                    self.acme
                        .delete_account(cause, id, expected(&request.if_match)?)
                        .await?;
                    Ok((Vec::new(), String::new()))
                }
                ("acme.certificates.create", Target::Automatic) => {
                    let body: NewAutomaticCertificate = decode(&content)?;
                    let automatic = self.acme.create_certificate(cause, body).await?;
                    Ok((encode(&automatic), automatic.etag()))
                }
                ("acme.certificates.renew", Target::AutomaticCertificate(id)) => {
                    let automatic = self.acme.renew(cause, id).await?;
                    Ok((encode(&automatic), automatic.etag()))
                }
                ("acme.certificates.delete", Target::AutomaticCertificate(id)) => {
                    self.acme
                        .delete_certificate(cause, id, expected(&request.if_match)?)
                        .await?;
                    Ok((Vec::new(), String::new()))
                }
                ("acme.dns_providers.create", Target::DnsProviders) => {
                    let body: NewDnsProvider = decode(&content)?;
                    let provider = self.dns.create(cause, body).await?;
                    Ok((encode(&provider), provider.etag()))
                }
                ("acme.dns_providers.update", Target::DnsProvider(id)) => {
                    let body: DnsProviderChange = decode(&content)?;
                    let provider = self
                        .dns
                        .update(cause, &id, expected(&request.if_match)?, body)
                        .await?;
                    Ok((encode(&provider), provider.etag()))
                }
                ("acme.dns_providers.delete", Target::DnsProvider(id)) => {
                    self.dns
                        .delete(cause, &id, expected(&request.if_match)?)
                        .await?;
                    Ok((Vec::new(), String::new()))
                }
                (operation, _) => Err(unknown(operation)),
            }
        }
        .await;
        Ok(Response::new(match result {
            Ok((content, etag)) => wire::ChangeResponse {
                content,
                etag,
                error: None,
            },
            Err(error) => wire::ChangeResponse {
                error: Some((&error).into()),
                ..wire::ChangeResponse::default()
            },
        }))
    }
}
