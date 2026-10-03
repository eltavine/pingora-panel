//! Answering challenges: HTTP-01 through files the gateway serves, DNS-01
//! through TXT records a [`DnsProvider`] publishes.

use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// How a CA checks control of the names a certificate is ordered for.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ChallengeKind {
    /// A key authorization served over HTTP on port 80 (RFC 8555 §8.3).
    #[serde(rename = "http-01")]
    Http01,
    /// A TXT record at `_acme-challenge.<name>` (RFC 8555 §8.4); the only
    /// kind CAs accept for wildcard names.
    #[serde(rename = "dns-01")]
    Dns01,
}

impl ChallengeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http01 => "http-01",
            Self::Dns01 => "dns-01",
        }
    }
}

/// A challenge to answer for one name of an order.
#[derive(Clone, Debug)]
pub struct Challenge {
    pub kind: ChallengeKind,
    /// The DNS name or IP address, without a wildcard label.
    pub identifier: String,
    pub wildcard: bool,
    pub token: String,
    /// The token and the account key's thumbprint (RFC 8555 §8.1).
    pub key_authorization: String,
    /// The base64url SHA-256 digest of the key authorization, which DNS-01
    /// publishes.
    pub dns_value: String,
}

/// Sets up and removes the answers to challenges.
#[async_trait]
pub trait ChallengeSolver: Send + Sync {
    /// Makes `challenge` answerable; the CA validates it once this returns.
    async fn present(&self, challenge: &Challenge) -> Result<()>;

    /// Removes what [`present`](Self::present) set up.
    async fn clean_up(&self, challenge: &Challenge) -> Result<()>;
}

/// HTTP-01 key authorizations, written as files named after their tokens
/// into the directory the gateway answers challenges from.
#[derive(Clone, Debug)]
pub struct Http01 {
    directory: PathBuf,
}

impl Http01 {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    fn file(&self, challenge: &Challenge) -> Result<PathBuf> {
        let token = &challenge.token;
        let valid = (1..=128).contains(&token.len())
            && token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if !valid {
            return Err(PanelError::validation_failed(
                "the CA sent a challenge token outside the base64url alphabet",
            ));
        }
        Ok(self.directory.join(token))
    }
}

fn write_atomically(directory: &Path, file: &Path, content: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(content)?;
    temporary.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Key authorizations are public; the gateway may run as another user.
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o644))?;
    }
    temporary.persist(file).map_err(|error| error.error)?;
    Ok(())
}

#[async_trait]
impl ChallengeSolver for Http01 {
    async fn present(&self, challenge: &Challenge) -> Result<()> {
        let file = self.file(challenge)?;
        let directory = self.directory.clone();
        let content = challenge.key_authorization.clone().into_bytes();
        tokio::task::spawn_blocking(move || write_atomically(&directory, &file, &content))
            .await
            .map_err(|error| PanelError::internal(format!("challenge writing stopped: {error}")))?
            .map_err(|error| {
                PanelError::unavailable(format!("the challenge cannot be written: {error}"))
            })
    }

    async fn clean_up(&self, challenge: &Challenge) -> Result<()> {
        match tokio::fs::remove_file(self.file(challenge)?).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PanelError::unavailable(format!(
                "the challenge cannot be removed: {error}"
            ))),
        }
    }
}

/// Publishes and withdraws TXT records in a DNS zone, for DNS-01.
#[async_trait]
pub trait DnsProvider: Send + Sync {
    /// Adds a TXT record with `value` at the fully qualified `name`, beside
    /// any records already there.
    async fn add_txt(&self, name: &str, value: &str) -> Result<()>;

    /// Removes the TXT record with `value` at `name`, leaving others.
    async fn remove_txt(&self, name: &str, value: &str) -> Result<()>;
}

/// DNS-01 through a [`DnsProvider`]: each challenge is a TXT record at
/// `_acme-challenge.<name>`, given time to reach the zone's authoritative
/// servers before the CA looks.
#[derive(Clone)]
pub struct Dns01 {
    provider: Arc<dyn DnsProvider>,
    propagation: Duration,
}

impl Dns01 {
    pub fn new(provider: Arc<dyn DnsProvider>, propagation: Duration) -> Self {
        Self {
            provider,
            propagation,
        }
    }

    fn record(challenge: &Challenge) -> String {
        format!(
            "_acme-challenge.{}.",
            challenge.identifier.trim_end_matches('.')
        )
    }
}

#[async_trait]
impl ChallengeSolver for Dns01 {
    async fn present(&self, challenge: &Challenge) -> Result<()> {
        self.provider
            .add_txt(&Self::record(challenge), &challenge.dns_value)
            .await?;
        tokio::time::sleep(self.propagation).await;
        Ok(())
    }

    async fn clean_up(&self, challenge: &Challenge) -> Result<()> {
        self.provider
            .remove_txt(&Self::record(challenge), &challenge.dns_value)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn challenge(kind: ChallengeKind, token: &str) -> Challenge {
        Challenge {
            kind,
            identifier: "shop.example".into(),
            wildcard: false,
            token: token.into(),
            key_authorization: format!("{token}.thumbprint"),
            dns_value: "digest".into(),
        }
    }

    #[tokio::test]
    async fn http01_writes_key_authorizations_named_after_tokens() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("acme-challenge");
        let solver = Http01::new(&directory);
        let pending = challenge(ChallengeKind::Http01, "Tok_en-1");
        solver.present(&pending).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.join("Tok_en-1")).unwrap(),
            "Tok_en-1.thumbprint"
        );
        solver.clean_up(&pending).await.unwrap();
        assert!(!directory.join("Tok_en-1").exists());
        solver.clean_up(&pending).await.unwrap();
        let escaping = challenge(ChallengeKind::Http01, "../escape");
        assert!(solver.present(&escaping).await.is_err());
    }

    #[derive(Default)]
    struct Records(Mutex<Vec<(String, String)>>);

    #[async_trait]
    impl DnsProvider for Records {
        async fn add_txt(&self, name: &str, value: &str) -> Result<()> {
            self.0.lock().unwrap().push((name.into(), value.into()));
            Ok(())
        }

        async fn remove_txt(&self, name: &str, value: &str) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .retain(|record| record != &(name.to_owned(), value.to_owned()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn dns01_publishes_digests_under_the_challenge_label() {
        let records = Arc::new(Records::default());
        let solver = Dns01::new(Arc::clone(&records) as Arc<dyn DnsProvider>, Duration::ZERO);
        let pending = challenge(ChallengeKind::Dns01, "token");
        solver.present(&pending).await.unwrap();
        assert_eq!(
            *records.0.lock().unwrap(),
            [(
                "_acme-challenge.shop.example.".to_owned(),
                "digest".to_owned()
            )]
        );
        solver.clean_up(&pending).await.unwrap();
        assert!(records.0.lock().unwrap().is_empty());
    }
}
