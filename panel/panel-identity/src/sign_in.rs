//! Signing in through identity providers (ADR 0018): the attempt kept until
//! the provider sends the person back, the account found by its link or
//! created for a new person, and the roles their groups grant.

use crate::{
    csrf_token,
    service::{cause, ensure_managers, role_permissions},
    store::{Attempt, NewAccount, NewSession},
    AccountId, Client, IdentityProvider, IdentityStore, Login, OpenIdConnect, PendingSignIn,
    ProviderDirectory, ProviderLink, ProviderSession, ProviderSignIn, ProviderStore, Refreshed,
    Secret, SecretHash, Session, SessionId, SessionPolicy, Transport, Username,
};
use chrono::{DateTime, Utc};
use panel_context::{RequestId, RequestScope};
use panel_errors::{ErrorCode, PanelError, Result};
use panel_secrets::{Sealed, SecretVault};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// How long a sign-in may take at the provider.
const ATTEMPT_LIFETIME: Duration = Duration::from_secs(600);

/// A provider listed on the sign-in page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignInOption {
    pub id: String,
    pub display_name: String,
}

/// Where to send the browser, and the state it must keep for the callback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Started {
    pub url: String,
    pub state: String,
}

type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// How often a provider is asked whether it still vouches for a session.
pub const RECHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// The most sessions one recheck takes.
const RECHECK_BATCH: u32 = 200;

/// What a recheck found.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rechecked {
    /// Still vouched for.
    pub kept: usize,
    /// Refused, and ended.
    pub ended: usize,
    /// Not answered for; tried again at a later recheck.
    pub unanswered: usize,
}

/// How far a sign-in got, for recording it when refused.
struct Progress {
    attempt: Attempt,
    reason: &'static str,
}

/// Sign-ins through identity providers.
#[derive(Clone)]
pub struct ProviderSignIns {
    directory: ProviderDirectory,
    providers: Arc<dyn ProviderStore>,
    identity: Arc<dyn IdentityStore>,
    connect: Arc<dyn OpenIdConnect>,
    vault: Arc<dyn SecretVault>,
    public_origin: String,
    sessions: SessionPolicy,
    clock: Clock,
}

/// The path to return to after signing in: one of the panel's own.
fn return_path(requested: Option<&str>) -> String {
    match requested {
        Some(path)
            if path.starts_with('/')
                && !path.starts_with("//")
                && !path.contains('\\')
                && !path.chars().any(char::is_control)
                && path.len() <= 2048 =>
        {
            path.to_owned()
        }
        _ => "/".to_owned(),
    }
}

fn claim<'a>(claims: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    claims
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// An account name from the provider's username claim; an email-like one
/// gives its local part.
fn username(value: &str) -> Result<Username> {
    Username::new(value)
        .or_else(|error| match value.split_once('@') {
            Some((local, _)) => Username::new(local),
            None => Err(error),
        })
        .map_err(|_| {
            PanelError::permission_denied(format!(
                "the provider names you {value:?}, which is not a valid account name; ask an Administrator to choose another username claim"
            ))
        })
}

fn groups(claims: &Map<String, Value>, name: &str) -> BTreeSet<String> {
    match claims.get(name) {
        Some(Value::Array(groups)) => groups
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(group)) => BTreeSet::from([group.clone()]),
        _ => BTreeSet::new(),
    }
}

fn duration(value: Duration) -> chrono::Duration {
    chrono::Duration::from_std(value).unwrap_or(chrono::Duration::MAX)
}

impl ProviderSignIns {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        directory: ProviderDirectory,
        providers: Arc<dyn ProviderStore>,
        identity: Arc<dyn IdentityStore>,
        connect: Arc<dyn OpenIdConnect>,
        vault: Arc<dyn SecretVault>,
        public_origin: impl Into<String>,
        sessions: SessionPolicy,
    ) -> Self {
        Self {
            directory,
            providers,
            identity,
            connect,
            vault,
            public_origin: public_origin.into().trim_end_matches('/').to_owned(),
            sessions,
            clock: Arc::new(Utc::now),
        }
    }

    /// Reads the time from `clock` instead of the system.
    pub fn with_clock(mut self, clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// The enabled providers, for the sign-in page.
    pub async fn options(&self) -> Result<Vec<SignInOption>> {
        Ok(self
            .providers
            .providers()
            .await?
            .into_iter()
            .filter(|provider| provider.enabled)
            .map(|provider| SignInOption {
                id: provider.id,
                display_name: provider.display_name,
            })
            .collect())
    }

    fn redirect_uri(&self, provider: &str) -> String {
        format!(
            "{}/api/v1/auth/oidc/{provider}/callback",
            self.public_origin
        )
    }

    async fn enabled(&self, id: &str) -> Result<IdentityProvider> {
        let provider = self.directory.stored(id).await?;
        if provider.enabled {
            Ok(provider)
        } else {
            Err(PanelError::permission_denied(format!(
                "signing in with {} is turned off",
                provider.display_name
            )))
        }
    }

    /// Starts a sign-in with `provider`.
    pub async fn start(&self, provider: &str, return_to: Option<&str>) -> Result<Started> {
        let stored = self.enabled(provider).await?;
        let settings = self
            .directory
            .settings(&stored, self.redirect_uri(provider))
            .await?;
        let request = self.connect.sign_in_request(&settings).await?;
        self.providers
            .save_sign_in(PendingSignIn {
                state: SecretHash::of(&request.state),
                provider: provider.to_owned(),
                nonce: request.nonce,
                verifier: request.verifier,
                return_to: return_path(return_to),
                expires_at: (self.clock)() + duration(ATTEMPT_LIFETIME),
            })
            .await?;
        Ok(Started {
            url: request.url,
            state: request.state,
        })
    }

    /// Completes a sign-in from the provider's callback. `browser_state` is
    /// the state the browser kept, which must equal the one returned.
    /// Refused sign-ins are recorded with the stage they failed at.
    #[allow(clippy::too_many_arguments)]
    pub async fn finish(
        &self,
        provider: &str,
        code: &str,
        state: &str,
        browser_state: &str,
        transport: Transport,
        client: &Client,
        scope: &RequestScope,
    ) -> Result<(Login, String)> {
        let mut progress = Progress {
            attempt: Attempt {
                username: String::new(),
                provider: Some(provider.to_owned()),
                break_glass: false,
                client_address: client.address.clone(),
                user_agent: client.user_agent.clone(),
            },
            reason: "provider_state",
        };
        let finished = self
            .complete_sign_in(
                provider,
                code,
                state,
                browser_state,
                transport,
                scope,
                &mut progress,
            )
            .await;
        if let Err(error) = &finished {
            let reason = if error.code.as_str() == ErrorCode::CONFLICT {
                "username_taken"
            } else {
                progress.reason
            };
            let cause = cause(scope, &progress.attempt.username);
            self.identity
                .login_refused(&progress.attempt, reason, &cause)
                .await;
        }
        finished
    }

    #[allow(clippy::too_many_arguments)]
    async fn complete_sign_in(
        &self,
        provider: &str,
        code: &str,
        state: &str,
        browser_state: &str,
        transport: Transport,
        scope: &RequestScope,
        progress: &mut Progress,
    ) -> Result<(Login, String)> {
        let refused = || {
            PanelError::permission_denied(
                "the sign-in was not started in this browser or has expired",
            )
        };
        if state.is_empty() || !bool::from(state.as_bytes().ct_eq(browser_state.as_bytes())) {
            return Err(refused());
        }
        let now = (self.clock)();
        let pending = self
            .providers
            .take_sign_in(&SecretHash::of(state), now)
            .await?
            .filter(|pending| pending.provider == provider)
            .ok_or_else(refused)?;
        progress.reason = "provider_disabled";
        let stored = self.enabled(provider).await?;
        let settings = self
            .directory
            .settings(&stored, self.redirect_uri(provider))
            .await?;
        progress.reason = "provider_refused";
        let signed_in = self
            .connect
            .complete(&settings, code, &pending.verifier, &pending.nonce)
            .await?;
        let claims = &signed_in.claims;
        progress.attempt.username = claim(claims, &stored.claims.username)
            .unwrap_or(&signed_in.subject)
            .chars()
            .take(Username::MAX_LEN)
            .collect();
        progress.reason = "sign_in";
        let mapped: BTreeSet<String> = {
            let groups = groups(claims, &stored.claims.groups);
            stored
                .group_roles
                .iter()
                .filter(|mapping| groups.contains(&mapping.group))
                .map(|mapping| mapping.role.clone())
                .collect()
        };
        let link = self.providers.link(provider, &signed_in.subject).await?;
        let (account_id, new_account, held, previously_granted) = match link {
            Some(link) => {
                let account = self
                    .identity
                    .account(link.account)
                    .await?
                    .ok_or_else(|| PanelError::corrupt_state("a provider link names no account"))?
                    .account;
                if account.disabled {
                    progress.reason = "disabled";
                    return Err(PanelError::permission_denied("your account is disabled"));
                }
                (account.id, None, account.roles, link.granted_roles)
            }
            None => {
                if !stored.create_accounts {
                    progress.reason = "unknown_account";
                    return Err(PanelError::permission_denied(
                        "no account is linked to you; ask an Administrator to create one",
                    ));
                }
                progress.reason = "no_username";
                let raw = claim(claims, &stored.claims.username).ok_or_else(|| {
                    PanelError::permission_denied(format!(
                        "the provider sent no {:?} claim to name your account",
                        stored.claims.username
                    ))
                })?;
                let id = AccountId::generate();
                let new = NewAccount {
                    id,
                    username: username(raw)?,
                    display_name: claim(claims, &stored.claims.display_name).map(str::to_owned),
                    password_hash: None,
                    roles: Vec::new(),
                    now,
                    first: false,
                    service: false,
                };
                (id, Some(new), Vec::new(), Vec::new())
            }
        };
        let granted_before: BTreeSet<String> = previously_granted.into_iter().collect();
        let manual: BTreeSet<String> = held
            .iter()
            .filter(|role| !granted_before.contains(*role))
            .cloned()
            .collect();
        let mut roles: Vec<String> = manual.union(&mapped).cloned().collect();
        let mut granted: Vec<String> = mapped.difference(&manual).cloned().collect();
        if new_account.is_none() && roles != held {
            let permissions = role_permissions(self.identity.as_ref()).await?;
            let accounts: Vec<(bool, Vec<String>)> = self
                .identity
                .accounts()
                .await?
                .into_iter()
                .map(|account| {
                    if account.id == account_id {
                        (account.disabled, roles.clone())
                    } else {
                        (account.disabled, account.roles)
                    }
                })
                .collect();
            if ensure_managers(&accounts, &permissions).is_err() {
                // Losing a group must not leave the panel without anyone
                // able to manage accounts; the roles stay until someone can.
                let kept: BTreeSet<String> = held.iter().cloned().collect();
                roles = kept.union(&mapped).cloned().collect();
                granted = granted_before
                    .union(&mapped)
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .filter(|role| roles.contains(role))
                    .collect();
            }
        }
        let secret = Secret::generate()?;
        let session = Session {
            id: SessionId::generate(),
            account: account_id,
            transport,
            created_at: now,
            last_seen_at: now,
            expires_at: now + duration(self.sessions.absolute),
            client_address: progress.attempt.client_address.clone(),
            user_agent: progress.attempt.user_agent.clone(),
            revoked_at: None,
        };
        let refresh_token = match signed_in.refresh_token {
            Some(token) => Some(
                self.vault
                    .seal(&refresh_owner(session.id), token.as_bytes())
                    .await?
                    .as_str()
                    .to_owned(),
            ),
            None => None,
        };
        let account = self
            .providers
            .sign_in_with_provider(
                ProviderSignIn {
                    link: ProviderLink {
                        provider: provider.to_owned(),
                        subject: signed_in.subject.clone(),
                        account: account_id,
                        granted_roles: granted,
                    },
                    new_account,
                    roles,
                    session: NewSession {
                        session: session.clone(),
                        secret: secret.hash(),
                    },
                    refresh_token,
                },
                &progress.attempt,
                &cause(scope, &progress.attempt.username),
            )
            .await?;
        Ok((
            Login {
                session,
                csrf: csrf_token(secret.expose()),
                secret,
                account,
            },
            pending.return_to,
        ))
    }

    /// Asks providers about the sessions they have not vouched for in
    /// [`RECHECK_INTERVAL`]. Refused sessions end; ones a provider could not
    /// answer for are tried again at a later recheck.
    pub async fn recheck(&self) -> Result<Rechecked> {
        let now = (self.clock)();
        let due = self
            .providers
            .claim_rechecks(
                now - duration(RECHECK_INTERVAL),
                now - duration(self.sessions.idle),
                now,
                RECHECK_BATCH,
            )
            .await?;
        let mut rechecked = Rechecked::default();
        for session in due {
            match self.recheck_one(&session, now).await {
                Ok(true) => rechecked.kept += 1,
                Ok(false) => rechecked.ended += 1,
                Err(_) => rechecked.unanswered += 1,
            }
        }
        Ok(rechecked)
    }

    /// Whether the provider still vouches for `session`, which ends if not.
    async fn recheck_one(&self, session: &ProviderSession, now: DateTime<Utc>) -> Result<bool> {
        let provider = self
            .providers
            .provider(&session.provider)
            .await?
            .filter(|provider| provider.enabled);
        let vouched = match provider {
            Some(provider) => {
                let settings = self
                    .directory
                    .settings(&provider, self.redirect_uri(&provider.id))
                    .await?;
                let owner = refresh_owner(session.session);
                let opened = self
                    .vault
                    .open(&owner, &Sealed::new(session.refresh_token.clone()))
                    .await?;
                let token = String::from_utf8(opened.to_vec())
                    .map_err(|_| PanelError::corrupt_state("the refresh token is not text"))?;
                match self.connect.refresh(&settings, &token).await? {
                    Refreshed::Valid { refresh_token } => {
                        if let Some(rotated) = refresh_token {
                            let sealed = self.vault.seal(&owner, rotated.as_bytes()).await?;
                            self.providers
                                .rotate_refresh_token(session.session, sealed.as_str().to_owned())
                                .await?;
                        }
                        true
                    }
                    Refreshed::Refused => false,
                }
            }
            None => false,
        };
        if !vouched {
            let scope = RequestScope::new(RequestId::new(Uuid::now_v7().to_string())?);
            self.identity
                .end_session(
                    session.account,
                    session.session,
                    "provider_refused",
                    now,
                    &cause(&scope, &format!("identity-provider/{}", session.provider)),
                )
                .await?;
        }
        Ok(vouched)
    }
}

/// Whom a session's sealed refresh token belongs to.
pub(crate) fn refresh_owner(session: SessionId) -> String {
    format!("session/{session}/refresh-token")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_stay_inside_the_panel_and_names_come_from_claims() {
        assert_eq!(return_path(Some("/sites?page=2")), "/sites?page=2");
        for outside in ["//evil.example", "https://evil.example", "/\\evil", "sites"] {
            assert_eq!(return_path(Some(outside)), "/", "{outside}");
        }
        assert_eq!(return_path(None), "/");
        assert_eq!(username("Alice").unwrap().as_str(), "alice");
        assert_eq!(username("alice@corp.example").unwrap().as_str(), "alice");
        assert!(username("ali ce").is_err());
        let claims = serde_json::json!({"groups": ["ops", "dev"], "group": "solo"});
        let claims = claims.as_object().unwrap();
        assert_eq!(groups(claims, "groups").len(), 2);
        assert_eq!(groups(claims, "group").len(), 1);
        assert!(groups(claims, "missing").is_empty());
    }
}
