//! The events identity records (ADR 0004), built here once from the domain
//! for every store, so their data follows its Protobuf definition and never
//! the shape of a domain type.

use crate::{
    store::{AccountChange, Attempt, Failure},
    Account, AccountId, ApiToken, Grant, GrantId, GrantScope, IdentityProvider, PasswordSignIn,
    Role, Session, SessionId, TokenId, WorkloadTrust,
};
use panel_event_contracts::identity::v1 as event;

pub use event::{
    AccountCreated, AccountUpdated, BreakGlassUsed, GrantCreated, GrantDeleted, LoginFailed,
    LoginSucceeded, PasswordChanged, ProviderCreated, ProviderDeleted, ProviderUpdated,
    RoleCreated, RoleDeleted, RoleUpdated, SessionEnded, SignInPolicyUpdated, TokenCreated,
    TokenRevoked, TokenRotated, WorkloadTrustCreated, WorkloadTrustDeleted, WorkloadTrustUpdated,
};

fn attempt(attempt: &Attempt) -> event::Attempt {
    event::Attempt {
        username: attempt.username.clone(),
        provider: attempt.provider.clone(),
        break_glass: attempt.break_glass,
        client_address: attempt.client_address.clone(),
        user_agent: attempt.user_agent.clone(),
    }
}

pub fn account_created(account: &Account) -> AccountCreated {
    AccountCreated {
        account: account.id.to_string(),
        username: account.username.to_string(),
        roles: account.roles.clone(),
        provider: None,
    }
}

/// An account a sign-in through `provider` created.
pub fn provider_account_created(
    account: AccountId,
    username: impl ToString,
    provider: &str,
) -> AccountCreated {
    AccountCreated {
        account: account.to_string(),
        username: username.to_string(),
        roles: Vec::new(),
        provider: Some(provider.to_owned()),
    }
}

pub fn account_updated(account: AccountId, change: &AccountChange) -> AccountUpdated {
    AccountUpdated {
        account: account.to_string(),
        display_name: change
            .display_name
            .as_ref()
            .map(|name| name.clone().unwrap_or_default()),
        disabled: change.disabled,
        roles: change.roles.as_ref().map(|names| event::Roles {
            names: names.clone(),
        }),
        break_glass: change.break_glass,
        unlocked: change.unlock,
    }
}

pub fn password_changed(account: AccountId) -> PasswordChanged {
    PasswordChanged {
        account: account.to_string(),
    }
}

pub fn wrong_password(sign_in: &Attempt, failure: &Failure) -> LoginFailed {
    LoginFailed {
        attempt: Some(attempt(sign_in)),
        reason: "wrong_password".into(),
        failures: Some(failure.failures),
        locked: Some(failure.lock),
    }
}

/// A sign-in refused before any password was checked: `unknown`,
/// `disabled`, `locked` or `waiting`.
pub fn login_refused(sign_in: &Attempt, reason: &str) -> LoginFailed {
    LoginFailed {
        attempt: Some(attempt(sign_in)),
        reason: reason.to_owned(),
        failures: None,
        locked: None,
    }
}

pub fn login_succeeded(sign_in: &Attempt, session: &Session) -> LoginSucceeded {
    LoginSucceeded {
        attempt: Some(attempt(sign_in)),
        session: session.id.to_string(),
        transport: session.transport.as_str().into(),
    }
}

/// A sign-in through `provider`.
pub fn provider_login_succeeded(
    sign_in: &Attempt,
    session: &Session,
    provider: &str,
) -> LoginSucceeded {
    let mut event = login_succeeded(sign_in, session);
    if let Some(attempt) = &mut event.attempt {
        attempt.provider = Some(provider.to_owned());
    }
    event
}

pub fn break_glass_used(sign_in: &Attempt, session: &Session) -> BreakGlassUsed {
    BreakGlassUsed {
        attempt: Some(attempt(sign_in)),
        session: session.id.to_string(),
        transport: session.transport.as_str().into(),
    }
}

pub fn session_ended(account: AccountId, session: SessionId, reason: &str) -> SessionEnded {
    SessionEnded {
        account: account.to_string(),
        session: Some(session.to_string()),
        reason: reason.to_owned(),
        sessions: None,
    }
}

/// Every session of `account` but one it keeps was revoked.
pub fn sessions_revoked(account: AccountId, sessions: u64) -> SessionEnded {
    SessionEnded {
        account: account.to_string(),
        session: None,
        reason: "revoked".into(),
        sessions: Some(u32::try_from(sessions).unwrap_or(u32::MAX)),
    }
}

pub fn token_created(token: &ApiToken) -> TokenCreated {
    TokenCreated {
        account: token.account.to_string(),
        token: token.id.to_string(),
        name: token.name.clone(),
        permissions: names(&token.permissions),
        expires_at: Some(token.expires_at.into()),
    }
}

pub fn token_revoked(account: AccountId, token: TokenId) -> TokenRevoked {
    TokenRevoked {
        account: account.to_string(),
        token: token.to_string(),
    }
}

pub fn token_rotated(token: &ApiToken, replaces: TokenId) -> TokenRotated {
    TokenRotated {
        account: token.account.to_string(),
        token: token.id.to_string(),
        replaces: replaces.to_string(),
    }
}

fn names(permissions: &crate::PermissionSet) -> Vec<String> {
    permissions.names().into_iter().map(str::to_owned).collect()
}

pub fn role_created(role: &Role) -> RoleCreated {
    RoleCreated {
        role: role.id.clone(),
        permissions: names(&role.permissions),
    }
}

pub fn role_updated(role: &Role) -> RoleUpdated {
    RoleUpdated {
        role: role.id.clone(),
        permissions: names(&role.permissions),
    }
}

pub fn role_deleted(role: &str) -> RoleDeleted {
    RoleDeleted {
        role: role.to_owned(),
    }
}

pub fn grant_created(grant: &Grant) -> GrantCreated {
    let scope = match &grant.scope {
        GrantScope::SiteGroup { group } => event::GrantScope {
            kind: "site_group".into(),
            group: Some(group.clone()),
            site: None,
        },
        GrantScope::Site { site } => event::GrantScope {
            kind: "site".into(),
            group: None,
            site: Some(site.to_string()),
        },
        _ => event::GrantScope {
            kind: "everything".into(),
            group: None,
            site: None,
        },
    };
    GrantCreated {
        account: grant.account.to_string(),
        grant: grant.id.to_string(),
        role: grant.role.clone(),
        scope: Some(scope),
        conditions: Some(event::GrantConditions {
            not_after: grant.conditions.not_after.map(Into::into),
            networks: grant.conditions.networks.clone(),
            windows: grant
                .conditions
                .windows
                .iter()
                .map(|window| event::Window {
                    recurrence: window.recurrence().as_str().to_owned(),
                    minutes: u32::try_from(window.duration().as_secs() / 60).unwrap_or(u32::MAX),
                })
                .collect(),
        }),
    }
}

pub fn grant_deleted(account: AccountId, grant: GrantId) -> GrantDeleted {
    GrantDeleted {
        account: account.to_string(),
        grant: grant.to_string(),
    }
}

pub fn sign_in_policy_updated(policy: PasswordSignIn) -> SignInPolicyUpdated {
    SignInPolicyUpdated {
        password_sign_in: policy.as_str().into(),
    }
}

pub fn provider_created(provider: &IdentityProvider) -> ProviderCreated {
    ProviderCreated {
        provider: provider.id.clone(),
        issuer: provider.issuer.clone(),
        enabled: provider.enabled,
    }
}

pub fn provider_updated(provider: &IdentityProvider) -> ProviderUpdated {
    ProviderUpdated {
        provider: provider.id.clone(),
        issuer: provider.issuer.clone(),
        enabled: provider.enabled,
    }
}

pub fn provider_deleted(provider: &str) -> ProviderDeleted {
    ProviderDeleted {
        provider: provider.to_owned(),
    }
}

pub fn workload_trust_created(trust: &WorkloadTrust) -> WorkloadTrustCreated {
    WorkloadTrustCreated {
        workload_identity: trust.id.clone(),
        account: trust.account.to_string(),
        issuer: trust.issuer.clone(),
        audience: trust.audience.clone(),
        subject: trust.subject.clone(),
        claims: trust.claims.clone().into_iter().collect(),
        session_minutes: trust.session_minutes,
        enabled: trust.enabled,
    }
}

pub fn workload_trust_updated(trust: &WorkloadTrust) -> WorkloadTrustUpdated {
    let created = workload_trust_created(trust);
    WorkloadTrustUpdated {
        workload_identity: created.workload_identity,
        account: created.account,
        issuer: created.issuer,
        audience: created.audience,
        subject: created.subject,
        claims: created.claims,
        session_minutes: created.session_minutes,
        enabled: created.enabled,
    }
}

pub fn workload_trust_deleted(trust: &str) -> WorkloadTrustDeleted {
    WorkloadTrustDeleted {
        workload_identity: trust.to_owned(),
    }
}
