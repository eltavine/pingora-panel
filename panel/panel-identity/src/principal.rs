//! An authenticated caller and what it may do.

use crate::{AccountId, Permission, PermissionSet, SecretHash, SessionId, TokenId, Username};

/// How a request proved who it is.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Credential {
    /// A session cookie; unsafe requests must also present its CSRF token.
    SessionCookie {
        session: SessionId,
        csrf: SecretHash,
    },
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
    pub permissions: PermissionSet,
}

impl Principal {
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

    /// Whether `presented` is this session's CSRF token; always true for
    /// credentials the browser does not send by itself.
    pub fn csrf_matches(&self, presented: Option<&str>) -> bool {
        match &self.credential {
            Credential::SessionCookie { csrf, .. } => {
                presented.is_some_and(|token| SecretHash::of(token).matches(csrf))
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cookie_sessions_need_their_csrf_token() {
        let csrf = crate::Secret::generate().unwrap();
        let mut principal = Principal {
            account: AccountId::generate(),
            username: Username::new("alice").unwrap(),
            credential: Credential::SessionCookie {
                session: SessionId::generate(),
                csrf: csrf.hash(),
            },
            permissions: PermissionSet::from_names(&["config.read"]).unwrap(),
        };
        assert!(principal.can(Permission::ConfigRead));
        assert!(!principal.can(Permission::ConfigApply));
        assert!(principal.csrf_matches(Some(csrf.expose())));
        assert!(!principal.csrf_matches(Some("forged")));
        assert!(!principal.csrf_matches(None));
        assert_eq!(principal.actor(), "alice");
        principal.credential = Credential::Token {
            token: TokenId::generate(),
        };
        assert!(principal.csrf_matches(None));
        assert_eq!(principal.session(), None);
    }
}
