//! Workload identity (ADR 0020): programs prove what they are with a
//! short-lived token from an issuer an account manager trusts, and get a
//! short bearer session for a service account in exchange.

use crate::{
    csrf_token,
    providers::{check_issuer, text},
    service::cause,
    store::{Attempt, Cause, NewSession},
    AccountId, Client, IdentityStore, Login, Secret, Session, SessionId, Transport,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use panel_context::RequestScope;
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, sync::Arc};

pub const MIN_SESSION_MINUTES: u32 = 5;
pub const MAX_SESSION_MINUTES: u32 = 60;
const MAX_CLAIMS: usize = 16;

/// What a workload token says once its signature, issuer and lifetime
/// have been checked.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedWorkload {
    pub issuer: String,
    pub subject: String,
    pub audiences: Vec<String>,
    pub claims: Map<String, Value>,
}

/// Verifies workload tokens against their issuer's published keys.
#[async_trait]
pub trait WorkloadVerifier: Send + Sync {
    /// Refuses tokens from issuers outside `issuers`.
    async fn verify(&self, token: &str, issuers: &[String]) -> Result<VerifiedWorkload>;
}

/// A trust in an issuer's tokens for a service account.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkloadTrust {
    pub id: String,
    pub account: AccountId,
    pub issuer: String,
    /// The token must name it among its audiences.
    pub audience: String,
    /// The subject exactly, or a prefix ending in `*`.
    pub subject: String,
    /// Further claims that must equal these values.
    pub claims: BTreeMap<String, String>,
    pub session_minutes: u32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl WorkloadTrust {
    /// Whether a verified token is one this trust admits.
    pub fn admits(&self, token: &VerifiedWorkload) -> bool {
        let subject = match self.subject.strip_suffix('*') {
            Some(prefix) => token.subject.starts_with(prefix),
            None => token.subject == self.subject,
        };
        self.enabled
            && token.issuer == self.issuer
            && token.audiences.contains(&self.audience)
            && subject
            && self
                .claims
                .iter()
                .all(|(name, expected)| match token.claims.get(name) {
                    Some(Value::String(value)) => value == expected,
                    Some(Value::Bool(value)) => value.to_string() == *expected,
                    Some(Value::Number(value)) => value.to_string() == *expected,
                    _ => false,
                })
    }
}

/// A trust as an account manager writes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkloadRequest {
    pub account: AccountId,
    pub issuer: String,
    pub audience: String,
    pub subject: String,
    pub claims: BTreeMap<String, String>,
    pub session_minutes: u32,
    pub enabled: bool,
}

/// Where trusts are kept. Every change records its event:
/// `identity.workload_trust.created`, `.updated` and `.deleted`.
#[async_trait]
pub trait WorkloadStore: Send + Sync {
    async fn trusts(&self) -> Result<Vec<WorkloadTrust>>;

    /// Creates or replaces a trust; true when it is new.
    async fn put_trust(&self, trust: WorkloadTrust, cause: &Cause) -> Result<bool>;

    async fn delete_trust(&self, id: &str, cause: &Cause) -> Result<()>;
}

type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Trusts and the exchange of workload tokens for sessions.
#[derive(Clone)]
pub struct WorkloadIdentity {
    trusts: Arc<dyn WorkloadStore>,
    identity: Arc<dyn IdentityStore>,
    verifier: Arc<dyn WorkloadVerifier>,
    clock: Clock,
}

impl WorkloadIdentity {
    pub fn new(
        trusts: Arc<dyn WorkloadStore>,
        identity: Arc<dyn IdentityStore>,
        verifier: Arc<dyn WorkloadVerifier>,
    ) -> Self {
        Self {
            trusts,
            identity,
            verifier,
            clock: Arc::new(Utc::now),
        }
    }

    /// Reads the time from `clock` instead of the system.
    pub fn with_clock(mut self, clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    pub async fn list(&self) -> Result<Vec<WorkloadTrust>> {
        self.trusts.trusts().await
    }

    pub async fn get(&self, id: &str) -> Result<WorkloadTrust> {
        self.trusts
            .trusts()
            .await?
            .into_iter()
            .find(|trust| trust.id == id)
            .ok_or_else(|| PanelError::not_found(format!("there is no workload identity {id}")))
    }

    /// Creates or replaces a trust for an enabled service account; true when
    /// it is new.
    pub async fn put(
        &self,
        id: &str,
        request: WorkloadRequest,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<(WorkloadTrust, bool)> {
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        {
            return Err(PanelError::invalid_argument(
                "a workload identity's ID has 1 to 64 letters, digits, dots, dashes or underscores",
            ));
        }
        check_issuer(&request.issuer)?;
        text(&request.audience, "audience", 256)?;
        text(&request.subject, "subject", 256)?;
        if request.claims.len() > MAX_CLAIMS {
            return Err(PanelError::invalid_argument(format!(
                "a workload identity checks at most {MAX_CLAIMS} claims"
            )));
        }
        for (name, value) in &request.claims {
            text(name, "claim name", 64)?;
            text(value, "claim value", 256)?;
        }
        if !(MIN_SESSION_MINUTES..=MAX_SESSION_MINUTES).contains(&request.session_minutes) {
            return Err(PanelError::invalid_argument(format!(
                "sessions last {MIN_SESSION_MINUTES} to {MAX_SESSION_MINUTES} minutes"
            )));
        }
        let account = self
            .identity
            .account(request.account)
            .await?
            .ok_or_else(|| PanelError::not_found("there is no such account"))?
            .account;
        if !account.service {
            return Err(PanelError::invalid_argument(
                "workloads act as service accounts, not as people",
            ));
        }
        let now = (self.clock)();
        let existing = self
            .trusts
            .trusts()
            .await?
            .into_iter()
            .find(|trust| trust.id == id);
        let trust = WorkloadTrust {
            id: id.to_owned(),
            account: request.account,
            issuer: request.issuer,
            audience: request.audience,
            subject: request.subject,
            claims: request.claims,
            session_minutes: request.session_minutes,
            enabled: request.enabled,
            created_at: existing.map_or(now, |trust| trust.created_at),
            updated_at: now,
        };
        let created = self
            .trusts
            .put_trust(trust.clone(), &cause(scope, actor))
            .await?;
        Ok((trust, created))
    }

    pub async fn delete(&self, id: &str, scope: &RequestScope, actor: &str) -> Result<()> {
        self.trusts.delete_trust(id, &cause(scope, actor)).await
    }

    /// Exchanges a workload token for a bearer session of the service account
    /// the first matching trust names. Refusals are recorded as failed
    /// logins.
    pub async fn exchange(
        &self,
        token: &str,
        client: &Client,
        scope: &RequestScope,
    ) -> Result<Login> {
        let mut attempt = Attempt {
            username: String::new(),
            provider: None,
            break_glass: false,
            client_address: client.address.clone(),
            user_agent: client.user_agent.clone(),
        };
        let mut trusts: Vec<WorkloadTrust> = self
            .trusts
            .trusts()
            .await?
            .into_iter()
            .filter(|trust| trust.enabled)
            .collect();
        trusts.sort_by(|a, b| a.id.cmp(&b.id));
        let mut issuers: Vec<String> = trusts.iter().map(|trust| trust.issuer.clone()).collect();
        issuers.dedup();
        let refuse = |attempt: &Attempt, reason: &'static str| {
            let attempt = attempt.clone();
            let cause = cause(scope, "workload");
            async move {
                self.identity.login_refused(&attempt, reason, &cause).await;
            }
        };
        let verified = match self.verifier.verify(token, &issuers).await {
            Ok(verified) => verified,
            Err(error) => {
                refuse(&attempt, "workload_token").await;
                return Err(PanelError::unauthenticated(format!(
                    "the workload token was refused: {}",
                    error.message
                )));
            }
        };
        attempt.username = verified.subject.chars().take(256).collect();
        let Some(trust) = trusts.iter().find(|trust| trust.admits(&verified)) else {
            refuse(&attempt, "workload_untrusted").await;
            return Err(PanelError::permission_denied(
                "no workload identity admits this token",
            ));
        };
        attempt.provider = Some(format!("workload/{}", trust.id));
        let account = self
            .identity
            .account(trust.account)
            .await?
            .map(|stored| stored.account)
            .filter(|account| account.service && !account.disabled);
        let Some(account) = account else {
            refuse(&attempt, "disabled").await;
            return Err(PanelError::permission_denied(
                "the workload's service account is disabled",
            ));
        };
        attempt.username = account.username.to_string();
        let now = (self.clock)();
        let secret = Secret::generate()?;
        let session = Session {
            id: SessionId::generate(),
            account: account.id,
            transport: Transport::Bearer,
            created_at: now,
            last_seen_at: now,
            expires_at: now + Duration::minutes(i64::from(trust.session_minutes)),
            client_address: client.address.clone(),
            user_agent: client.user_agent.clone(),
            revoked_at: None,
        };
        self.identity
            .create_session(
                NewSession {
                    session: session.clone(),
                    secret: secret.hash(),
                },
                &attempt,
                &cause(scope, account.username.as_str()),
            )
            .await?;
        Ok(Login {
            session,
            csrf: csrf_token(secret.expose()),
            secret,
            account,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn trust(subject: &str, claims: &[(&str, &str)]) -> WorkloadTrust {
        WorkloadTrust {
            id: "ci".into(),
            account: AccountId::generate(),
            issuer: "https://token.example".into(),
            audience: "pingora-panel".into(),
            subject: subject.into(),
            claims: claims
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            session_minutes: 15,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn token(subject: &str, claims: Value) -> VerifiedWorkload {
        VerifiedWorkload {
            issuer: "https://token.example".into(),
            subject: subject.into(),
            audiences: vec!["pingora-panel".into()],
            claims: claims.as_object().cloned().unwrap_or_default(),
        }
    }

    #[test]
    fn trusts_admit_matching_subjects_audiences_and_claims() {
        let main = token(
            "repo:shop/site:ref:refs/heads/main",
            json!({"repository": "shop/site", "run_attempt": 1, "protected": true}),
        );
        assert!(trust("repo:shop/site:ref:refs/heads/main", &[]).admits(&main));
        assert!(trust("repo:shop/site:*", &[("repository", "shop/site")]).admits(&main));
        assert!(trust(
            "repo:shop/site:*",
            &[("protected", "true"), ("run_attempt", "1")]
        )
        .admits(&main));
        assert!(!trust("repo:shop/site:*", &[("repository", "shop/other")]).admits(&main));
        assert!(!trust("repo:shop/other:*", &[]).admits(&main));
        assert!(!trust("repo:shop/site:*", &[("environment", "production")]).admits(&main));
        let mut elsewhere = main.clone();
        elsewhere.audiences = vec!["another-panel".into()];
        assert!(!trust("repo:shop/site:*", &[]).admits(&elsewhere));
        let mut disabled = trust("repo:shop/site:*", &[]);
        disabled.enabled = false;
        assert!(!disabled.admits(&main));
    }
}
