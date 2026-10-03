//! Accounts and the names they log in with.

use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use uuid::Uuid;

/// Identifies an account.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(Uuid);

impl AccountId {
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }

    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A login name: 1 to 64 lowercase ASCII letters, digits, `.`, `_` or `-`,
/// starting with a letter or digit. Names are lowercased when read and
/// never change, so they identify the actor in the audit trail.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Username(String);

impl Username {
    pub const MAX_LEN: usize = 64;

    pub fn new(value: &str) -> Result<Self> {
        let name = value.to_ascii_lowercase();
        let valid = !name.is_empty()
            && name.len() <= Self::MAX_LEN
            && name
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphanumeric())
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
        if valid {
            Ok(Self(name))
        } else {
            Err(PanelError::invalid_argument(format!(
                "{value:?} is not a username: use 1 to {} letters, digits, '.', '_' or '-', starting with a letter or digit",
                Self::MAX_LEN
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Username {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Username {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(|error| serde::de::Error::custom(error.message))
    }
}

/// An account as administrators see it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Account {
    pub id: AccountId,
    pub username: Username,
    pub display_name: Option<String>,
    /// A disabled account cannot log in, and its sessions and tokens stop
    /// working.
    pub disabled: bool,
    /// Too many failed logins disabled its password until an Administrator
    /// unlocks it.
    pub locked: bool,
    /// The roles it holds, by identifier.
    pub roles: Vec<String>,
    /// Keeps password sign-in when it is limited to break-glass accounts;
    /// every sign-in with it is recorded for review.
    pub break_glass: bool,
    /// Belongs to a program: it never signs in, and account managers issue
    /// its API tokens.
    pub service: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub password_changed_at: Option<DateTime<Utc>>,
}

/// Who may sign in with a password.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PasswordSignIn {
    /// Every account that has a password.
    #[default]
    Everyone,
    /// Break-glass accounts only; everyone else signs in through an identity
    /// provider.
    BreakGlassOnly,
}

impl PasswordSignIn {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Everyone => "everyone",
            Self::BreakGlassOnly => "break_glass_only",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "everyone" => Some(Self::Everyone),
            "break_glass_only" => Some(Self::BreakGlassOnly),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_are_lowercase_and_restricted() {
        assert_eq!(Username::new("Alice.Ops").unwrap().as_str(), "alice.ops");
        assert_eq!(Username::new("ops-2_b").unwrap().to_string(), "ops-2_b");
        for invalid in ["", ".alice", "-x", "a b", "ålice", "a/b", &"a".repeat(65)] {
            assert!(Username::new(invalid).is_err(), "{invalid:?}");
        }
        let parsed: Username = serde_json::from_str("\"Root\"").unwrap();
        assert_eq!(parsed.as_str(), "root");
        assert!(serde_json::from_str::<Username>("\"no way\"").is_err());
    }
}
