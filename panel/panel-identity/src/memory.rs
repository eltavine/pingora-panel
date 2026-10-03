//! An identity store in memory, for tests and trials. It keeps the events
//! a durable store would record so tests can check them.

use crate::{
    built_in_roles,
    store::{
        AccountChange, Attempt, Cause, Failure, IdentityStore, NewAccount, NewSession, NewToken,
        SessionGrant, StoredAccount, StoredPassword, TokenGrant,
    },
    Account, AccountId, ApiToken, PermissionSet, Role, SecretHash, Session, SessionId, TokenId,
    Username,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use serde_json::{json, Value};
use std::sync::Mutex;

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
    events: Vec<RecordedEvent>,
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

fn record(state: &mut State, event_type: &str, cause: &Cause, data: Value) {
    state.events.push(RecordedEvent {
        event_type: event_type.into(),
        actor: cause.actor.clone(),
        data,
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
impl IdentityStore for MemoryIdentityStore {
    async fn has_accounts(&self) -> Result<bool> {
        Ok(!self.state().accounts.is_empty())
    }

    async fn roles(&self) -> Result<Vec<Role>> {
        Ok(self.state().roles.clone())
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
        record(
            &mut state,
            "identity.account.created",
            cause,
            json!({ "account": account.id, "username": account.username, "roles": account.roles }),
        );
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
        record(
            &mut state,
            "identity.account.updated",
            cause,
            json!({ "account": id, "disabled": change.disabled, "roles": change.roles, "unlocked": change.unlock }),
        );
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
        record(
            &mut state,
            "identity.password.changed",
            cause,
            json!({ "account": id }),
        );
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
            "identity.login.failed",
            cause,
            json!({ "attempt": attempt, "reason": "wrong_password", "failures": failure.failures, "locked": failure.lock }),
        );
        Ok(())
    }

    async fn login_refused(&self, attempt: &Attempt, reason: &str, cause: &Cause) {
        record(
            &mut self.state(),
            "identity.login.failed",
            cause,
            json!({ "attempt": attempt, "reason": reason }),
        );
    }

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
        let event = json!({
            "attempt": attempt,
            "session": new.session.id,
            "transport": new.session.transport,
        });
        state.sessions.push((new.session, new.secret));
        record(&mut state, "identity.login.succeeded", cause, event);
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
            "identity.session.ended",
            cause,
            json!({ "account": account, "session": id, "reason": reason }),
        );
        Ok(true)
    }

    async fn create_token(&self, new: NewToken, cause: &Cause) -> Result<()> {
        let mut state = self.state();
        let event = json!({
            "account": new.token.account,
            "token": new.token.id,
            "name": new.token.name,
            "permissions": new.token.permissions,
            "expires_at": new.token.expires_at,
        });
        state.tokens.push((new.token, new.secret));
        record(&mut state, "identity.token.created", cause, event);
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
        record(
            &mut state,
            "identity.token.revoked",
            cause,
            json!({ "account": account, "token": id }),
        );
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
        let event = json!({ "account": account, "token": new.token.id, "replaces": old });
        state.tokens.push((new.token, new.secret));
        record(&mut state, "identity.token.rotated", cause, event);
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
            record(
                &mut state,
                "identity.session.ended",
                cause,
                json!({ "account": account, "reason": "revoked", "sessions": ended }),
            );
        }
        Ok(ended)
    }

    async fn create_role(&self, role: Role, cause: &Cause) -> Result<Role> {
        let mut state = self.state();
        if state.roles.iter().any(|existing| existing.id == role.id) {
            return Err(PanelError::conflict(format!("the role {} exists", role.id)));
        }
        state.roles.push(role.clone());
        record(
            &mut state,
            "identity.role.created",
            cause,
            json!({ "role": role.id, "permissions": role.permissions }),
        );
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
        record(
            &mut state,
            "identity.role.updated",
            cause,
            json!({ "role": role.id, "permissions": role.permissions }),
        );
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
        if holders > 0 {
            return Err(PanelError::conflict(format!(
                "the role {id} is still granted to {holders} accounts"
            )));
        }
        state.roles.retain(|role| role.id != id);
        record(
            &mut state,
            "identity.role.deleted",
            cause,
            json!({ "role": id }),
        );
        Ok(())
    }
}
