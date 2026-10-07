use crate::{Files, LuaTest};
use panel_application::operations;
use panel_config_model::{
    ApprovalPolicyInput, BatchRequest, CachePolicy, CacheSettings, Domain, HttpPolicy, Listener,
    NodeInput, RouteInput, SecurityPolicy, SiteBundle, SiteInput, TlsProfileInput, UpstreamInput,
};
use panel_domain::NormalizedHost;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

operations! {
    /// A change to the draft's model.
    pub enum ModelChange {
        "sites.create" => CreateSite { site: SiteInput },
        "sites.replace" => ReplaceSite { id: Uuid, site: SiteInput },
        "sites.enable" => EnableSite { id: Uuid },
        "sites.disable" => DisableSite { id: Uuid },
        "sites.favorite" => FavoriteSite { id: Uuid },
        "sites.unfavorite" => UnfavoriteSite { id: Uuid },
        /// Moves a site to the recycle bin.
        "sites.delete" => DeleteSite { id: Uuid },
        "sites.restore" => RestoreSite { id: Uuid },
        /// Deletes a site for good.
        "sites.purge" => PurgeSite { id: Uuid },
        "sites.clone" => CloneSite { id: Uuid, name: String },
        "sites.import" => ImportSites { bundle: SiteBundle },
        "sites.batch" => BatchSites { batch: BatchRequest },
        "domains.add" => AddDomains { site: Uuid, domains: Vec<Domain> },
        "domains.replace" => ReplaceDomain { site: Uuid, host: NormalizedHost, domain: Domain },
        "domains.remove" => RemoveDomain { site: Uuid, host: NormalizedHost },
        "routes.create" => CreateRoute { site: Uuid, route: RouteInput },
        /// Orders a site's routes as listed.
        "routes.reorder" => ReorderRoutes { site: Uuid, order: Vec<Uuid> },
        "routes.replace" => ReplaceRoute { id: Uuid, route: RouteInput },
        "routes.delete" => DeleteRoute { id: Uuid },
        "upstreams.create" => CreateUpstream { upstream: UpstreamInput },
        "upstreams.replace" => ReplaceUpstream { id: Uuid, upstream: UpstreamInput },
        "upstreams.delete" => DeleteUpstream { id: Uuid },
        "nodes.add" => AddNode { upstream: Uuid, node: NodeInput },
        "nodes.replace" => ReplaceNode { upstream: Uuid, id: Uuid, node: NodeInput },
        "nodes.delete" => DeleteNode { upstream: Uuid, id: Uuid },
        /// Creates or replaces the listener of the listener's ID.
        "listeners.put" => PutListener { listener: Listener },
        "listeners.delete" => DeleteListener { id: String },
        /// Creates or replaces the TLS profile of the profile's ID.
        "tls_profiles.put" => PutTlsProfile { profile: TlsProfileInput },
        "tls_profiles.delete" => DeleteTlsProfile { id: String },
        /// Creates or replaces the security policy of the policy's ID.
        "security_policies.put" => PutSecurityPolicy { policy: SecurityPolicy },
        "security_policies.delete" => DeleteSecurityPolicy { id: String },
        /// Creates or replaces the HTTP policy of the policy's ID.
        "http_policies.put" => PutHttpPolicy { policy: HttpPolicy },
        "http_policies.delete" => DeleteHttpPolicy { id: String },
        /// Creates or replaces the cache policy of the policy's ID.
        "cache_policies.put" => PutCachePolicy { policy: CachePolicy },
        "cache_policies.delete" => DeleteCachePolicy { id: String },
        /// Sets how much the gateway's cache keeps.
        "cache_settings.put" => PutCacheSettings { settings: CacheSettings },
    }
}

operations! {
    /// A change to the draft's files.
    pub enum LanguageChange {
        "config.source.replace" => ReplaceSource { files: Files },
    }
}

operations! {
    /// Lua scripts run without changing anything (ADR 0039).
    pub enum LuaCommand {
        /// Runs the draft's handlers a request reaches, or one script, on a
        /// request described in full.
        "config.lua.test" => Test { test: LuaTest },
    }
}

operations! {
    /// A change of the draft from a revision, or of a revision's record.
    pub enum RevisionChange {
        /// Makes the revision's files the draft.
        "revisions.restore" => Restore { id: u64 },
        "revisions.note" => Note { id: u64, note: Option<String> },
    }
}

operations! {
    /// A change to approval policies, or a decision on a request.
    pub enum ApprovalChange {
        /// Creates or replaces a policy.
        "approval_policies.put" => PutPolicy { id: String, policy: ApprovalPolicyInput },
        "approval_policies.delete" => DeletePolicy { id: String },
        "approvals.approve" => Approve { id: Uuid },
        "approvals.reject" => Reject { id: Uuid, reason: Option<String> },
        "approvals.withdraw" => Withdraw { id: Uuid },
        /// Takes back the caller's approval.
        "approvals.revoke" => Revoke { id: Uuid },
    }
}

impl ModelChange {
    /// The path of what it changes.
    pub fn resource(&self) -> String {
        match self {
            Self::CreateSite { .. } | Self::ImportSites { .. } | Self::BatchSites { .. } => {
                "sites".into()
            }
            Self::ReplaceSite { id, .. }
            | Self::EnableSite { id }
            | Self::DisableSite { id }
            | Self::FavoriteSite { id }
            | Self::UnfavoriteSite { id }
            | Self::DeleteSite { id }
            | Self::RestoreSite { id }
            | Self::PurgeSite { id }
            | Self::CloneSite { id, .. } => format!("sites/{id}"),
            Self::AddDomains { site, .. } => format!("sites/{site}/domains"),
            Self::ReplaceDomain { site, host, .. } | Self::RemoveDomain { site, host } => {
                format!("sites/{site}/domains/{}", host.as_str())
            }
            Self::CreateRoute { site, .. } | Self::ReorderRoutes { site, .. } => {
                format!("sites/{site}/routes")
            }
            Self::ReplaceRoute { id, .. } | Self::DeleteRoute { id } => format!("routes/{id}"),
            Self::CreateUpstream { .. } => "upstreams".into(),
            Self::ReplaceUpstream { id, .. } | Self::DeleteUpstream { id } => {
                format!("upstreams/{id}")
            }
            Self::AddNode { upstream, .. } => format!("upstreams/{upstream}/nodes"),
            Self::ReplaceNode { upstream, id, .. } | Self::DeleteNode { upstream, id } => {
                format!("upstreams/{upstream}/nodes/{id}")
            }
            Self::PutListener { listener } => format!("listeners/{}", listener.id),
            Self::DeleteListener { id } => format!("listeners/{id}"),
            Self::PutTlsProfile { profile } => format!("tls-profiles/{}", profile.id),
            Self::DeleteTlsProfile { id } => format!("tls-profiles/{id}"),
            Self::PutSecurityPolicy { policy } => format!("security-policies/{}", policy.id),
            Self::DeleteSecurityPolicy { id } => format!("security-policies/{id}"),
            Self::PutHttpPolicy { policy } => format!("http-policies/{}", policy.id),
            Self::DeleteHttpPolicy { id } => format!("http-policies/{id}"),
            Self::PutCachePolicy { policy } => format!("cache-policies/{}", policy.id),
            Self::DeleteCachePolicy { id } => format!("cache-policies/{id}"),
            Self::PutCacheSettings { .. } => "cache".into(),
        }
    }
}

/// A change of the configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationCommand {
    Model(Box<ModelChange>),
    Language(LanguageChange),
    Revision(RevisionChange),
    Approval(ApprovalChange),
    Lua(LuaCommand),
}

impl ConfigurationCommand {
    pub fn operation(&self) -> &'static str {
        match self {
            Self::Model(change) => change.operation(),
            Self::Language(change) => change.operation(),
            Self::Revision(change) => change.operation(),
            Self::Approval(change) => change.operation(),
            Self::Lua(command) => command.operation(),
        }
    }

    /// The path of what it changes, which audit records name.
    pub fn resource(&self) -> String {
        match self {
            Self::Model(change) => change.resource(),
            Self::Language(LanguageChange::ReplaceSource { .. }) => "config/source".into(),
            Self::Revision(RevisionChange::Restore { id } | RevisionChange::Note { id, .. }) => {
                format!("revisions/{id}")
            }
            Self::Approval(
                ApprovalChange::PutPolicy { id, .. } | ApprovalChange::DeletePolicy { id },
            ) => format!("approval-policies/{id}"),
            Self::Approval(
                ApprovalChange::Approve { id }
                | ApprovalChange::Reject { id, .. }
                | ApprovalChange::Withdraw { id }
                | ApprovalChange::Revoke { id },
            ) => format!("approvals/{id}"),
            Self::Lua(LuaCommand::Test { .. }) => "config/lua".into(),
        }
    }
}

impl From<ModelChange> for ConfigurationCommand {
    fn from(change: ModelChange) -> Self {
        Self::Model(Box::new(change))
    }
}

impl From<LanguageChange> for ConfigurationCommand {
    fn from(change: LanguageChange) -> Self {
        Self::Language(change)
    }
}

impl From<RevisionChange> for ConfigurationCommand {
    fn from(change: RevisionChange) -> Self {
        Self::Revision(change)
    }
}

impl From<ApprovalChange> for ConfigurationCommand {
    fn from(change: ApprovalChange) -> Self {
        Self::Approval(change)
    }
}

impl From<LuaCommand> for ConfigurationCommand {
    fn from(command: LuaCommand) -> Self {
        Self::Lua(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_name_their_operation_and_resource() {
        let site = Uuid::nil();
        let host = NormalizedHost::new("Shop.Example.com").unwrap();
        let command = ConfigurationCommand::from(ModelChange::RemoveDomain { site, host });
        assert_eq!(command.operation(), "domains.remove");
        assert_eq!(
            command.resource(),
            format!("sites/{site}/domains/shop.example.com")
        );
        let encoded = serde_json::to_value(&command).unwrap();
        assert_eq!(encoded["model"]["operation"], "domains.remove");
        assert_eq!(
            serde_json::from_value::<ConfigurationCommand>(encoded).unwrap(),
            command
        );

        let note = ConfigurationCommand::from(RevisionChange::Note { id: 4, note: None });
        assert_eq!(
            (note.operation(), note.resource().as_str()),
            ("revisions.note", "revisions/4")
        );
    }

    #[test]
    fn unknown_operations_and_parameters_are_refused() {
        let unknown =
            serde_json::json!({"model": {"operation": "sites.explode", "parameters": {}}});
        assert!(serde_json::from_value::<ConfigurationCommand>(unknown).is_err());
        let extra = serde_json::json!({
            "approval": {"operation": "approvals.withdraw", "parameters": {"id": Uuid::nil(), "why": 1}}
        });
        assert!(serde_json::from_value::<ConfigurationCommand>(extra).is_err());
    }
}
