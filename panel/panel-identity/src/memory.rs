//! An identity store in memory, for tests and trials. It keeps the events
//! a durable store would record so tests can check them.

use crate::{
    built_in_roles, events,
    store::{
        AccountChange, AccountStore, Attempt, Cause, Failure, GrantStore, NewAccount, NewSession,
        NewToken, RoleStore, SessionGrant, SessionStore, SignInPolicyStore, StoredAccount,
        StoredPassword, TokenGrant, TokenStore,
    },
    Account, AccountId, ApiToken, Grant, GrantId, IdentityProvider, PasswordSignIn, PendingSignIn,
    PermissionSet, ProviderLink, ProviderSession, ProviderSignIn, ProviderStore, Role, SecretHash,
    Session, SessionId, TokenId, Username, WorkloadStore, WorkloadTrust,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use panel_events::EventData;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

/// An event as a durable store would record it.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedEvent {
    pub event_type: String,
    pub actor: String,
    pub data: Value,
}

struct State {
    roles: Vec<Role>,
    accounts: Vec<StoredAccount>,
    sessions: Vec<(Session, SecretHash)>,
    tokens: Vec<(ApiToken, SecretHash)>,
    providers: Vec<IdentityProvider>,
    pending: Vec<PendingSignIn>,
    links: Vec<ProviderLink>,
    /// Sessions signed in through a provider, with the sealed refresh token.
    provider_sessions: Vec<ProviderSessionRow>,
    password_sign_in: PasswordSignIn,
    trusts: Vec<WorkloadTrust>,
    grants: Vec<Grant>,
    events: Vec<RecordedEvent>,
}

struct ProviderSessionRow {
    session: SessionId,
    provider: String,
    refresh_token: Option<String>,
    checked_at: DateTime<Utc>,
}

pub struct MemoryIdentityStore {
    state: Mutex<State>,
}

impl Default for MemoryIdentityStore {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                roles: built_in_roles(),
                accounts: Vec::new(),
                sessions: Vec::new(),
                tokens: Vec::new(),
                providers: Vec::new(),
                pending: Vec::new(),
                links: Vec::new(),
                provider_sessions: Vec::new(),
                password_sign_in: PasswordSignIn::default(),
                trusts: Vec::new(),
                grants: Vec::new(),
                events: Vec::new(),
            }),
        }
    }
}

impl MemoryIdentityStore {
    pub fn events(&self) -> Vec<RecordedEvent> {
        self.state().events.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn permissions(state: &State, account: &Account) -> PermissionSet {
    state
        .roles
        .iter()
        .filter(|role| account.roles.contains(&role.id))
        .fold(PermissionSet::default(), |all, role| {
            all.union(&role.permissions)
        })
}

fn record<E: EventData>(state: &mut State, cause: &Cause, data: &E) {
    state.events.push(RecordedEvent {
        event_type: E::TYPE.into(),
        actor: cause.actor.clone(),
        data: serde_json::to_value(data).expect("event data serializes"),
    });
}

fn end_sessions(
    state: &mut State,
    account: AccountId,
    keep: Option<SessionId>,
    now: DateTime<Utc>,
) {
    for (session, _) in &mut state.sessions {
        if session.account == account && Some(session.id) != keep && session.revoked_at.is_none() {
            session.revoked_at = Some(now);
        }
    }
}

fn account_mut(state: &mut State, id: AccountId) -> Result<&mut StoredAccount> {
    state
        .accounts
        .iter_mut()
        .find(|stored| stored.account.id == id)
        .ok_or_else(|| PanelError::not_found(format!("there is no account {id}")))
}

#[async_trait]
impl AccountStore for MemoryIdentityStore {
    async fn has_accounts(&self) -> Result<bool> {
        Ok(!self.state().accounts.is_empty())
    }

    async fn create_account(&self, new: NewAccount, cause: &Cause) -> Result<Account> {
        let mut state = self.state();
        if new.first && !state.accounts.is_empty() {
            return Err(PanelError::conflict("the panel is already set up"));
        }
        if state
            .accounts
            .iter()
            .any(|stored| stored.account.username == new.username)
        {
            return Err(PanelError::conflict(format!(
                "the username {} is taken",
                new.username
            )));
        }
        let account = Account {
            id: new.id,
            username: new.username,
            display_name: new.display_name,
            disabled: false,
            locked: false,
            roles: new.roles,
            break_glass: false,
            service: new.service,
            created_at: new.now,
            updated_at: new.now,
            last_login_at: None,
            password_changed_at: new.password_hash.as_ref().map(|_| new.now),
        };
        state.accounts.push(StoredAccount {
            account: account.clone(),
            password: new.password_hash.map(|hash| StoredPassword {
                hash,
                failures: 0,
                retry_after: None,
            }),
        });
        record(&mut state, cause, &events::account_created(&account));
        Ok(account)
    }

    async fn account(&self, id: AccountId) -> Result<Option<StoredAccount>> {
        Ok(self
            .state()
            .accounts
            .iter()
            .find(|stored| stored.account.id == id)
            .cloned())
    }

    async fn account_named(&self, username: &Username) -> Result<Option<StoredAccount>> {
        Ok(self
            .state()
            .accounts
            .iter()
            .find(|stored| stored.account.username == *username)
            .cloned())
    }

    async fn accounts(&self) -> Result<Vec<Account>> {
        Ok(self
            .state()
            .accounts
            .iter()
            .map(|stored| stored.account.clone())
            .collect())
    }

    async fn update_account(
        &self,
        id: AccountId,
        change: AccountChange,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<Account> {
        let mut state = self.state();
        let stored = account_mut(&mut state, id)?;
        if let Some(display_name) = change.display_name.clone() {
            stored.account.display_name = display_name;
        }
        if let Some(disabled) = change.disabled {
            stored.account.disabled = disabled;
        }
        if let Some(roles) = change.roles.clone() {
            stored.account.roles = roles;
        }
        if let Some(break_glass) = change.break_glass {
            stored.account.break_glass = break_glass;
        }
        if change.unlock {
            stored.account.locked = false;
            if let Some(password) = &mut stored.password {
                password.failures = 0;
                password.retry_after = None;
            }
        }
        stored.account.updated_at = now;
        let account = stored.account.clone();
        if change.disabled == Some(true) {
            end_sessions(&mut state, id, None, now);
            for (token, _) in &mut state.tokens {
                if token.account == id && token.revoked_at.is_none() {
                    token.revoked_at = Some(now);
                }
            }
        }
        record(&mut state, cause, &events::account_updated(id, &change));
        Ok(account)
    }

    async fn set_password(
        &self,
        id: AccountId,
        hash: String,
        keep: Option<SessionId>,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<()> {
        let mut state = self.state();
        let stored = account_mut(&mut state, id)?;
        stored.password = Some(StoredPassword {
            hash,
            failures: 0,
            retry_after: None,
        });
        stored.account.locked = false;
        stored.account.password_changed_at = Some(now);
        end_sessions(&mut state, id, keep, now);
        record(&mut state, cause, &events::password_changed(id));
        Ok(())
    }

    async fn rehash_password(&self, id: AccountId, hash: String) -> Result<()> {
        let mut state = self.state();
        if let Some(password) = &mut account_mut(&mut state, id)?.password {
            password.hash = hash;
        }
        Ok(())
    }

    async fn login_failed(&self, failure: Failure, attempt: &Attempt, cause: &Cause) -> Result<()> {
        let mut state = self.state();
        let stored = account_mut(&mut state, failure.account)?;
        if let Some(password) = &mut stored.password {
            password.failures = failure.failures;
            password.retry_after = failure.retry_after;
        }
        stored.account.locked |= failure.lock;
        record(
            &mut state,
            cause,
            &events::wrong_password(attempt, &failure),
        );
        Ok(())
    }

    async fn login_refused(&self, attempt: &Attempt, reason: &str, cause: &Cause) {
        record(
            &mut self.state(),
            cause,
            &events::login_refused(attempt, reason),
        );
    }
}

#[async_trait]
impl SessionStore for MemoryIdentityStore {
    async fn create_session(
        &self,
        new: NewSession,
        attempt: &Attempt,
        cause: &Cause,
    ) -> Result<()> {
        let mut state = self.state();
        let stored = account_mut(&mut state, new.session.account)?;
        if let Some(password) = &mut stored.password {
            password.failures = 0;
            password.retry_after = None;
        }
        stored.account.last_login_at = Some(new.session.created_at);
        if attempt.break_glass {
            record(
                &mut state,
                cause,
                &events::break_glass_used(attempt, &new.session),
            );
        }
        record(
            &mut state,
            cause,
            &events::login_succeeded(attempt, &new.session),
        );
        state.sessions.push((new.session, new.secret));
        Ok(())
    }

    async fn session(&self, secret: &SecretHash) -> Result<Option<SessionGrant>> {
        let state = self.state();
        let Some((session, _)) = state
            .sessions
            .iter()
            .find(|(_, stored)| stored.matches(secret))
        else {
            return Ok(None);
        };
        let Some(stored) = state
            .accounts
            .iter()
            .find(|stored| stored.account.id == session.account)
        else {
            return Ok(None);
        };
        Ok(Some(SessionGrant {
            session: session.clone(),
            account: stored.account.clone(),
            permissions: permissions(&state, &stored.account),
        }))
    }

    async fn touch_session(&self, id: SessionId, at: DateTime<Utc>) -> Result<()> {
        for (session, _) in &mut self.state().sessions {
            if session.id == id {
                session.last_seen_at = at;
            }
        }
        Ok(())
    }

    async fn sessions(&self, account: AccountId, _now: DateTime<Utc>) -> Result<Vec<Session>> {
        Ok(self
            .state()
            .sessions
            .iter()
            .filter(|(session, _)| session.account == account && session.revoked_at.is_none())
            .map(|(session, _)| session.clone())
            .collect())
    }

    async fn end_session(
        &self,
        account: AccountId,
        id: SessionId,
        reason: &str,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<bool> {
        let mut state = self.state();
        let Some((session, _)) = state.sessions.iter_mut().find(|(session, _)| {
            session.id == id && session.account == account && session.revoked_at.is_none()
        }) else {
            return Ok(false);
        };
        session.revoked_at = Some(now);
        record(
            &mut state,
            cause,
            &events::session_ended(account, id, reason),
        );
        Ok(true)
    }

    async fn end_sessions(
        &self,
        account: AccountId,
        keep: Option<SessionId>,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<u64> {
        let mut state = self.state();
        let mut ended = 0;
        for (session, _) in &mut state.sessions {
            if session.account == account
                && Some(session.id) != keep
                && session.revoked_at.is_none()
            {
                session.revoked_at = Some(now);
                ended += 1;
            }
        }
        if ended > 0 {
            record(&mut state, cause, &events::sessions_revoked(account, ended));
        }
        Ok(ended)
    }
}

#[async_trait]
impl TokenStore for MemoryIdentityStore {
    async fn create_token(&self, new: NewToken, cause: &Cause) -> Result<()> {
        let mut state = self.state();
        record(&mut state, cause, &events::token_created(&new.token));
        state.tokens.push((new.token, new.secret));
        Ok(())
    }

    async fn token(&self, secret: &SecretHash) -> Result<Option<TokenGrant>> {
        let state = self.state();
        let Some((token, _)) = state
            .tokens
            .iter()
            .find(|(_, stored)| stored.matches(secret))
        else {
            return Ok(None);
        };
        let Some(stored) = state
            .accounts
            .iter()
            .find(|stored| stored.account.id == token.account)
        else {
            return Ok(None);
        };
        Ok(Some(TokenGrant {
            token: token.clone(),
            account: stored.account.clone(),
            permissions: permissions(&state, &stored.account),
        }))
    }

    async fn touch_token(&self, id: TokenId, at: DateTime<Utc>) -> Result<()> {
        for (token, _) in &mut self.state().tokens {
            if token.id == id {
                token.last_used_at = Some(at);
            }
        }
        Ok(())
    }

    async fn tokens(&self, account: AccountId) -> Result<Vec<ApiToken>> {
        Ok(self
            .state()
            .tokens
            .iter()
            .filter(|(token, _)| token.account == account)
            .map(|(token, _)| token.clone())
            .collect())
    }

    async fn revoke_token(
        &self,
        account: AccountId,
        id: TokenId,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<bool> {
        let mut state = self.state();
        let Some((token, _)) = state.tokens.iter_mut().find(|(token, _)| {
            token.id == id && token.account == account && token.revoked_at.is_none()
        }) else {
            return Ok(false);
        };
        token.revoked_at = Some(now);
        record(&mut state, cause, &events::token_revoked(account, id));
        Ok(true)
    }

    async fn rotate_token(
        &self,
        old: TokenId,
        new: NewToken,
        now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<bool> {
        let mut state = self.state();
        let account = new.token.account;
        let Some((token, _)) = state.tokens.iter_mut().find(|(token, _)| {
            token.id == old && token.account == account && token.revoked_at.is_none()
        }) else {
            return Ok(false);
        };
        token.revoked_at = Some(now);
        record(&mut state, cause, &events::token_rotated(&new.token, old));
        state.tokens.push((new.token, new.secret));
        Ok(true)
    }
}

#[async_trait]
impl RoleStore for MemoryIdentityStore {
    async fn roles(&self) -> Result<Vec<Role>> {
        Ok(self.state().roles.clone())
    }

    async fn create_role(&self, role: Role, cause: &Cause) -> Result<Role> {
        let mut state = self.state();
        if state.roles.iter().any(|existing| existing.id == role.id) {
            return Err(PanelError::conflict(format!("the role {} exists", role.id)));
        }
        state.roles.push(role.clone());
        record(&mut state, cause, &events::role_created(&role));
        Ok(role)
    }

    async fn update_role(&self, role: Role, cause: &Cause) -> Result<Role> {
        let mut state = self.state();
        let Some(existing) = state
            .roles
            .iter_mut()
            .find(|existing| existing.id == role.id && !existing.built_in)
        else {
            return Err(PanelError::not_found(format!(
                "there is no custom role {}",
                role.id
            )));
        };
        *existing = role.clone();
        record(&mut state, cause, &events::role_updated(&role));
        Ok(role)
    }

    async fn delete_role(&self, id: &str, cause: &Cause) -> Result<()> {
        let mut state = self.state();
        if !state
            .roles
            .iter()
            .any(|role| role.id == id && !role.built_in)
        {
            return Err(PanelError::not_found(format!(
                "there is no custom role {id}"
            )));
        }
        let holders = state
            .accounts
            .iter()
            .filter(|stored| stored.account.roles.iter().any(|role| role == id))
            .count();
        let holders = holders + state.grants.iter().filter(|grant| grant.role == id).count();
        if holders > 0 {
            return Err(PanelError::conflict(format!(
                "the role {id} is still granted to {holders} accounts"
            )));
        }
        state.roles.retain(|role| role.id != id);
        record(&mut state, cause, &events::role_deleted(id));
        Ok(())
    }
}

#[async_trait]
impl GrantStore for MemoryIdentityStore {
    async fn grants(&self, account: AccountId) -> Result<Vec<Grant>> {
        Ok(self
            .state()
            .grants
            .iter()
            .filter(|grant| grant.account == account)
            .cloned()
            .collect())
    }

    async fn create_grant(&self, grant: Grant, cause: &Cause) -> Result<()> {
        let mut state = self.state();
        record(&mut state, cause, &events::grant_created(&grant));
        state.grants.push(grant);
        Ok(())
    }

    async fn delete_grant(&self, account: AccountId, id: GrantId, cause: &Cause) -> Result<bool> {
        let mut state = self.state();
        let before = state.grants.len();
        state
            .grants
            .retain(|grant| !(grant.account == account && grant.id == id));
        if state.grants.len() == before {
            return Ok(false);
        }
        record(&mut state, cause, &events::grant_deleted(account, id));
        Ok(true)
    }
}

#[async_trait]
impl SignInPolicyStore for MemoryIdentityStore {
    async fn password_sign_in(&self) -> Result<PasswordSignIn> {
        Ok(self.state().password_sign_in)
    }

    async fn set_password_sign_in(
        &self,
        policy: PasswordSignIn,
        _now: DateTime<Utc>,
        cause: &Cause,
    ) -> Result<()> {
        let mut state = self.state();
        state.password_sign_in = policy;
        record(&mut state, cause, &events::sign_in_policy_updated(policy));
        Ok(())
    }
}

#[async_trait]
impl ProviderStore for MemoryIdentityStore {
    async fn providers(&self) -> Result<Vec<IdentityProvider>> {
        let mut providers = self.state().providers.clone();
        providers.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(providers)
    }

    async fn provider(&self, id: &str) -> Result<Option<IdentityProvider>> {
        Ok(self
            .state()
            .providers
            .iter()
            .find(|provider| provider.id == id)
            .cloned())
    }

    async fn put_provider(&self, provider: IdentityProvider, cause: &Cause) -> Result<bool> {
        let mut state = self.state();
        let (created_event, updated_event) = (
            events::provider_created(&provider),
            events::provider_updated(&provider),
        );
        let created = match state
            .providers
            .iter_mut()
            .find(|existing| existing.id == provider.id)
        {
            Some(existing) => {
                *existing = IdentityProvider {
                    created_at: existing.created_at,
                    ..provider
                };
                false
            }
            None => {
                state.providers.push(provider);
                true
            }
        };
        if !created_event.enabled {
            end_provider_sessions(&mut state, &created_event.provider);
        }
        if created {
            record(&mut state, cause, &created_event);
        } else {
            record(&mut state, cause, &updated_event);
        }
        Ok(created)
    }

    async fn delete_provider(&self, id: &str, cause: &Cause) -> Result<()> {
        let mut state = self.state();
        let before = state.providers.len();
        state.providers.retain(|provider| provider.id != id);
        if state.providers.len() == before {
            return Err(PanelError::not_found(format!(
                "there is no identity provider {id}"
            )));
        }
        state.links.retain(|link| link.provider != id);
        state.pending.retain(|pending| pending.provider != id);
        end_provider_sessions(&mut state, id);
        state.provider_sessions.retain(|row| row.provider != id);
        record(&mut state, cause, &events::provider_deleted(id));
        Ok(())
    }

    async fn save_sign_in(&self, pending: PendingSignIn) -> Result<()> {
        let mut state = self.state();
        let now = pending.expires_at;
        state
            .pending
            .retain(|kept| kept.expires_at > now - chrono::Duration::hours(1));
        state.pending.push(pending);
        Ok(())
    }

    async fn take_sign_in(
        &self,
        state_hash: &SecretHash,
        now: DateTime<Utc>,
    ) -> Result<Option<PendingSignIn>> {
        let mut state = self.state();
        let Some(index) = state
            .pending
            .iter()
            .position(|pending| pending.state.matches(state_hash))
        else {
            return Ok(None);
        };
        let pending = state.pending.remove(index);
        Ok((pending.expires_at > now).then_some(pending))
    }

    async fn link(&self, provider: &str, subject: &str) -> Result<Option<ProviderLink>> {
        Ok(self
            .state()
            .links
            .iter()
            .find(|link| link.provider == provider && link.subject == subject)
            .cloned())
    }

    async fn sign_in_with_provider(
        &self,
        sign_in: ProviderSignIn,
        attempt: &Attempt,
        cause: &Cause,
    ) -> Result<Account> {
        let mut state = self.state();
        let now = sign_in.session.session.created_at;
        if let Some(new) = sign_in.new_account {
            if state
                .accounts
                .iter()
                .any(|stored| stored.account.username == new.username)
            {
                return Err(PanelError::conflict(format!(
                    "the username {} is taken",
                    new.username
                )));
            }
            let account = Account {
                id: new.id,
                username: new.username,
                display_name: new.display_name,
                disabled: false,
                locked: false,
                roles: Vec::new(),
                break_glass: false,
                service: new.service,
                created_at: now,
                updated_at: now,
                last_login_at: None,
                password_changed_at: None,
            };
            state.accounts.push(StoredAccount {
                account: account.clone(),
                password: None,
            });
            record(
                &mut state,
                cause,
                &events::provider_account_created(
                    account.id,
                    &account.username,
                    &sign_in.link.provider,
                ),
            );
        }
        let stored = account_mut(&mut state, sign_in.link.account)?;
        if stored.account.disabled {
            return Err(PanelError::permission_denied("the account is disabled"));
        }
        if stored.account.roles != sign_in.roles {
            stored.account.roles = sign_in.roles;
            stored.account.updated_at = now;
        }
        stored.account.last_login_at = Some(now);
        let account = stored.account.clone();
        let link = sign_in.link;
        state
            .links
            .retain(|kept| !(kept.provider == link.provider && kept.subject == link.subject));
        let event =
            events::provider_login_succeeded(attempt, &sign_in.session.session, &link.provider);
        state.provider_sessions.push(ProviderSessionRow {
            session: sign_in.session.session.id,
            provider: link.provider.clone(),
            refresh_token: sign_in.refresh_token,
            checked_at: sign_in.session.session.created_at,
        });
        state.links.push(link);
        state
            .sessions
            .push((sign_in.session.session, sign_in.session.secret));
        record(&mut state, cause, &event);
        Ok(account)
    }

    async fn claim_rechecks(
        &self,
        checked_before: DateTime<Utc>,
        seen_after: DateTime<Utc>,
        now: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<ProviderSession>> {
        let mut guard = self.state();
        let state = &mut *guard;
        let live: HashMap<SessionId, AccountId> = state
            .sessions
            .iter()
            .map(|(session, _)| session)
            .filter(|session| {
                session.revoked_at.is_none()
                    && session.expires_at > now
                    && session.last_seen_at > seen_after
            })
            .map(|session| (session.id, session.account))
            .collect();
        let mut due: Vec<&mut ProviderSessionRow> = state
            .provider_sessions
            .iter_mut()
            .filter(|row| {
                row.refresh_token.is_some()
                    && row.checked_at < checked_before
                    && live.contains_key(&row.session)
            })
            .collect();
        due.sort_by_key(|row| row.checked_at);
        due.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(due
            .into_iter()
            .filter_map(|row| {
                row.checked_at = now;
                Some(ProviderSession {
                    session: row.session,
                    account: live[&row.session],
                    provider: row.provider.clone(),
                    refresh_token: row.refresh_token.clone()?,
                })
            })
            .collect())
    }

    async fn rotate_refresh_token(&self, session: SessionId, refresh_token: String) -> Result<()> {
        if let Some(row) = self
            .state()
            .provider_sessions
            .iter_mut()
            .find(|row| row.session == session)
        {
            row.refresh_token = Some(refresh_token);
        }
        Ok(())
    }
}

/// Ends the sessions signed in through `provider`.
fn end_provider_sessions(state: &mut State, provider: &str) {
    let ended: HashSet<SessionId> = state
        .provider_sessions
        .iter()
        .filter(|row| row.provider == provider)
        .map(|row| row.session)
        .collect();
    let now = Utc::now();
    for (session, _) in &mut state.sessions {
        if ended.contains(&session.id) && session.revoked_at.is_none() {
            session.revoked_at = Some(now);
        }
    }
}

#[async_trait]
impl WorkloadStore for MemoryIdentityStore {
    async fn trusts(&self) -> Result<Vec<WorkloadTrust>> {
        let mut trusts = self.state().trusts.clone();
        trusts.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(trusts)
    }

    async fn put_trust(&self, trust: WorkloadTrust, cause: &Cause) -> Result<bool> {
        let mut state = self.state();
        let (created_event, updated_event) = (
            events::workload_trust_created(&trust),
            events::workload_trust_updated(&trust),
        );
        let created = match state.trusts.iter_mut().find(|kept| kept.id == trust.id) {
            Some(kept) => {
                *kept = trust;
                false
            }
            None => {
                state.trusts.push(trust);
                true
            }
        };
        if created {
            record(&mut state, cause, &created_event);
        } else {
            record(&mut state, cause, &updated_event);
        }
        Ok(created)
    }

    async fn delete_trust(&self, id: &str, cause: &Cause) -> Result<()> {
        let mut state = self.state();
        let before = state.trusts.len();
        state.trusts.retain(|trust| trust.id != id);
        if state.trusts.len() == before {
            return Err(PanelError::not_found(format!(
                "there is no workload identity {id}"
            )));
        }
        record(&mut state, cause, &events::workload_trust_deleted(id));
        Ok(())
    }
}
