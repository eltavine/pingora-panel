//! An authenticated caller and what it may do.

use crate::{
    secret::same_secret, Access, AccountId, HeldGrant, Permission, PermissionSet, SessionId,
    TokenId, Username,
};
use chrono::{DateTime, Utc};
use std::net::IpAddr;

/// How a request proved who it is.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Credential {
    /// A session cookie; unsafe requests must also present `csrf`.
    SessionCookie { session: SessionId, csrf: String },
    /// A session secret sent as a bearer token.
    SessionBearer { session: SessionId },
    /// An API token.
    Token { token: TokenId },
}

/// An authenticated caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Principal {
    pub account: AccountId,
    pub username: Username,
    pub credential: Credential,
    /// From the roles the account holds everywhere; the token's own for
    /// API tokens.
    pub permissions: PermissionSet,
    /// Grants with a scope or conditions; sessions only.
    pub grants: Vec<HeldGrant>,
}

impl Principal {
    /// What the principal may do for a request at `at` from `client`.
    pub fn access(&self, at: DateTime<Utc>, client: Option<IpAddr>) -> Access {
        Access::of(&self.permissions, &self.grants, at, client)
    }

    pub fn can(&self, permission: Permission) -> bool {
        self.permissions.contains(permission)
    }

    /// The actor recorded with what it does.
    pub fn actor(&self) -> &str {
        self.username.as_str()
    }

    pub fn session(&self) -> Option<SessionId> {
        match &self.credential {
            Credential::SessionCookie { session, .. } | Credential::SessionBearer { session } => {
                Some(*session)
            }
            Credential::Token { .. } => None,
        }
    }

    /// The CSRF token unsafe requests of a cookie session carry.
    pub fn csrf_token(&self) -> Option<&str> {
        match &self.credential {
            Credential::SessionCookie { csrf, .. } => Some(csrf),
            _ => None,
        }
    }

    /// Whether `presented` is this session's CSRF token; always true for
    /// credentials the browser does not send by itself.
    pub fn csrf_matches(&self, presented: Option<&str>) -> bool {
        match self.csrf_token() {
            Some(expected) => presented.is_some_and(|token| same_secret(token, expected)),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{csrf_token, Secret};

    #[test]
    fn only_cookie_sessions_need_their_csrf_token() {
        let secret = Secret::generate().unwrap();
        let csrf = csrf_token(secret.expose());
        let mut principal = Principal {
            account: AccountId::generate(),
            username: Username::new("alice").unwrap(),
            credential: Credential::SessionCookie {
                session: SessionId::generate(),
                csrf: csrf.clone(),
            },
            permissions: PermissionSet::from_names(&["config.read"]).unwrap(),
            grants: Vec::new(),
        };
        assert!(principal.can(Permission::ConfigRead));
        assert!(!principal.can(Permission::ConfigApply));
        assert!(principal.csrf_matches(Some(&csrf)));
        assert!(!principal.csrf_matches(Some("forged")));
        assert!(!principal.csrf_matches(None));
        assert_eq!(principal.csrf_token(), Some(csrf.as_str()));
        assert_eq!(principal.actor(), "alice");
        principal.credential = Credential::Token {
            token: TokenId::generate(),
        };
        assert!(principal.csrf_matches(None));
        assert_eq!(principal.session(), None);
        assert_eq!(principal.csrf_token(), None);
    }
}
