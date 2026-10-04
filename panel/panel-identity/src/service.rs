//! The identity rules: setting up the first account, logging in,
//! authenticating requests, passwords, sessions, tokens and accounts.

use crate::{
    csrf_token,
    password::Verification,
    store::{
        AccountChange, Attempt, Cause, Failure, IdentityStore, NewAccount, NewSession, NewToken,
        StoredAccount,
    },
    time::duration,
    Account, AccountId, ApiToken, Credential, FailurePolicy, Grant, GrantConditions, GrantId,
    GrantScope, HeldGrant, PasswordHasher, PasswordPolicy, PasswordSignIn, Permission,
    PermissionSet, Principal, Role, Secret, SecretHash, Session, SessionId, SessionPolicy, TokenId,
    Transport, Username, SCOPABLE, TOKEN_PREFIX,
};
use chrono::{DateTime, Utc};
use panel_context::RequestScope;
use panel_errors::{PanelError, Result};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

#[derive(Clone, Debug)]
pub struct IdentitySettings {
    pub passwords: PasswordPolicy,
    pub sessions: SessionPolicy,
    pub failures: FailurePolicy,
    /// Keys password hashes; changing it invalidates every password.
    pub pepper: Option<Vec<u8>>,
    /// The hash of the one-time token that creates the first account.
    pub bootstrap: Option<SecretHash>,
    /// The longest an API token may live.
    pub max_token_lifetime: Duration,
}

impl Default for IdentitySettings {
    fn default() -> Self {
        Self {
            passwords: PasswordPolicy::default(),
            sessions: SessionPolicy::default(),
            failures: FailurePolicy::default(),
            pepper: None,
            bootstrap: None,
            max_token_lifetime: Duration::from_secs(365 * 24 * 3600),
        }
    }
}

/// Where a request comes from, as recorded with logins and sessions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Client {
    pub address: Option<String>,
    pub user_agent: Option<String>,
}

/// A new session and the secrets only its client receives.
#[derive(Debug)]
pub struct Login {
    pub session: Session,
    pub secret: Secret,
    /// Sent back in `x-csrf-token` with unsafe requests of cookie sessions.
    pub csrf: String,
    pub account: Account,
}

/// A new account.
#[derive(Clone, Debug, Default)]
pub struct AccountRequest {
    pub username: String,
    pub display_name: Option<String>,
    /// Without one the account cannot log in until a password is set.
    pub password: Option<String>,
    pub roles: Vec<String>,
    /// A service account, which never has a password.
    pub service: bool,
}

/// A role to give with a scope and conditions.
#[derive(Clone, Debug)]
pub struct GrantRequest {
    pub role: String,
    pub scope: GrantScope,
    pub conditions: GrantConditions,
}

/// A role to create or replace.
#[derive(Clone, Debug)]
pub struct RoleRequest {
    /// 1 to 64 lowercase letters, digits, `.`, `_` or `-`.
    pub id: String,
    pub name: String,
    pub description: String,
    pub permissions: PermissionSet,
}

impl RoleRequest {
    fn into_role(self) -> Result<Role> {
        let id = Username::new(&self.id)
            .map_err(|_| {
                PanelError::invalid_argument(format!(
                    "{:?} is not a role identifier: use 1 to 64 letters, digits, '.', '_' or '-'",
                    self.id
                ))
            })?
            .to_string();
        let name = self.name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(PanelError::invalid_argument(
                "a role needs a name of 1 to 64 characters",
            ));
        }
        if self.description.chars().count() > 256 {
            return Err(PanelError::invalid_argument(
                "a role's description has at most 256 characters",
            ));
        }
        if self.permissions.is_empty() {
            return Err(PanelError::invalid_argument(
                "a role needs at least one permission",
            ));
        }
        Ok(Role {
            id,
            name,
            description: self.description.trim().to_owned(),
            permissions: self.permissions,
            built_in: false,
        })
    }
}

/// Refuses a state in which no enabled account, given as whether it is
/// disabled and the roles it holds, can manage accounts.
pub(crate) fn ensure_managers(
    accounts: &[(bool, Vec<String>)],
    roles: &BTreeMap<String, PermissionSet>,
) -> Result<()> {
    let manages = |held: &[String]| {
        held.iter().any(|role| {
            roles
                .get(role)
                .is_some_and(|permissions| permissions.contains(Permission::IdentityManage))
        })
    };
    if accounts
        .iter()
        .any(|(disabled, held)| !disabled && manages(held))
    {
        Ok(())
    } else {
        Err(PanelError::precondition_failed(
            "at least one enabled account must be able to manage accounts",
        ))
    }
}

/// A new API token.
#[derive(Clone, Debug)]
pub struct TokenRequest {
    pub name: String,
    /// Its owner's permissions when absent.
    pub permissions: Option<PermissionSet>,
    pub lifetime: Duration,
}

type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

#[derive(Clone)]
pub struct Identity {
    store: Arc<dyn IdentityStore>,
    hasher: PasswordHasher,
    settings: Arc<IdentitySettings>,
    clock: Clock,
}

fn invalid_login() -> PanelError {
    PanelError::unauthenticated("invalid username or password")
}

/// The permissions of every role, by its identifier.
pub(crate) async fn role_permissions(
    store: &dyn IdentityStore,
) -> Result<BTreeMap<String, PermissionSet>> {
    Ok(store
        .roles()
        .await?
        .into_iter()
        .map(|role| (role.id, role.permissions))
        .collect())
}

pub(crate) fn cause(scope: &RequestScope, actor: &str) -> Cause {
    Cause {
        scope: scope.clone(),
        actor: actor.to_owned(),
    }
}

impl Identity {
    pub fn new(store: Arc<dyn IdentityStore>, settings: IdentitySettings) -> Self {
        Self {
            store,
            hasher: PasswordHasher::new(settings.pepper.clone()),
            settings: Arc::new(settings),
            clock: Arc::new(Utc::now),
        }
    }

    /// Reads the time from `clock` instead of the system.
    pub fn with_clock(mut self, clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    pub fn settings(&self) -> &IdentitySettings {
        &self.settings
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    fn check_password(
        &self,
        password: &str,
        username: &Username,
        display_name: Option<&str>,
    ) -> Result<()> {
        let mut context = vec![username.as_str()];
        context.extend(display_name.into_iter().flat_map(str::split_whitespace));
        self.settings
            .passwords
            .check(password, &context)
            .map_err(|problem| problem.into_error())
    }

    async fn known_roles(&self, roles: &[String]) -> Result<()> {
        let known = self.store.roles().await?;
        match roles
            .iter()
            .find(|role| !known.iter().any(|known| known.id == **role))
        {
            Some(unknown) => Err(PanelError::invalid_argument(format!(
                "there is no role {unknown:?}"
            ))),
            None => Ok(()),
        }
    }

    /// Whether the first account still has to be created.
    pub async fn setup_required(&self) -> Result<bool> {
        Ok(!self.store.has_accounts().await?)
    }

    /// Creates the first account, holding a role with every permission,
    /// given the one-time bootstrap token.
    pub async fn setup(
        &self,
        token: &str,
        request: AccountRequest,
        scope: &RequestScope,
    ) -> Result<Account> {
        let Some(expected) = &self.settings.bootstrap else {
            return Err(PanelError::precondition_failed(
                "no bootstrap token is configured",
            ));
        };
        if !SecretHash::of(token).matches(expected) {
            return Err(PanelError::permission_denied(
                "the bootstrap token is not valid",
            ));
        }
        if self.store.has_accounts().await? {
            return Err(PanelError::conflict("the panel is already set up"));
        }
        let username = Username::new(&request.username)?;
        let password = request
            .password
            .ok_or_else(|| PanelError::invalid_argument("the first account needs a password"))?;
        self.check_password(&password, &username, request.display_name.as_deref())?;
        let everything = PermissionSet::all();
        let role = self
            .store
            .roles()
            .await?
            .into_iter()
            .find(|role| role.permissions.is_superset(&everything))
            .ok_or_else(|| PanelError::corrupt_state("no role holds every permission"))?;
        let hash = self.hasher.hash(&password).await?;
        let actor = username.to_string();
        self.store
            .create_account(
                NewAccount {
                    id: AccountId::generate(),
                    username,
                    display_name: request.display_name,
                    password_hash: Some(hash),
                    roles: vec![role.id],
                    now: self.now(),
                    first: true,
                    service: false,
                },
                &cause(scope, &actor),
            )
            .await
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
        transport: Transport,
        client: &Client,
        scope: &RequestScope,
    ) -> Result<Login> {
        let mut attempt = Attempt {
            provider: None,
            break_glass: false,
            username: username.chars().take(Username::MAX_LEN).collect(),
            client_address: client.address.clone(),
            user_agent: client.user_agent.clone(),
        };
        let cause = cause(scope, &attempt.username);
        let stored = match Username::new(username) {
            Ok(name) => self.store.account_named(&name).await?,
            Err(_) => None,
        };
        let (account, stored_password) = match stored {
            Some(StoredAccount {
                account,
                password: Some(stored_password),
            }) if !account.disabled => (account, stored_password),
            other => {
                self.hasher.verify_nothing(password).await?;
                let reason = match other {
                    None => "unknown_account",
                    Some(stored) if stored.account.disabled => "disabled",
                    Some(_) => "no_password",
                };
                self.store.login_refused(&attempt, reason, &cause).await;
                return Err(invalid_login());
            }
        };
        let now = self.now();
        if account.locked {
            self.store.login_refused(&attempt, "locked", &cause).await;
            return Err(PanelError::permission_denied(
                "too many failed logins locked this account's password; an Administrator can unlock it",
            ));
        }
        if let Some(until) = stored_password.retry_after.filter(|until| now < *until) {
            self.store.login_refused(&attempt, "waiting", &cause).await;
            let seconds = (until - now).num_seconds().max(1);
            return Err(PanelError::resource_exhausted(format!(
                "too many failed logins; try again in {seconds} seconds"
            )));
        }
        match self.hasher.verify(password, &stored_password.hash).await? {
            Verification::Mismatch => {
                let failures = stored_password.failures.saturating_add(1);
                let failure = Failure {
                    account: account.id,
                    failures,
                    retry_after: self
                        .settings
                        .failures
                        .wait_after(failures)
                        .map(|wait| now + duration(wait)),
                    lock: self.settings.failures.disables(failures),
                };
                self.store.login_failed(failure, &attempt, &cause).await?;
                Err(invalid_login())
            }
            Verification::Matches { outdated } => {
                if !account.break_glass
                    && self.store.password_sign_in().await? == PasswordSignIn::BreakGlassOnly
                {
                    self.store
                        .login_refused(&attempt, "password_sign_in_limited", &cause)
                        .await;
                    return Err(PanelError::permission_denied(
                        "password sign-in is limited to break-glass accounts; continue with your identity provider",
                    ));
                }
                attempt.break_glass = account.break_glass;
                if outdated {
                    let hash = self.hasher.hash(password).await?;
                    self.store.rehash_password(account.id, hash).await?;
                }
                let secret = Secret::generate()?;
                let session = Session {
                    id: SessionId::generate(),
                    account: account.id,
                    transport,
                    created_at: now,
                    last_seen_at: now,
                    expires_at: now + duration(self.settings.sessions.absolute),
                    client_address: client.address.clone(),
                    user_agent: client.user_agent.clone(),
                    revoked_at: None,
                };
                self.store
                    .create_session(
                        NewSession {
                            session: session.clone(),
                            secret: secret.hash(),
                        },
                        &attempt,
                        &cause,
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
    }

    /// The principal a session secret stands for while the session lives.
    pub async fn authenticate_session(
        &self,
        secret: &str,
        transport: Transport,
    ) -> Result<Option<Principal>> {
        let Some(grant) = self.store.session(&SecretHash::of(secret)).await? else {
            return Ok(None);
        };
        let now = self.now();
        let policy = &self.settings.sessions;
        if grant.session.transport != transport
            || !grant.session.is_live(now, policy)
            || grant.account.disabled
        {
            return Ok(None);
        }
        if now - grant.session.last_seen_at >= duration(policy.touch_every) {
            self.store.touch_session(grant.session.id, now).await?;
        }
        let credential = match transport {
            Transport::Cookie => Credential::SessionCookie {
                session: grant.session.id,
                csrf: csrf_token(secret),
            },
            Transport::Bearer => Credential::SessionBearer {
                session: grant.session.id,
            },
        };
        Ok(Some(Principal {
            account: grant.account.id,
            username: grant.account.username,
            credential,
            permissions: grant.permissions,
            grants: self.held_grants(grant.account.id).await?,
        }))
    }

    /// The principal an API token stands for while it lives, holding the
    /// permissions both the token and its owner have.
    pub async fn authenticate_token(&self, secret: &str) -> Result<Option<Principal>> {
        if !secret.starts_with(TOKEN_PREFIX) {
            return Ok(None);
        }
        let Some(grant) = self.store.token(&SecretHash::of(secret)).await? else {
            return Ok(None);
        };
        let now = self.now();
        if !grant.token.is_live(now) || grant.account.disabled {
            return Ok(None);
        }
        let stale = grant
            .token
            .last_used_at
            .is_none_or(|used| now - used >= duration(self.settings.sessions.touch_every));
        if stale {
            self.store.touch_token(grant.token.id, now).await?;
        }
        Ok(Some(Principal {
            account: grant.account.id,
            username: grant.account.username,
            credential: Credential::Token {
                token: grant.token.id,
            },
            permissions: grant.token.permissions.intersection(&grant.permissions),
            grants: Vec::new(),
        }))
    }

    /// Ends the caller's own session.
    pub async fn logout(&self, principal: &Principal, scope: &RequestScope) -> Result<()> {
        let session = principal.session().ok_or_else(|| {
            PanelError::invalid_argument("an API token does not log out; revoke it instead")
        })?;
        self.store
            .end_session(
                principal.account,
                session,
                "logout",
                self.now(),
                &cause(scope, principal.actor()),
            )
            .await
            .map(|_| ())
    }

    /// Changes the caller's own password given the current one. Every
    /// other session of the account ends.
    pub async fn change_password(
        &self,
        principal: &Principal,
        current: &str,
        new: &str,
        scope: &RequestScope,
    ) -> Result<()> {
        if principal.session().is_none() {
            return Err(PanelError::permission_denied(
                "passwords are changed from a login session, not with an API token",
            ));
        }
        let stored = self
            .store
            .account(principal.account)
            .await?
            .ok_or_else(|| PanelError::unauthenticated("the account no longer exists"))?;
        let password = stored
            .password
            .ok_or_else(|| PanelError::precondition_failed("the account has no password"))?;
        if self.hasher.verify(current, &password.hash).await? == Verification::Mismatch {
            return Err(PanelError::permission_denied(
                "the current password is not correct",
            ));
        }
        self.check_password(
            new,
            &stored.account.username,
            stored.account.display_name.as_deref(),
        )?;
        let hash = self.hasher.hash(new).await?;
        self.store
            .set_password(
                principal.account,
                hash,
                principal.session(),
                self.now(),
                &cause(scope, principal.actor()),
            )
            .await
    }

    /// Sets an account's password on an administrator's behalf; all of the
    /// account's sessions end.
    pub async fn reset_password(
        &self,
        account: AccountId,
        new: &str,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<()> {
        let stored = self.stored(account).await?;
        if stored.account.service {
            return Err(PanelError::precondition_failed(
                "service accounts have no password; issue them API tokens",
            ));
        }
        self.check_password(
            new,
            &stored.account.username,
            stored.account.display_name.as_deref(),
        )?;
        let hash = self.hasher.hash(new).await?;
        self.store
            .set_password(account, hash, None, self.now(), &cause(scope, actor))
            .await
    }

    async fn stored(&self, account: AccountId) -> Result<StoredAccount> {
        self.store
            .account(account)
            .await?
            .ok_or_else(|| PanelError::not_found(format!("there is no account {account}")))
    }

    pub async fn account(&self, account: AccountId) -> Result<Account> {
        Ok(self.stored(account).await?.account)
    }

    pub async fn accounts(&self) -> Result<Vec<Account>> {
        self.store.accounts().await
    }

    pub async fn roles(&self) -> Result<Vec<Role>> {
        self.store.roles().await
    }

    pub async fn create_account(
        &self,
        request: AccountRequest,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<Account> {
        let username = Username::new(&request.username)?;
        self.known_roles(&request.roles).await?;
        if request.service && request.password.is_some() {
            return Err(PanelError::invalid_argument(
                "service accounts have no password; issue them API tokens",
            ));
        }
        let password_hash = match &request.password {
            Some(password) => {
                self.check_password(password, &username, request.display_name.as_deref())?;
                Some(self.hasher.hash(password).await?)
            }
            None => None,
        };
        self.store
            .create_account(
                NewAccount {
                    id: AccountId::generate(),
                    username,
                    display_name: request.display_name,
                    password_hash,
                    roles: request.roles,
                    now: self.now(),
                    first: false,
                    service: request.service,
                },
                &cause(scope, actor),
            )
            .await
    }

    /// Changes an account, keeping at least one enabled account that can
    /// manage accounts.
    pub async fn update_account(
        &self,
        id: AccountId,
        change: AccountChange,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<Account> {
        if let Some(roles) = &change.roles {
            self.known_roles(roles).await?;
        }
        let accounts = self.store.accounts().await?;
        if !accounts.iter().any(|account| account.id == id) {
            return Err(PanelError::not_found(format!("there is no account {id}")));
        }
        if change.break_glass == Some(true)
            && accounts
                .iter()
                .any(|account| account.id == id && account.service)
        {
            return Err(PanelError::invalid_argument(
                "service accounts never sign in, so they cannot be break-glass accounts",
            ));
        }
        let roles = self.role_permissions(None).await?;
        let after: Vec<(bool, bool, Vec<String>)> = accounts
            .into_iter()
            .map(|account| {
                if account.id == id {
                    (
                        change.disabled.unwrap_or(account.disabled),
                        change.break_glass.unwrap_or(account.break_glass),
                        change.roles.clone().unwrap_or(account.roles),
                    )
                } else {
                    (account.disabled, account.break_glass, account.roles)
                }
            })
            .collect();
        let managers = |break_glass_only: bool| -> Vec<(bool, Vec<String>)> {
            after
                .iter()
                .filter(|(_, break_glass, _)| *break_glass || !break_glass_only)
                .map(|(disabled, _, held)| (*disabled, held.clone()))
                .collect()
        };
        ensure_managers(&managers(false), &roles)?;
        if self.store.password_sign_in().await? == PasswordSignIn::BreakGlassOnly {
            ensure_managers(&managers(true), &roles).map_err(|_| {
                PanelError::precondition_failed(
                    "password sign-in is limited to break-glass accounts, so an enabled one must stay able to manage accounts",
                )
            })?;
        }
        self.store
            .update_account(id, change, self.now(), &cause(scope, actor))
            .await
    }

    /// Every role's permissions, with `replaced` in place of its stored
    /// version.
    async fn role_permissions(
        &self,
        replaced: Option<&Role>,
    ) -> Result<BTreeMap<String, PermissionSet>> {
        let mut roles = role_permissions(self.store.as_ref()).await?;
        if let Some(role) = replaced {
            roles.insert(role.id.clone(), role.permissions.clone());
        }
        Ok(roles)
    }

    /// Ends every session of an account but `keep`, such as the caller's
    /// own; returns how many ended.
    pub async fn end_sessions(
        &self,
        account: AccountId,
        keep: Option<SessionId>,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<u64> {
        self.store
            .end_sessions(account, keep, self.now(), &cause(scope, actor))
            .await
    }

    /// Creates a role of the caller's choosing.
    pub async fn create_role(
        &self,
        request: RoleRequest,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<Role> {
        let role = request.into_role()?;
        self.store.create_role(role, &cause(scope, actor)).await
    }

    /// Replaces a role that is not built in, keeping at least one enabled
    /// account that can manage accounts.
    pub async fn update_role(
        &self,
        request: RoleRequest,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<Role> {
        let role = request.into_role()?;
        self.custom_role(&role.id).await?;
        let roles = self.role_permissions(Some(&role)).await?;
        let accounts: Vec<(bool, Vec<String>)> = self
            .store
            .accounts()
            .await?
            .into_iter()
            .map(|account| (account.disabled, account.roles))
            .collect();
        ensure_managers(&accounts, &roles)?;
        self.store.update_role(role, &cause(scope, actor)).await
    }

    /// Deletes a role that is not built in and that no account holds.
    pub async fn delete_role(&self, id: &str, scope: &RequestScope, actor: &str) -> Result<()> {
        self.custom_role(id).await?;
        self.store.delete_role(id, &cause(scope, actor)).await
    }

    /// Refuses roles that do not exist or are built in.
    async fn custom_role(&self, id: &str) -> Result<()> {
        match self
            .store
            .roles()
            .await?
            .into_iter()
            .find(|role| role.id == id)
        {
            None => Err(PanelError::not_found(format!("there is no role {id:?}"))),
            Some(role) if role.built_in => Err(PanelError::permission_denied(
                "built-in roles cannot be changed or deleted",
            )),
            Some(_) => Ok(()),
        }
    }

    /// The sessions of an account that have not ended.
    pub async fn sessions(&self, account: AccountId) -> Result<Vec<Session>> {
        let now = self.now();
        let policy = self.settings.sessions;
        Ok(self
            .store
            .sessions(account, now)
            .await?
            .into_iter()
            .filter(|session| session.is_live(now, &policy))
            .collect())
    }

    pub async fn end_session(
        &self,
        account: AccountId,
        session: SessionId,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<()> {
        let ended = self
            .store
            .end_session(
                account,
                session,
                "revoked",
                self.now(),
                &cause(scope, actor),
            )
            .await?;
        if ended {
            Ok(())
        } else {
            Err(PanelError::not_found(format!(
                "there is no session {session}"
            )))
        }
    }

    pub async fn tokens(&self, account: AccountId) -> Result<Vec<ApiToken>> {
        self.store.tokens(account).await
    }

    /// Grants the caller a token. Tokens come from login sessions, never
    /// from other tokens, and cannot do more than their owner.
    pub async fn create_token(
        &self,
        principal: &Principal,
        request: TokenRequest,
        scope: &RequestScope,
    ) -> Result<(ApiToken, Secret)> {
        if principal.session().is_none() {
            return Err(PanelError::permission_denied(
                "API tokens are created from a login session, not with another token",
            ));
        }
        if self.stored(principal.account).await?.account.service {
            return Err(PanelError::permission_denied(
                "service accounts get API tokens from account managers",
            ));
        }
        self.new_token(
            principal.account,
            &principal.permissions,
            request,
            scope,
            principal.actor(),
        )
        .await
    }

    /// An account's grants with their roles' permissions, for its principal.
    async fn held_grants(&self, account: AccountId) -> Result<Vec<HeldGrant>> {
        let grants = self.store.grants(account).await?;
        if grants.is_empty() {
            return Ok(Vec::new());
        }
        let roles = self.role_permissions(None).await?;
        Ok(grants
            .into_iter()
            .map(|grant| HeldGrant {
                permissions: roles.get(&grant.role).cloned().unwrap_or_default(),
                scope: grant.scope,
                conditions: grant.conditions,
            })
            .collect())
    }

    pub async fn grants(&self, account: AccountId) -> Result<Vec<Grant>> {
        self.stored(account).await?;
        self.store.grants(account).await
    }

    /// Gives an account a role with a scope and conditions.
    pub async fn grant(
        &self,
        account: AccountId,
        request: GrantRequest,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<Grant> {
        self.stored(account).await?;
        self.known_roles(std::slice::from_ref(&request.role))
            .await?;
        let mut problems = request.conditions.problems();
        match &request.scope {
            GrantScope::SiteGroup { group } if group.trim().is_empty() || group.len() > 64 => {
                problems.push("a site group has a name of 1 to 64 characters".into());
            }
            GrantScope::Everything => {}
            _ => {
                let roles = self.role_permissions(None).await?;
                let permissions = roles.get(&request.role).cloned().unwrap_or_default();
                if !SCOPABLE
                    .iter()
                    .any(|permission| permissions.contains(*permission))
                {
                    problems.push(format!(
                        "the role {} grants nothing that can be limited to sites",
                        request.role
                    ));
                }
            }
        }
        if !problems.is_empty() {
            return Err(PanelError::invalid_argument(problems.join("; ")));
        }
        let grant = Grant {
            id: GrantId::generate(),
            account,
            role: request.role,
            scope: request.scope,
            conditions: request.conditions,
            created_at: self.now(),
            created_by: actor.to_owned(),
        };
        self.store
            .create_grant(grant.clone(), &cause(scope, actor))
            .await?;
        Ok(grant)
    }

    pub async fn revoke_grant(
        &self,
        account: AccountId,
        id: GrantId,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<()> {
        if self
            .store
            .delete_grant(account, id, &cause(scope, actor))
            .await?
        {
            Ok(())
        } else {
            Err(PanelError::not_found(format!("there is no grant {id}")))
        }
    }

    /// Issues an API token for a service account on an account manager's
    /// behalf, with at most the service account's permissions.
    pub async fn issue_token(
        &self,
        account: AccountId,
        request: TokenRequest,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<(ApiToken, Secret)> {
        let stored = self.stored(account).await?;
        if !stored.account.service {
            return Err(PanelError::precondition_failed(
                "only service accounts are issued tokens; people create their own",
            ));
        }
        if stored.account.disabled {
            return Err(PanelError::precondition_failed("the account is disabled"));
        }
        let roles = self.role_permissions(None).await?;
        let permissions: PermissionSet = stored
            .account
            .roles
            .iter()
            .filter_map(|role| roles.get(role))
            .flat_map(PermissionSet::iter)
            .collect();
        self.new_token(account, &permissions, request, scope, actor)
            .await
    }

    async fn new_token(
        &self,
        account: AccountId,
        allowed: &PermissionSet,
        request: TokenRequest,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<(ApiToken, Secret)> {
        let name = request.name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(PanelError::invalid_argument(
                "a token needs a name of 1 to 64 characters",
            ));
        }
        if request.lifetime < Duration::from_secs(3600)
            || request.lifetime > self.settings.max_token_lifetime
        {
            return Err(PanelError::invalid_argument(format!(
                "a token lives between an hour and {} days",
                self.settings.max_token_lifetime.as_secs() / 86_400
            )));
        }
        let permissions = request.permissions.unwrap_or_else(|| allowed.clone());
        if !allowed.is_superset(&permissions) {
            let missing: Vec<_> = permissions
                .iter()
                .filter(|permission| !allowed.contains(*permission))
                .map(Permission::name)
                .collect();
            return Err(PanelError::permission_denied(format!(
                "a token cannot have permissions its owner lacks: {}",
                missing.join(", ")
            )));
        }
        let now = self.now();
        let token = ApiToken {
            id: TokenId::generate(),
            account,
            name: name.to_owned(),
            permissions,
            created_at: now,
            expires_at: now + duration(request.lifetime),
            last_used_at: None,
            revoked_at: None,
        };
        let secret = Secret::token()?;
        self.store
            .create_token(
                NewToken {
                    token: token.clone(),
                    secret: secret.hash(),
                },
                &cause(scope, actor),
            )
            .await?;
        Ok((token, secret))
    }

    /// Replaces one of the caller's tokens by a new one with its name,
    /// permissions and lifetime and a new secret; the old one stops
    /// working at once.
    pub async fn rotate_token(
        &self,
        principal: &Principal,
        token: TokenId,
        scope: &RequestScope,
    ) -> Result<(ApiToken, Secret)> {
        if principal.session().is_none() {
            return Err(PanelError::permission_denied(
                "API tokens are rotated from a login session, not with another token",
            ));
        }
        let now = self.now();
        let old = self
            .store
            .tokens(principal.account)
            .await?
            .into_iter()
            .find(|candidate| candidate.id == token && candidate.is_live(now))
            .ok_or_else(|| PanelError::not_found(format!("there is no live token {token}")))?;
        let permissions = old.permissions.intersection(&principal.permissions);
        if permissions.is_empty() {
            return Err(PanelError::permission_denied(
                "you no longer hold any of the token's permissions",
            ));
        }
        let lifetime =
            (old.expires_at - old.created_at).min(duration(self.settings.max_token_lifetime));
        let new = ApiToken {
            id: TokenId::generate(),
            account: principal.account,
            name: old.name,
            permissions,
            created_at: now,
            expires_at: now + lifetime,
            last_used_at: None,
            revoked_at: None,
        };
        let secret = Secret::token()?;
        let rotated = self
            .store
            .rotate_token(
                token,
                NewToken {
                    token: new.clone(),
                    secret: secret.hash(),
                },
                now,
                &cause(scope, principal.actor()),
            )
            .await?;
        if rotated {
            Ok((new, secret))
        } else {
            Err(PanelError::not_found(format!(
                "there is no live token {token}"
            )))
        }
    }

    pub async fn revoke_token(
        &self,
        account: AccountId,
        token: TokenId,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<()> {
        if self
            .store
            .revoke_token(account, token, self.now(), &cause(scope, actor))
            .await?
        {
            Ok(())
        } else {
            Err(PanelError::not_found(format!("there is no token {token}")))
        }
    }
}
