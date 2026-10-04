use crate::{CertificateCommand, CertificateQuery};
use async_trait::async_trait;
use panel_application::{CommandContext, RequestScope};
use panel_errors::Result;

/// A change, and the entity tag its target must still have (RFC 9110
/// §13.1.1).
#[derive(Clone, Debug, PartialEq)]
pub struct CertificateChange {
    pub command: CertificateCommand,
    pub if_match: Option<String>,
}

impl CertificateChange {
    pub fn new(command: CertificateCommand) -> Self {
        Self {
            command,
            if_match: None,
        }
    }

    pub fn if_match(mut self, tag: impl Into<String>) -> Self {
        self.if_match = Some(tag.into());
        self
    }
}

/// A JSON result and, for a single resource, its entity tag.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CertificateOutput {
    pub content: Vec<u8>,
    pub etag: Option<String>,
}

/// The certificate inventory, ACME accounts, automatic certificates and DNS
/// providers, whose private keys and secrets stay with their owner.
#[async_trait]
pub trait CertificatePort: Send + Sync {
    async fn read(&self, scope: RequestScope, query: CertificateQuery)
        -> Result<CertificateOutput>;

    async fn change(
        &self,
        context: CommandContext,
        change: CertificateChange,
    ) -> Result<CertificateOutput>;
}
