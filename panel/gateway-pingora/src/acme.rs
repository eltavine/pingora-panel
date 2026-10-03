//! HTTP-01 challenges (RFC 8555 §8.3). While a certificate is being issued,
//! the key authorization for each token waits as a file named after the
//! token, and the gateway answers `/.well-known/acme-challenge/<token>` with
//! it on every listener and host. Requests for tokens without a file go on
//! to the site, so applications can still answer their own challenges.

use bytes::Bytes;
use std::path::PathBuf;
use tokio::io::AsyncReadExt;

const CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";
/// A key authorization is a token and a key thumbprint, far below this.
const MAX_KEY_AUTHORIZATION: u64 = 1024;

/// The directory key authorizations wait in, one file per token.
#[derive(Clone, Debug)]
pub struct ChallengeDirectory {
    directory: PathBuf,
}

impl ChallengeDirectory {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// The key authorization waiting for the token `path` asks for.
    pub(crate) async fn answer(&self, path: &str) -> Option<Bytes> {
        let token = path.strip_prefix(CHALLENGE_PREFIX)?;
        if !is_token(token) {
            return None;
        }
        let file = tokio::fs::File::open(self.directory.join(token))
            .await
            .ok()?;
        let mut content = Vec::new();
        file.take(MAX_KEY_AUTHORIZATION + 1)
            .read_to_end(&mut content)
            .await
            .ok()?;
        let size = u64::try_from(content.len()).ok()?;
        (size > 0 && size <= MAX_KEY_AUTHORIZATION).then(|| Bytes::from(content))
    }
}

/// RFC 8555 §8.1: tokens only use the base64url alphabet, which also keeps
/// them inside the directory.
fn is_token(token: &str) -> bool {
    (1..=128).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_waiting_tokens_are_answered() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("tok-EN_1"), "tok-EN_1.thumb").unwrap();
        std::fs::write(directory.path().join("empty"), "").unwrap();
        std::fs::write(directory.path().join("large"), vec![b'a'; 2048]).unwrap();
        std::fs::write(directory.path().join(".hidden"), "secret").unwrap();
        let challenges = ChallengeDirectory::new(directory.path());

        assert_eq!(
            challenges
                .answer("/.well-known/acme-challenge/tok-EN_1")
                .await
                .as_deref(),
            Some(&b"tok-EN_1.thumb"[..])
        );
        for path in [
            "/.well-known/acme-challenge/missing",
            "/.well-known/acme-challenge/empty",
            "/.well-known/acme-challenge/large",
            "/.well-known/acme-challenge/.hidden",
            "/.well-known/acme-challenge/../tok-EN_1",
            "/.well-known/acme-challenge/",
            "/tok-EN_1",
        ] {
            assert!(challenges.answer(path).await.is_none(), "{path}");
        }
    }
}
