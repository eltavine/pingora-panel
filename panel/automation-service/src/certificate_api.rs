//! The certificate inventory over `pingora.panel.automation.v1.Certificates`.

use crate::certificates::{Cause, CertificateInventory};
use panel_certificates::CertificateId;
use panel_contracts::{
    automation::v1::{self as wire, certificates_server::Certificates},
    common::v1 as common,
};
use panel_errors::{PanelError, Result};
use panel_events::{RequestId, RequestScope};
use panel_service::trace_context;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tonic::{Request, Response, Status};
use zeroize::Zeroizing;

const COLLECTION: &str = "certificates";

pub struct CertificateService {
    inventory: CertificateInventory,
}

impl CertificateService {
    pub fn new(inventory: CertificateInventory) -> Self {
        Self { inventory }
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

/// What a request names: the inventory or one certificate in it.
enum Target {
    Collection,
    Certificate(CertificateId),
}

fn target(resource: &str) -> Result<Target> {
    match resource.split_once('/') {
        None if resource == COLLECTION => Ok(Target::Collection),
        Some((COLLECTION, id)) => Ok(Target::Certificate(CertificateId::new(id)?)),
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

fn etag(version: u64) -> String {
    format!("\"{version}\"")
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
                    Ok((encode(&certificate), etag(certificate.version)))
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
            let cause = Cause {
                scope: &scope,
                actor: &actor,
            };
            let created = |certificate: panel_certificates::Certificate| {
                (encode(&certificate), etag(certificate.version))
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
