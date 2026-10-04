//! The permission catalog and roles. Decisions are made with permissions
//! only; roles are data that group them.

use panel_errors::{PanelError, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;

/// An action the API guards.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Permission {
    GatewayRead,
    GatewayOperate,
    GatewayPublish,
    ConfigRead,
    ConfigWrite,
    ConfigApply,
    ApprovalManage,
    ApprovalDecide,
    ApprovalBypass,
    CertificateRead,
    CertificateManage,
    AuditRead,
    LogsRead,
    LogsDelete,
    AlertsRead,
    AlertsManage,
    HostRead,
    HostManage,
    ContainersRead,
    ContainersInspect,
    ContainersManage,
    PlatformRead,
    IdentityRead,
    IdentityManage,
}

/// Every permission with its name and what it allows.
const CATALOG: &[(Permission, &str, &str)] = &[
    (
        Permission::GatewayRead,
        "gateway.read",
        "Read the gateway's status, data plane and upstream health.",
    ),
    (
        Permission::GatewayOperate,
        "gateway.operate",
        "Reload the gateway, change its workers, drain and restore nodes, and shut it down.",
    ),
    (
        Permission::GatewayPublish,
        "gateway.publish",
        "Validate, prepare, activate and abort runtime snapshots directly.",
    ),
    (
        Permission::ConfigRead,
        "config.read",
        "Read sites, upstreams, listeners, TLS profiles, the draft, its files and revisions.",
    ),
    (
        Permission::ConfigWrite,
        "config.write",
        "Change the draft: its resources, files and revision notes.",
    ),
    (
        Permission::ConfigApply,
        "config.apply",
        "Apply the draft to the gateway, run dry runs and roll back.",
    ),
    (
        Permission::ApprovalManage,
        "approval.manage",
        "Create, change and delete the policies that decide which changes need approval.",
    ),
    (
        Permission::ApprovalDecide,
        "approval.decide",
        "Approve or reject changes other people asked to apply.",
    ),
    (
        Permission::ApprovalBypass,
        "approval.bypass",
        "Apply a change without its approvals in an emergency, giving a reason and an incident.",
    ),
    (
        Permission::CertificateRead,
        "certificate.read",
        "Read certificates, their names, validity and fingerprints, and check the hosts they cover.",
    ),
    (
        Permission::CertificateManage,
        "certificate.manage",
        "Upload, generate, replace and delete certificates; private keys are never returned.",
    ),
    (
        Permission::AuditRead,
        "audit.read",
        "Read and verify the audit trail.",
    ),
    (
        Permission::LogsRead,
        "logs.read",
        "Search, follow and download the gateway's access and error logs.",
    ),
    (
        Permission::LogsDelete,
        "logs.delete",
        "Delete the gateway's logs of a site or of every site.",
    ),
    (
        Permission::AlertsRead,
        "alerts.read",
        "Read alert rules, where they stand, their channels and the notifications sent.",
    ),
    (
        Permission::AlertsManage,
        "alerts.manage",
        "Change alert rules and channels and send test notifications.",
    ),
    (
        Permission::HostRead,
        "host.read",
        "Read the host's figures and what its agent reports: the panel's directories, what holds ports and the gateway's service.",
    ),
    (
        Permission::HostManage,
        "host.manage",
        "Start, stop and restart the gateway's service on the host.",
    ),
    (
        Permission::ContainersRead,
        "containers.read",
        "Read the container engines and their containers, images, networks, volumes and Compose projects.",
    ),
    (
        Permission::ContainersInspect,
        "containers.inspect",
        "Read containers' logs and details and Compose files, which can hold secrets.",
    ),
    (
        Permission::ContainersManage,
        "containers.manage",
        "Enable engines and start, stop, remove and prune what runs on them.",
    ),
    (
        Permission::PlatformRead,
        "platform.read",
        "Read the services of the control plane.",
    ),
    (
        Permission::IdentityRead,
        "identity.read",
        "Read accounts, their roles and sessions.",
    ),
    (
        Permission::IdentityManage,
        "identity.manage",
        "Create, change, disable and unlock accounts, grant roles and end sessions.",
    ),
];

impl Permission {
    /// Every permission, in catalog order.
    pub fn all() -> impl Iterator<Item = Permission> {
        CATALOG.iter().map(|(permission, _, _)| *permission)
    }

    fn entry(self) -> &'static (Permission, &'static str, &'static str) {
        CATALOG
            .iter()
            .find(|(permission, _, _)| *permission == self)
            .expect("every permission is in the catalog")
    }

    pub fn name(self) -> &'static str {
        self.entry().1
    }

    pub fn description(self) -> &'static str {
        self.entry().2
    }

    pub fn parse(name: &str) -> Option<Self> {
        CATALOG
            .iter()
            .find(|(_, candidate, _)| *candidate == name)
            .map(|(permission, _, _)| *permission)
    }
}

impl Serialize for Permission {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for Permission {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::parse(&name)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown permission {name:?}")))
    }
}

/// A set of permissions.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct PermissionSet(BTreeSet<Permission>);

impl PermissionSet {
    pub fn all() -> Self {
        Permission::all().collect()
    }

    /// The permissions named, refusing names outside the catalog.
    pub fn from_names<S: AsRef<str>>(names: &[S]) -> Result<Self> {
        names
            .iter()
            .map(|name| {
                Permission::parse(name.as_ref()).ok_or_else(|| {
                    PanelError::invalid_argument(format!("unknown permission {:?}", name.as_ref()))
                })
            })
            .collect()
    }

    pub fn contains(&self, permission: Permission) -> bool {
        self.0.contains(&permission)
    }

    pub fn is_superset(&self, other: &Self) -> bool {
        self.0.is_superset(&other.0)
    }

    pub fn intersection(&self, other: &Self) -> Self {
        Self(self.0.intersection(&other.0).copied().collect())
    }

    pub fn union(&self, other: &Self) -> Self {
        Self(self.0.union(&other.0).copied().collect())
    }

    pub fn iter(&self) -> impl Iterator<Item = Permission> + '_ {
        self.0.iter().copied()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.iter().map(Permission::name).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<Permission> for PermissionSet {
    fn from_iter<I: IntoIterator<Item = Permission>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// A named set of permissions an account can hold.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Role {
    /// Its identifier, such as `operator`.
    pub id: String,
    pub name: String,
    pub description: String,
    pub permissions: PermissionSet,
    /// Shipped with the panel; it cannot be removed.
    pub built_in: bool,
}

/// The roles the panel ships, which stores keep up to date.
pub fn built_in_roles() -> Vec<Role> {
    use Permission::*;
    let role = |id: &str, name: &str, description: &str, permissions: PermissionSet| Role {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        permissions,
        built_in: true,
    };
    vec![
        role(
            "administrator",
            "Administrator",
            "Everything, accounts and roles included.",
            PermissionSet::all(),
        ),
        role(
            "operator",
            "Operator",
            "Changes and applies configuration, decides on others' changes, manages certificates, operates the gateway, keeps its logs and its alerts.",
            [
                GatewayRead,
                GatewayOperate,
                ConfigRead,
                ConfigWrite,
                ConfigApply,
                ApprovalDecide,
                CertificateRead,
                CertificateManage,
                LogsRead,
                LogsDelete,
                AlertsRead,
                AlertsManage,
                HostRead,
                ContainersRead,
                ContainersInspect,
                ContainersManage,
                PlatformRead,
            ]
            .into_iter()
            .collect(),
        ),
        role(
            "viewer",
            "Viewer",
            "Reads configuration, certificates, the gateway's state, its logs and its alerts.",
            [
                GatewayRead,
                ConfigRead,
                CertificateRead,
                LogsRead,
                AlertsRead,
                HostRead,
                ContainersRead,
                PlatformRead,
            ]
            .into_iter()
            .collect(),
        ),
        role(
            "auditor",
            "Auditor",
            "Reads the audit trail, the gateway's logs and alerts, configuration, certificates and accounts.",
            [
                AuditRead,
                LogsRead,
                AlertsRead,
                HostRead,
                ContainersRead,
                ConfigRead,
                CertificateRead,
                GatewayRead,
                PlatformRead,
                IdentityRead,
            ]
            .into_iter()
            .collect(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_names_every_permission_once() {
        let names: BTreeSet<_> = Permission::all().map(Permission::name).collect();
        assert_eq!(names.len(), CATALOG.len());
        for permission in Permission::all() {
            assert_eq!(Permission::parse(permission.name()), Some(permission));
            assert!(permission.description().ends_with('.'));
        }
        assert_eq!(Permission::parse("config.delete"), None);
    }

    #[test]
    fn sets_serialize_as_names_and_refuse_unknown_ones() {
        let set = PermissionSet::from_names(&["config.read", "audit.read"]).unwrap();
        assert_eq!(
            serde_json::to_string(&set).unwrap(),
            r#"["config.read","audit.read"]"#
        );
        assert_eq!(
            serde_json::from_str::<PermissionSet>(r#"["audit.read","config.read"]"#).unwrap(),
            set
        );
        assert!(PermissionSet::from_names(&["config.read", "root"]).is_err());
        assert!(serde_json::from_str::<PermissionSet>(r#"["root"]"#).is_err());
        assert!(PermissionSet::all().is_superset(&set));
        assert_eq!(
            set.intersection(&PermissionSet::from_names(&["audit.read"]).unwrap())
                .names(),
            ["audit.read"]
        );
    }
}
