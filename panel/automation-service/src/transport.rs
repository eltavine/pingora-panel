//! The certificate API over gRPC: requests decode into the certificate
//! port's terms and what it answers encodes back, so the inventory and ACME
//! automation behind the port know nothing of the transport.
//!
//! Application failures travel in each response's `error` field; transport
//! status codes are left to the transport.

use panel_certificate_api::{CertificateChange, CertificatePort};
use panel_contracts::automation::v1::{self as wire, certificates_server::Certificates};
use panel_errors::{PanelError, Result};
use panel_service::{decode_command, decode_scope, trace_context};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use zeroize::Zeroizing;

/// Serves a certificate port over gRPC.
pub struct CertificatesTransport {
    port: Arc<dyn CertificatePort>,
}

impl CertificatesTransport {
    pub fn new(port: Arc<dyn CertificatePort>) -> Self {
        Self { port }
    }
}

fn decoded<T: DeserializeOwned>(value: &[u8], what: &str) -> Result<T> {
    serde_json::from_slice(value).map_err(|error| {
        PanelError::invalid_argument(format!("the certificate {what} cannot be read: {error}"))
    })
}

#[tonic::async_trait]
impl Certificates for CertificatesTransport {
    async fn read(
        &self,
        request: Request<wire::ReadRequest>,
    ) -> std::result::Result<Response<wire::ReadResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let result = async {
            let scope = decode_scope(request.context, trace)?;
            self.port
                .read(scope, decoded(&request.query, "query")?)
                .await
        }
        .await;
        Ok(Response::new(match result {
            Ok(output) => wire::ReadResponse {
                content: output.content,
                etag: output.etag.unwrap_or_default(),
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
        let encoded = Zeroizing::new(request.command);
        let result = async {
            let context = decode_command(request.context, trace)?;
            let change = CertificateChange {
                command: decoded(&encoded, "command")?,
                if_match: Some(request.if_match).filter(|tag| !tag.is_empty()),
            };
            self.port.change(context, change).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(output) => wire::ChangeResponse {
                content: output.content,
                etag: output.etag.unwrap_or_default(),
                error: None,
            },
            Err(error) => wire::ChangeResponse {
                error: Some((&error).into()),
                ..wire::ChangeResponse::default()
            },
        }))
    }
}
