//! The certificate inventory, ACME accounts, automatic certificates and DNS
//! providers behind the certificate port.

use crate::{
    acme::AcmeAutomation,
    certificates::{Cause, CertificateInventory},
    dns::DnsProviders,
};
use async_trait::async_trait;
use panel_application::{CommandContext, RequestScope};
use panel_certificate_api::{
    CertificateChange, CertificateCommand, CertificateOutput, CertificatePort, CertificateQuery,
};
use panel_errors::{PanelError, Result};
use panel_sqlite::EventLog;
use serde::Serialize;

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

fn output(value: &impl Serialize, etag: Option<String>) -> CertificateOutput {
    CertificateOutput {
        content: serde_json::to_vec(value).expect("API values serialize"),
        etag,
    }
}

fn deleted() -> CertificateOutput {
    CertificateOutput {
        content: Vec::new(),
        etag: None,
    }
}

/// The version an `If-Match` entity tag names; none means unconditional.
fn expected(if_match: Option<&str>) -> Result<Option<u64>> {
    if_match
        .map(|tag| {
            tag.trim_matches('"')
                .parse()
                .map_err(|_| PanelError::precondition_failed(format!("unknown entity tag {tag}")))
        })
        .transpose()
}

#[async_trait]
impl CertificatePort for CertificateService {
    async fn read(
        &self,
        _scope: RequestScope,
        query: CertificateQuery,
    ) -> Result<CertificateOutput> {
        Ok(match query {
            CertificateQuery::Certificates => output(&self.inventory.list().await?, None),
            CertificateQuery::Certificate { id } => {
                let certificate = self.inventory.get(&id).await?;
                output(&certificate, Some(certificate.etag()))
            }
            CertificateQuery::Accounts => output(&self.acme.accounts().await?, None),
            CertificateQuery::Account { id } => {
                let account = self.acme.account(&id).await?;
                output(&account, Some(account.etag()))
            }
            CertificateQuery::AutomaticCertificates => {
                output(&self.acme.certificates().await?, None)
            }
            CertificateQuery::AutomaticCertificate { id } => {
                let automatic = self.acme.certificate(&id).await?;
                output(&automatic, Some(automatic.etag()))
            }
            CertificateQuery::DnsProviders => output(&self.dns.list().await?, None),
            CertificateQuery::DnsProvider { id } => {
                let provider = self.dns.get(&id).await?;
                output(&provider, Some(provider.etag()))
            }
        })
    }

    async fn change(
        &self,
        context: CommandContext,
        change: CertificateChange,
    ) -> Result<CertificateOutput> {
        let scope = context.scope();
        let principal = EventLog::user(context.actor());
        let cause = Cause {
            scope: &scope,
            principal: &principal,
        };
        let expected = expected(change.if_match.as_deref())?;
        let certificate = |certificate: panel_certificates::Certificate| {
            let etag = certificate.etag();
            output(&certificate, Some(etag))
        };
        Ok(match change.command {
            CertificateCommand::Upload { id, chain, key } => certificate(
                self.inventory
                    .upload(cause, id, &chain, key.expose())
                    .await?,
            ),
            CertificateCommand::Generate { id, names, days } => {
                certificate(self.inventory.generate(cause, id, &names, days).await?)
            }
            CertificateCommand::Replace { id, chain, key } => certificate(
                self.inventory
                    .replace(cause, id, expected, &chain, key.expose())
                    .await?,
            ),
            CertificateCommand::Delete { id } => {
                self.inventory.delete(cause, id, expected).await?;
                deleted()
            }
            CertificateCommand::CreateAccount { account } => {
                let account = self.acme.create_account(cause, account).await?;
                output(&account, Some(account.etag()))
            }
            CertificateCommand::DeleteAccount { id } => {
                self.acme.delete_account(cause, id, expected).await?;
                deleted()
            }
            CertificateCommand::CreateAutomaticCertificate { certificate } => {
                let automatic = self.acme.create_certificate(cause, certificate).await?;
                output(&automatic, Some(automatic.etag()))
            }
            CertificateCommand::RenewAutomaticCertificate { id } => {
                let automatic = self.acme.renew(cause, id).await?;
                output(&automatic, Some(automatic.etag()))
            }
            CertificateCommand::DeleteAutomaticCertificate { id } => {
                self.acme.delete_certificate(cause, id, expected).await?;
                deleted()
            }
            CertificateCommand::CreateDnsProvider { provider } => {
                let provider = self.dns.create(cause, provider).await?;
                output(&provider, Some(provider.etag()))
            }
            CertificateCommand::UpdateDnsProvider { id, change } => {
                let provider = self.dns.update(cause, &id, expected, change).await?;
                output(&provider, Some(provider.etag()))
            }
            CertificateCommand::DeleteDnsProvider { id } => {
                self.dns.delete(cause, &id, expected).await?;
                deleted()
            }
        })
    }
}
