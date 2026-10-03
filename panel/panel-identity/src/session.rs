//! Sessions and API tokens: what a login or a token grant leaves behind.

use crate::{AccountId, PermissionSet};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{fmt, time::Duration};
use uuid::Uuid;

macro_rules! identifier {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
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

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

identifier!(
    /// Identifies a session; not its secret.
    SessionId
);
identifier!(
    /// Identifies an API token; not its secret.
    TokenId
);

/// How a session's secret travels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Transport {
    /// In a cookie the browser sends by itself; unsafe requests also need
    /// the session's CSRF token.
    Cookie,
    /// In the `Authorization` header, set by the client on every request.
    Bearer,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cookie => "cookie",
            Self::Bearer => "bearer",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "cookie" => Some(Self::Cookie),
            "bearer" => Some(Self::Bearer),
            _ => None,
        }
    }
}

/// How long sessions last.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionPolicy {
    /// Without activity for this long, a session ends.
    pub idle: Duration,
    /// A session ends this long after login regardless of activity.
    pub absolute: Duration,
    /// Activity is recorded at most this often.
    pub touch_every: Duration,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(3600),
            absolute: Duration::from_secs(24 * 3600),
            touch_every: Duration::from_secs(60),
        }
    }
}

/// A session, without its secret.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Session {
    pub id: SessionId,
    pub account: AccountId,
    pub transport: Transport,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    /// When it ends regardless of activity.
    pub expires_at: DateTime<Utc>,
    pub client_address: Option<String>,
    pub user_agent: Option<String>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl Session {
    /// When it ends unless there is activity first.
    pub fn idle_until(&self, policy: &SessionPolicy) -> DateTime<Utc> {
        let idle = chrono::Duration::from_std(policy.idle).unwrap_or(chrono::Duration::MAX);
        (self.last_seen_at + idle).min(self.expires_at)
    }

    pub fn is_live(&self, now: DateTime<Utc>, policy: &SessionPolicy) -> bool {
        self.revoked_at.is_none() && now < self.idle_until(policy)
    }
}

/// An API token, without its secret.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ApiToken {
    pub id: TokenId,
    pub account: AccountId,
    pub name: String,
    /// What it may do, never more than its owner may.
    pub permissions: PermissionSet,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl ApiToken {
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && now < self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_end_when_idle_or_old_or_revoked() {
        let policy = SessionPolicy::default();
        let start = DateTime::parse_from_rfc3339("2026-10-03T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let hours = |hours| start + chrono::Duration::hours(hours);
        let mut session = Session {
            id: SessionId::generate(),
            account: AccountId::generate(),
            transport: Transport::Cookie,
            created_at: start,
            last_seen_at: start,
            expires_at: hours(24),
            client_address: None,
            user_agent: None,
            revoked_at: None,
        };
        assert!(session.is_live(start + chrono::Duration::minutes(59), &policy));
        assert!(!session.is_live(hours(1), &policy));
        session.last_seen_at = hours(23) + chrono::Duration::minutes(30);
        assert_eq!(session.idle_until(&policy), hours(24));
        assert!(!session.is_live(hours(24), &policy));
        session.revoked_at = Some(hours(23) + chrono::Duration::minutes(40));
        assert!(!session.is_live(hours(23) + chrono::Duration::minutes(45), &policy));
        assert_eq!(Transport::parse("bearer"), Some(Transport::Bearer));
        assert_eq!(Transport::Cookie.as_str(), "cookie");
    }
}
