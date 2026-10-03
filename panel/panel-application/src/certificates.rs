use crate::{CommandContext, RequestScope};
use async_trait::async_trait;
use panel_errors::Result;
use std::fmt;

/// A named read of the certificate inventory; `parameters` is a JSON object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CertificateRead {
    pub operation: String,
    pub resource: String,
    pub parameters: Vec<u8>,
}

/// A named change of the certificate inventory; `content` is the JSON body,
/// which may hold a private key and is therefore never printed.
#[derive(Clone, Eq, PartialEq)]
pub struct CertificateChange {
    pub operation: String,
    pub resource: String,
    /// Entity tag the certificate must still have (RFC 9110 §13.1.1).
    pub if_match: Option<String>,
    pub content: Vec<u8>,
}

impl fmt::Debug for CertificateChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CertificateChange")
            .field("operation", &self.operation)
            .field("resource", &self.resource)
            .field("if_match", &self.if_match)
            .finish_non_exhaustive()
    }
}

/// A JSON result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CertificateOutput {
    pub content: Vec<u8>,
    pub etag: Option<String>,
}

/// The certificate inventory, whose private keys stay with its owner.
#[async_trait]
pub trait CertificatePort: Send + Sync {
    async fn read(&self, scope: RequestScope, read: CertificateRead) -> Result<CertificateOutput>;

    async fn change(
        &self,
        context: CommandContext,
        change: CertificateChange,
    ) -> Result<CertificateOutput>;
}
