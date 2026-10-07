//! The plugins API over gRPC: requests decode into the plugins port's terms
//! and what it answers encodes back, so the module behind the port knows
//! nothing of the transport.
//!
//! Application failures travel in each response's `error` field; transport
//! status codes are left to the transport.

use panel_contracts::plugins::v1::{self as wire, plugins_server::Plugins};
use panel_errors::{PanelError, Result};
use panel_plugin_api::{PluginChange, PluginsPort};
use panel_service::{decode_command, decode_scope, trace_context};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use zeroize::Zeroizing;

/// Serves a plugins port over gRPC.
pub struct PluginsTransport {
    port: Arc<dyn PluginsPort>,
}

impl PluginsTransport {
    pub fn new(port: Arc<dyn PluginsPort>) -> Self {
        Self { port }
    }
}

fn decoded<T: DeserializeOwned>(value: &[u8], what: &str) -> Result<T> {
    serde_json::from_slice(value).map_err(|error| {
        PanelError::invalid_argument(format!("the plugin {what} cannot be read: {error}"))
    })
}

#[tonic::async_trait]
impl Plugins for PluginsTransport {
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
            let change = PluginChange {
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
