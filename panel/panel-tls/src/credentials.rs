use arc_swap::ArcSwap;
use panel_errors::{PanelError, Result};
use panel_pki::{CredentialFiles, WorkloadIdentity};
use rustls::{
    crypto::{ring, CryptoProvider},
    server::WebPkiClientVerifier,
    version::TLS13,
    ClientConfig, RootCertStore, ServerConfig,
};
use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio_util::sync::CancellationToken;

struct Loaded {
    server: Arc<ServerConfig>,
    client: Arc<ClientConfig>,
    stamp: (SystemTime, SystemTime),
}

/// A service's TLS identity and trust, read from its credential files.
pub struct TlsCredentials {
    files: CredentialFiles,
    identity: WorkloadIdentity,
    loaded: ArcSwap<Loaded>,
}

fn failed(what: &str) -> impl FnOnce(rustls::Error) -> PanelError + '_ {
    move |error| PanelError::invalid_argument(format!("{what}: {error}"))
}

fn stamp(files: &CredentialFiles) -> Result<(SystemTime, SystemTime)> {
    let modified = |path: std::path::PathBuf| {
        std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .map_err(|error| {
                PanelError::storage_unavailable(format!("cannot read {}: {error}", path.display()))
            })
    };
    Ok((
        modified(files.identity_path())?,
        modified(files.trust_path())?,
    ))
}

fn load(files: &CredentialFiles) -> Result<Loaded> {
    let stamp = stamp(files)?;
    let read = |path: std::path::PathBuf| {
        std::fs::read(&path).map_err(|error| {
            PanelError::storage_unavailable(format!("cannot read {}: {error}", path.display()))
        })
    };
    let identity = read(files.identity_path())?;
    let trust = read(files.trust_path())?;
    let key = PrivateKeyDer::from_pem_slice(&identity)
        .map_err(|_| PanelError::invalid_argument("the identity has no private key"))?;
    let chain = CertificateDer::pem_slice_iter(&identity)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| PanelError::invalid_argument("the identity certificates are not PEM"))?;
    if chain.is_empty() {
        return Err(PanelError::invalid_argument(
            "the identity has no certificate",
        ));
    }
    let mut roots = RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(&trust) {
        let certificate =
            certificate.map_err(|_| PanelError::invalid_argument("the trust bundle is not PEM"))?;
        roots
            .add(certificate)
            .map_err(failed("invalid trusted certificate"))?;
    }
    if roots.is_empty() {
        return Err(PanelError::invalid_argument("the trust bundle is empty"));
    }
    let roots = Arc::new(roots);
    let provider: Arc<CryptoProvider> = Arc::new(ring::default_provider());
    let verifier =
        WebPkiClientVerifier::builder_with_provider(Arc::clone(&roots), Arc::clone(&provider))
            .build()
            .map_err(|error| {
                PanelError::invalid_argument(format!("invalid client verifier: {error}"))
            })?;
    let mut server = ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&TLS13])
        .map_err(failed("TLS 1.3 is unavailable"))?
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain.clone(), key.clone_key())
        .map_err(failed("the identity key does not match its certificate"))?;
    server.alpn_protocols = vec![b"h2".to_vec()];
    let mut client = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&TLS13])
        .map_err(failed("TLS 1.3 is unavailable"))?
        .with_root_certificates(roots)
        .with_client_auth_cert(chain, key)
        .map_err(failed("the identity key does not match its certificate"))?;
    client.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Loaded {
        server: Arc::new(server),
        client: Arc::new(client),
        stamp,
    })
}

impl TlsCredentials {
    pub fn load(files: CredentialFiles, identity: WorkloadIdentity) -> Result<Arc<Self>> {
        let loaded = load(&files)?;
        Ok(Arc::new(Self {
            files,
            identity,
            loaded: ArcSwap::from_pointee(loaded),
        }))
    }

    pub fn identity(&self) -> &WorkloadIdentity {
        &self.identity
    }

    pub fn server_config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.loaded.load().server)
    }

    pub fn client_config(&self) -> Arc<ClientConfig> {
        Arc::clone(&self.loaded.load().client)
    }

    /// Reloads the credentials if their files changed. A change that cannot
    /// be loaded keeps the current credentials and fails.
    pub fn reload(&self) -> Result<bool> {
        if stamp(&self.files)? == self.loaded.load().stamp {
            return Ok(false);
        }
        self.loaded.store(Arc::new(load(&self.files)?));
        Ok(true)
    }

    /// Reloads changed credentials every `interval` until `shutdown`.
    pub async fn watch(self: Arc<Self>, interval: Duration, shutdown: CancellationToken) {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(interval) => {}
            }
            match self.reload() {
                Ok(true) => {
                    tracing::info!(identity = %self.identity.dns_name(), "TLS credentials rotated")
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(error_code = %error.code, error = %error.message, "TLS credentials not reloaded")
                }
            }
        }
    }
}
