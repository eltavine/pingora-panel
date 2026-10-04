use crate::{
    AccountId, DnsProviderChange, NewAccount, NewAutomaticCertificate, NewDnsProvider, Secret,
};
use panel_application::operations;
use panel_domain::CertificateId;

operations! {
    /// A read of the certificate inventory, ACME accounts, automatic
    /// certificates or DNS providers.
    pub enum CertificateQuery {
        "certificates.list" => Certificates,
        "certificates.get" => Certificate { id: CertificateId },
        "acme.accounts.list" => Accounts,
        "acme.accounts.get" => Account { id: AccountId },
        "acme.certificates.list" => AutomaticCertificates,
        "acme.certificates.get" => AutomaticCertificate { id: CertificateId },
        "acme.dns_providers.list" => DnsProviders,
        "acme.dns_providers.get" => DnsProvider { id: String },
    }
}

operations! {
    /// A change of the certificate inventory, ACME accounts, automatic
    /// certificates or DNS providers.
    pub enum CertificateCommand {
        /// Adds a certificate from elsewhere with its private key.
        "certificates.upload" => Upload { id: CertificateId, chain: String, key: Secret },
        /// Adds a certificate signed by its own new key.
        "certificates.generate" => Generate { id: CertificateId, names: Vec<String>, days: u32 },
        /// Replaces a certificate's chain and key, such as with a renewed one.
        "certificates.replace" => Replace { id: CertificateId, chain: String, key: Secret },
        "certificates.delete" => Delete { id: CertificateId },
        "acme.accounts.create" => CreateAccount { account: NewAccount },
        "acme.accounts.delete" => DeleteAccount { id: AccountId },
        "acme.certificates.create" => CreateAutomaticCertificate { certificate: NewAutomaticCertificate },
        /// Issues an automatic certificate now rather than when due.
        "acme.certificates.renew" => RenewAutomaticCertificate { id: CertificateId },
        "acme.certificates.delete" => DeleteAutomaticCertificate { id: CertificateId },
        "acme.dns_providers.create" => CreateDnsProvider { provider: NewDnsProvider },
        "acme.dns_providers.update" => UpdateDnsProvider { id: String, change: DnsProviderChange },
        "acme.dns_providers.delete" => DeleteDnsProvider { id: String },
    }
}

const CERTIFICATES: &str = "certificates";
const ACCOUNTS: &str = "acme-accounts";
const AUTOMATIC: &str = "acme-certificates";
const DNS_PROVIDERS: &str = "dns-providers";

impl CertificateQuery {
    /// The path of what it reads.
    pub fn resource(&self) -> String {
        match self {
            Self::Certificates => CERTIFICATES.into(),
            Self::Certificate { id } => format!("{CERTIFICATES}/{id}"),
            Self::Accounts => ACCOUNTS.into(),
            Self::Account { id } => format!("{ACCOUNTS}/{}", id.as_str()),
            Self::AutomaticCertificates => AUTOMATIC.into(),
            Self::AutomaticCertificate { id } => format!("{AUTOMATIC}/{id}"),
            Self::DnsProviders => DNS_PROVIDERS.into(),
            Self::DnsProvider { id } => format!("{DNS_PROVIDERS}/{id}"),
        }
    }
}

impl CertificateCommand {
    /// The path of what it changes, which audit records name.
    pub fn resource(&self) -> String {
        match self {
            Self::Upload { .. } | Self::Generate { .. } => CERTIFICATES.into(),
            Self::Replace { id, .. } | Self::Delete { id } => format!("{CERTIFICATES}/{id}"),
            Self::CreateAccount { .. } => ACCOUNTS.into(),
            Self::DeleteAccount { id } => format!("{ACCOUNTS}/{}", id.as_str()),
            Self::CreateAutomaticCertificate { .. } => AUTOMATIC.into(),
            Self::RenewAutomaticCertificate { id } | Self::DeleteAutomaticCertificate { id } => {
                format!("{AUTOMATIC}/{id}")
            }
            Self::CreateDnsProvider { .. } => DNS_PROVIDERS.into(),
            Self::UpdateDnsProvider { id, .. } | Self::DeleteDnsProvider { id } => {
                format!("{DNS_PROVIDERS}/{id}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> CertificateId {
        CertificateId::new(value).unwrap()
    }

    #[test]
    fn operations_travel_under_their_names_with_their_secrets_hidden_in_logs() {
        let upload = CertificateCommand::Upload {
            id: id("example.com"),
            chain: "-----BEGIN CERTIFICATE-----".into(),
            key: Secret::new("-----BEGIN PRIVATE KEY-----"),
        };
        assert_eq!(
            (upload.operation(), upload.resource().as_str()),
            ("certificates.upload", "certificates")
        );
        assert!(!format!("{upload:?}").contains("PRIVATE KEY"));
        let encoded = serde_json::to_value(&upload).unwrap();
        assert_eq!(encoded["operation"], "certificates.upload");
        assert_eq!(encoded["parameters"]["key"], "-----BEGIN PRIVATE KEY-----");
        assert_eq!(
            serde_json::from_value::<CertificateCommand>(encoded).unwrap(),
            upload
        );

        let read = CertificateQuery::AutomaticCertificate {
            id: id("example.com"),
        };
        assert_eq!(
            (read.operation(), read.resource().as_str()),
            ("acme.certificates.get", "acme-certificates/example.com")
        );
    }

    #[test]
    fn unknown_operations_and_invalid_names_are_refused() {
        let unknown = serde_json::json!({"operation": "certificates.export"});
        assert!(serde_json::from_value::<CertificateQuery>(unknown).is_err());
        let account = serde_json::json!({"operation": "acme.accounts.get", "parameters": {"id": "Not Valid"}});
        assert!(serde_json::from_value::<CertificateQuery>(account).is_err());
    }
}
