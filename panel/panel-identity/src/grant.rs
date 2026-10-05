//! Grants (ADR 0021): roles given with a scope and conditions, besides the
//! roles an account holds everywhere and always.

use crate::{AccountId, Permission, PermissionSet};
use chrono::{DateTime, Utc};
use ipnet::IpNet;
use panel_schedule::Window;
use serde::{Deserialize, Serialize};
use std::{fmt, net::IpAddr};
use uuid::Uuid;

/// The permissions a scope narrower than everything can limit; only sites
/// have groups.
pub const SCOPABLE: [Permission; 4] = [
    Permission::ConfigRead,
    Permission::ConfigWrite,
    Permission::ConfigApply,
    Permission::ConfigLua,
];
const MAX_NETWORKS: usize = 32;
const MAX_WINDOWS: usize = 16;
/// Identifies a grant.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GrantId(Uuid);

impl GrantId {
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

impl fmt::Display for GrantId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Where a grant applies.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum GrantScope {
    Everything,
    /// The sites whose `group` is this.
    SiteGroup {
        group: String,
    },
    Site {
        site: Uuid,
    },
}

/// When a grant counts; every condition set must hold.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantConditions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<DateTime<Utc>>,
    /// Networks such as `10.0.0.0/8` or single addresses the client must be
    /// in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<Window>,
}

fn network(value: &str) -> Option<IpNet> {
    value
        .parse::<IpNet>()
        .ok()
        .or_else(|| value.parse::<IpAddr>().ok().map(IpNet::from))
}

impl GrantConditions {
    /// Whether the grant counts for a request at `at` from `client`.
    pub fn hold(&self, at: DateTime<Utc>, client: Option<IpAddr>) -> bool {
        self.not_after.is_none_or(|end| at < end)
            && (self.networks.is_empty()
                || client.is_some_and(|client| {
                    self.networks
                        .iter()
                        .filter_map(|value| network(value))
                        .any(|network| network.contains(&client))
                }))
            && (self.windows.is_empty() || self.windows.iter().any(|window| window.contains(at)))
    }

    /// What is wrong with them; empty when nothing.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.networks.len() > MAX_NETWORKS || self.windows.len() > MAX_WINDOWS {
            problems.push(format!(
                "a grant names at most {MAX_NETWORKS} networks and {MAX_WINDOWS} windows"
            ));
        }
        for value in &self.networks {
            if network(value).is_none() {
                problems.push(format!("{value:?} is not a network such as 10.0.0.0/8"));
            }
        }
        problems
    }
}

/// A role given to an account with a scope and conditions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub id: GrantId,
    pub account: AccountId,
    pub role: String,
    pub scope: GrantScope,
    pub conditions: GrantConditions,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

/// A grant with its role's permissions, as a principal carries it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeldGrant {
    pub permissions: PermissionSet,
    pub scope: GrantScope,
    pub conditions: GrantConditions,
}

/// What a principal may do for one request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Access {
    /// Held everywhere.
    pub unrestricted: PermissionSet,
    /// Configuration permissions held only for some sites.
    pub scoped: Vec<(Permission, GrantScope)>,
}

impl Access {
    /// The scopes `permission` is limited to; empty when it is not held
    /// through scoped grants.
    pub fn scopes(&self, permission: Permission) -> impl Iterator<Item = &GrantScope> {
        self.scoped
            .iter()
            .filter(move |(held, _)| *held == permission)
            .map(|(_, scope)| scope)
    }

    /// Whether `permission` is held at all, everywhere or somewhere.
    pub fn holds(&self, permission: Permission) -> bool {
        self.unrestricted.contains(permission) || self.scopes(permission).next().is_some()
    }

    /// The access of roles held everywhere and the grants whose conditions
    /// hold at `at` for `client`.
    pub fn of(
        roles: &PermissionSet,
        grants: &[HeldGrant],
        at: DateTime<Utc>,
        client: Option<IpAddr>,
    ) -> Self {
        let mut access = Self {
            unrestricted: roles.clone(),
            scoped: Vec::new(),
        };
        for grant in grants
            .iter()
            .filter(|grant| grant.conditions.hold(at, client))
        {
            match &grant.scope {
                GrantScope::Everything => {
                    access.unrestricted = access
                        .unrestricted
                        .iter()
                        .chain(grant.permissions.iter())
                        .collect();
                }
                scope => {
                    for permission in SCOPABLE {
                        if grant.permissions.contains(permission)
                            && !access.scoped.contains(&(permission, scope.clone()))
                        {
                            access.scoped.push((permission, scope.clone()));
                        }
                    }
                }
            }
        }
        access
            .scoped
            .retain(|(permission, _)| !access.unrestricted.contains(*permission));
        access
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn grant(
        scope: GrantScope,
        conditions: GrantConditions,
        permissions: &[Permission],
    ) -> HeldGrant {
        HeldGrant {
            permissions: permissions.iter().copied().collect(),
            scope,
            conditions,
        }
    }

    #[test]
    fn conditions_limit_when_and_from_where_a_grant_counts() {
        let monday_noon = at("2026-10-05T12:00:00Z");
        let office: Option<IpAddr> = "10.1.2.3".parse().ok();
        let conditions = GrantConditions {
            not_after: Some(at("2026-10-06T00:00:00Z")),
            networks: vec!["10.0.0.0/8".into(), "192.0.2.7".into()],
            windows: vec![serde_json::from_value(serde_json::json!({
                "recurrence": "DTSTART:20260105T090000Z\nRRULE:FREQ=WEEKLY;BYDAY=MO",
                "minutes": 540
            }))
            .unwrap()],
        };
        assert!(conditions.problems().is_empty());
        assert!(conditions.hold(monday_noon, office));
        assert!(conditions.hold(monday_noon, "192.0.2.7".parse().ok()));
        assert!(!conditions.hold(monday_noon, "203.0.113.1".parse().ok()));
        assert!(!conditions.hold(monday_noon, None));
        assert!(!conditions.hold(at("2026-10-05T18:00:00Z"), office));
        assert!(
            !conditions.hold(at("2026-10-12T12:00:00Z"), office),
            "expired"
        );
        let wrong = GrantConditions {
            networks: vec!["10.0.0.0/33".into(); MAX_NETWORKS + 1],
            ..GrantConditions::default()
        };
        assert_eq!(wrong.problems().len(), 34, "{:?}", wrong.problems());
    }

    #[test]
    fn scoped_grants_limit_only_configuration_permissions() {
        use Permission::*;
        let roles: PermissionSet = [ConfigRead].into_iter().collect();
        let shop = GrantScope::SiteGroup {
            group: "shop".into(),
        };
        let grants = [
            grant(
                shop.clone(),
                GrantConditions::default(),
                &[ConfigRead, ConfigWrite, GatewayOperate],
            ),
            grant(
                GrantScope::Everything,
                GrantConditions {
                    not_after: Some(at("2026-01-01T00:00:00Z")),
                    ..GrantConditions::default()
                },
                &[ConfigApply],
            ),
        ];
        let access = Access::of(&roles, &grants, at("2026-10-05T12:00:00Z"), None);
        assert!(access.unrestricted.contains(ConfigRead));
        assert!(!access.unrestricted.contains(GatewayOperate));
        assert!(!access.holds(ConfigApply), "the everywhere grant expired");
        assert_eq!(access.scopes(ConfigWrite).collect::<Vec<_>>(), [&shop]);
        assert_eq!(
            access.scopes(ConfigRead).count(),
            0,
            "held everywhere, so not limited"
        );
    }
}
