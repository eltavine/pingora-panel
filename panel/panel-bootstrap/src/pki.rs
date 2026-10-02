use chrono::Utc;
use panel_context::ServiceName;
use panel_control_runtime::TRUST_DOMAIN_ENV;
use panel_errors::{PanelError, Result};
use panel_pki::{
    CertificateAuthority, CredentialFiles, IssuanceTarget, TrustDomain, DEFAULT_AUTHORITY_VALIDITY,
};
use panel_service::Environment;
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;

/// Where the authority's key and certificate are kept.
pub const PKI_DIR_ENV: &str = "PINGORA_PANEL_PKI_DIR";
/// Comma-separated `service=directory` pairs to keep credentials current in.
pub const PKI_CREDENTIALS_ENV: &str = "PINGORA_PANEL_PKI_CREDENTIALS";
pub const CERTIFICATE_LIFETIME_MS_ENV: &str = "PINGORA_PANEL_CERTIFICATE_LIFETIME_MS";
pub const PKI_CHECK_INTERVAL_MS_ENV: &str = "PINGORA_PANEL_PKI_CHECK_INTERVAL_MS";

const DEFAULT_LIFETIME: Duration = Duration::from_secs(24 * 3600);
const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(600);

/// The internal certificate authority and the services it issues to.
pub struct PkiPlan {
    directory: PathBuf,
    trust_domain: TrustDomain,
    targets: Vec<IssuanceTarget>,
    lifetime: Duration,
    check_interval: Duration,
}

impl PkiPlan {
    pub fn read(env: &mut Environment<'_>) -> Result<Self> {
        let targets = env
            .required(PKI_CREDENTIALS_ENV)?
            .split(',')
            .filter(|entry| !entry.trim().is_empty())
            .map(|entry| {
                let (service, directory) = entry.trim().split_once('=').ok_or_else(|| {
                    PanelError::invalid_argument(format!(
                        "{PKI_CREDENTIALS_ENV} entries are service=directory"
                    ))
                })?;
                Ok(IssuanceTarget {
                    service: ServiceName::new(service)?,
                    files: CredentialFiles::new(directory),
                    alternative_names: vec![service.to_owned()],
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            directory: PathBuf::from(env.required(PKI_DIR_ENV)?),
            trust_domain: env
                .string(TRUST_DOMAIN_ENV)?
                .map(TrustDomain::new)
                .transpose()?
                .unwrap_or_default(),
            targets,
            lifetime: env.millis(CERTIFICATE_LIFETIME_MS_ENV, DEFAULT_LIFETIME)?,
            check_interval: env.millis(PKI_CHECK_INTERVAL_MS_ENV, DEFAULT_CHECK_INTERVAL)?,
        })
    }

    /// Creates the authority on first use and renews due credentials.
    pub fn apply(&self) -> Result<Vec<ServiceName>> {
        let now = Utc::now();
        let (authority, created) = CertificateAuthority::load_or_create(
            &self.directory,
            self.trust_domain.clone(),
            DEFAULT_AUTHORITY_VALIDITY,
            now,
        )?;
        if created {
            tracing::info!(trust_domain = %self.trust_domain, "internal certificate authority created");
        }
        let renewed = authority.renew_due(&self.targets, self.lifetime, now)?;
        for service in &renewed {
            tracing::info!(service = %service, "service credentials issued");
        }
        Ok(renewed)
    }

    /// Keeps credentials current until `shutdown`.
    pub async fn maintain(&self, shutdown: CancellationToken) {
        loop {
            if let Err(error) = self.apply() {
                tracing::error!(error_code = %error.code, error = %error.message, "credential renewal failed");
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(self.check_interval) => {}
            }
        }
    }
}
