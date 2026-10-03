//! Which passwords are accepted and how they are stored.

use argon2::{
    password_hash::{phc::PasswordHash, PasswordHasher as _, PasswordVerifier as _},
    Algorithm, Argon2, Params, Version,
};
use panel_errors::{PanelError, Result};
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;
use unicode_normalization::UnicodeNormalization;

/// Words every password is checked against besides the account's own.
const PRODUCT_WORDS: &[&str] = &["pingora", "panel", "ppanel", "admin", "administrator"];

/// The strength estimate reads this many characters; a password whose
/// beginning is strong enough is strong enough.
const ESTIMATED_CHARS: usize = 100;

/// What a new password must be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PasswordPolicy {
    /// In characters, counted as Unicode code points after NFC normalization.
    pub min_chars: usize,
    pub max_chars: usize,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            min_chars: 15,
            max_chars: 1024,
        }
    }
}

/// Why a password was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PasswordProblem {
    TooShort {
        min: usize,
    },
    TooLong {
        max: usize,
    },
    /// Common, predictable or built from the account's own names.
    Guessable {
        warning: Option<String>,
        suggestions: Vec<String>,
    },
}

impl PasswordProblem {
    pub fn message(&self) -> String {
        match self {
            Self::TooShort { min } => format!("the password must have at least {min} characters"),
            Self::TooLong { max } => format!("the password must have at most {max} characters"),
            Self::Guessable { warning, .. } => match warning {
                Some(warning) => format!("the password is too easy to guess: {warning}"),
                None => "the password is too easy to guess".into(),
            },
        }
    }

    pub fn help(&self) -> String {
        match self {
            Self::Guessable { suggestions, .. } if !suggestions.is_empty() => suggestions.join(" "),
            _ => "a few unrelated words make a long password that is easy to remember".into(),
        }
    }

    pub fn into_error(self) -> PanelError {
        let diagnostic = panel_errors::Diagnostic::error(
            panel_errors::ErrorCode::VALIDATION_FAILED,
            self.message(),
        )
        .with_resource("password")
        .with_help(self.help());
        PanelError::validation_failed(self.message()).with_diagnostics(vec![diagnostic])
    }
}

/// The password as it is checked and hashed: in Unicode normalization form C.
fn normalize(password: &str) -> String {
    password.nfc().collect()
}

impl PasswordPolicy {
    /// Checks a password being set; `context` holds words it must not be
    /// built from, such as the username.
    pub fn check(
        &self,
        password: &str,
        context: &[&str],
    ) -> std::result::Result<(), PasswordProblem> {
        let normalized = normalize(password);
        let count = normalized.chars().count();
        if count < self.min_chars {
            return Err(PasswordProblem::TooShort {
                min: self.min_chars,
            });
        }
        if count > self.max_chars {
            return Err(PasswordProblem::TooLong {
                max: self.max_chars,
            });
        }
        let inputs: Vec<&str> = context.iter().chain(PRODUCT_WORDS).copied().collect();
        let estimated: String = normalized.chars().take(ESTIMATED_CHARS).collect();
        let estimate = zxcvbn::zxcvbn(&estimated, &inputs);
        if estimate.score() >= zxcvbn::Score::Three {
            return Ok(());
        }
        let feedback = estimate.feedback();
        Err(PasswordProblem::Guessable {
            warning: feedback
                .and_then(|feedback| feedback.warning())
                .map(|warning| warning.to_string()),
            suggestions: feedback
                .map(|feedback| {
                    feedback
                        .suggestions()
                        .iter()
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// Whether a password matched a stored hash.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Verification {
    /// It matched; `outdated` when the hash should be replaced by one with
    /// the current parameters.
    Matches {
        outdated: bool,
    },
    Mismatch,
}

/// Hashes passwords as Argon2id PHC strings, keyed with an optional pepper.
/// Hashing runs on blocking threads, a few at a time, since each one takes
/// tens of milliseconds and 19 MiB.
#[derive(Clone)]
pub struct PasswordHasher {
    pepper: Option<Arc<[u8]>>,
    params: Params,
    running: Arc<Semaphore>,
}

impl PasswordHasher {
    pub fn new(pepper: Option<Vec<u8>>) -> Self {
        let lanes = std::thread::available_parallelism().map_or(2, |count| count.get().min(8));
        Self {
            pepper: pepper.map(Arc::from),
            params: Params::DEFAULT,
            running: Arc::new(Semaphore::new(lanes)),
        }
    }

    fn argon2(pepper: Option<&[u8]>, params: Params) -> Result<Argon2<'_>> {
        match pepper {
            Some(pepper) => {
                Argon2::new_with_secret(pepper, Algorithm::Argon2id, Version::V0x13, params)
                    .map_err(|error| {
                        PanelError::invalid_argument(format!("unusable pepper: {error}"))
                    })
            }
            None => Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params)),
        }
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(Option<&[u8]>, Params) -> T + Send + 'static,
    ) -> Result<T> {
        let _permit = self
            .running
            .acquire()
            .await
            .map_err(|_| PanelError::unavailable("password hashing stopped"))?;
        let pepper = self.pepper.clone();
        let params = self.params.clone();
        tokio::task::spawn_blocking(move || work(pepper.as_deref(), params))
            .await
            .map_err(|error| PanelError::internal(format!("password hashing failed: {error}")))
    }

    pub async fn hash(&self, password: &str) -> Result<String> {
        let password = normalize(password);
        self.blocking(move |pepper, params| {
            Self::argon2(pepper, params)?
                .hash_password(password.as_bytes())
                .map(|hash| hash.to_string())
                .map_err(|error| PanelError::internal(format!("password hashing failed: {error}")))
        })
        .await?
    }

    pub async fn verify(&self, password: &str, stored: &str) -> Result<Verification> {
        let password = normalize(password);
        let stored = stored.to_owned();
        self.blocking(move |pepper, params| {
            let Ok(parsed) = PasswordHash::new(&stored) else {
                return Ok(Verification::Mismatch);
            };
            let argon2 = Self::argon2(pepper, params.clone())?;
            if argon2
                .verify_password(password.as_bytes(), &parsed)
                .is_err()
            {
                return Ok(Verification::Mismatch);
            }
            let current = Params::try_from(&parsed).is_ok_and(|used| {
                used.m_cost() == params.m_cost()
                    && used.t_cost() == params.t_cost()
                    && used.p_cost() == params.p_cost()
            });
            let algorithm = parsed.algorithm.as_str() == Algorithm::Argon2id.as_ref();
            Ok(Verification::Matches {
                outdated: !(current && algorithm),
            })
        })
        .await?
    }

    /// Takes as long as verifying a password, for accounts without one, so
    /// the answer's timing does not tell whether an account exists.
    pub async fn verify_nothing(&self, password: &str) -> Result<()> {
        static DECOY: OnceLock<String> = OnceLock::new();
        let decoy = match DECOY.get() {
            Some(decoy) => decoy.clone(),
            None => {
                let decoy = self.hash("decoy password never accepted").await?;
                DECOY.get_or_init(|| decoy).clone()
            }
        };
        self.verify(password, &decoy).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_common_and_personal_passwords_are_refused() {
        let policy = PasswordPolicy::default();
        assert_eq!(
            policy.check("correct horse", &[]),
            Err(PasswordProblem::TooShort { min: 15 })
        );
        // Code points are counted, after composing "e" and its accent.
        assert_eq!(
            policy.check(&"e\u{301}".repeat(14), &[]),
            Err(PasswordProblem::TooShort { min: 15 })
        );
        assert_eq!(
            policy.check(&"x".repeat(1025), &[]),
            Err(PasswordProblem::TooLong { max: 1024 })
        );
        for weak in ["passwordpassword", "123456789012345", "aaaaaaaaaaaaaaaa"] {
            assert!(
                matches!(
                    policy.check(weak, &[]),
                    Err(PasswordProblem::Guessable { .. })
                ),
                "{weak}"
            );
        }
        assert!(matches!(
            policy.check("aliceoperator2024", &["aliceoperator"]),
            Err(PasswordProblem::Guessable { .. })
        ));
        assert!(policy.check("ppanelppanelppanel", &[]).is_err());
        assert_eq!(
            policy.check("glacier violin tapestry orbit", &["alice"]),
            Ok(())
        );
        let problem = policy.check("passwordpassword", &[]).unwrap_err();
        assert!(problem
            .message()
            .starts_with("the password is too easy to guess"));
        assert!(!problem.help().is_empty());
        assert_eq!(problem.into_error().diagnostics.len(), 1);
    }

    #[tokio::test]
    async fn hashes_verify_normalized_passwords_and_report_old_parameters() {
        let hasher = PasswordHasher::new(None);
        let hash = hasher
            .hash("caf\u{e9} au lait every morning")
            .await
            .unwrap();
        assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        // The decomposed spelling of the same text matches.
        assert_eq!(
            hasher
                .verify("cafe\u{301} au lait every morning", &hash)
                .await
                .unwrap(),
            Verification::Matches { outdated: false }
        );
        assert_eq!(
            hasher
                .verify("cafe au lait every morning", &hash)
                .await
                .unwrap(),
            Verification::Mismatch
        );
        assert_eq!(
            hasher.verify("anything", "not a hash").await.unwrap(),
            Verification::Mismatch
        );

        let weaker = Params::new(8 * 1024, 1, 1, None).unwrap();
        let old = Argon2::new(Algorithm::Argon2id, Version::V0x13, weaker)
            .hash_password(b"an old but fine password")
            .unwrap()
            .to_string();
        assert_eq!(
            hasher
                .verify("an old but fine password", &old)
                .await
                .unwrap(),
            Verification::Matches { outdated: true }
        );
        hasher.verify_nothing("whatever").await.unwrap();
    }

    #[tokio::test]
    async fn a_pepper_is_needed_to_verify_peppered_hashes() {
        let peppered = PasswordHasher::new(Some(b"a pepper of sixteen+ bytes".to_vec()));
        let hash = peppered.hash("a long enough passphrase").await.unwrap();
        assert_eq!(
            peppered
                .verify("a long enough passphrase", &hash)
                .await
                .unwrap(),
            Verification::Matches { outdated: false }
        );
        assert_eq!(
            PasswordHasher::new(None)
                .verify("a long enough passphrase", &hash)
                .await
                .unwrap(),
            Verification::Mismatch
        );
    }
}
