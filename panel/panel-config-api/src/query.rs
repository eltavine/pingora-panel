use chrono::{DateTime, Utc};
use panel_application::operations;
use panel_config_model::SiteQuery;
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, str::FromStr};
use uuid::Uuid;

/// Files of the configuration language by path.
pub type Files = BTreeMap<String, String>;

operations! {
    /// A read of the draft's model: sites with their domains and routes,
    /// upstreams, listeners, TLS profiles and security policies.
    pub enum ModelQuery {
        /// Nothing but the draft's version and application state, which
        /// every answer carries.
        "config.draft" => Draft,
        "sites.list" => Sites { query: SiteQuery },
        "sites.get" => Site { id: Uuid },
        "sites.summary" => SiteSummary,
        /// The sites to export; every live site when none are named.
        "sites.export" => ExportSites { ids: Vec<Uuid> },
        "domains.list" => Domains { site_id: Option<Uuid>, q: Option<String> },
        "domains.check" => CheckDomains { hosts: Vec<String> },
        "routes.list" => Routes { site: Uuid },
        "routes.get" => Route { id: Uuid },
        "upstreams.list" => Upstreams,
        "upstreams.get" => Upstream { id: Uuid },
        "listeners.list" => Listeners,
        "listeners.get" => Listener { id: String },
        "tls_profiles.list" => TlsProfiles,
        "tls_profiles.get" => TlsProfile { id: String },
        "security_policies.list" => SecurityPolicies,
        "security_policies.get" => SecurityPolicy { id: String },
        "http_policies.list" => HttpPolicies,
        "http_policies.get" => HttpPolicy { id: String },
        /// The draft's diagnostics, of the sites named when any are.
        "config.validate" => Validate { site_ids: Vec<Uuid> },
    }
}

operations! {
    /// A read in the configuration language, of the draft's files or of
    /// files sent with it.
    pub enum LanguageQuery {
        "config.source" => Source,
        "config.check" => Check { files: Files },
        "config.format" => Format { files: Files },
        /// The syntax tree of one file, the entry file when none is named.
        "config.ast" => Syntax { files: Files, file: Option<String> },
        /// What is written at a position in the files.
        "config.explain" => Explain { files: Files, file: String, line: usize, column: usize },
        "config.import.nginx" => ImportNginx { files: Files, entry: String },
        /// The draft as the gateway runs it.
        "config.ir" => Ir,
        "config.schema" => Schema,
        /// What applying the draft would change.
        "config.plan" => Plan,
        /// The Lua scripts of the draft, or of a revision: where each runs,
        /// its version and what checking found.
        "config.lua" => Lua { revision: Option<u64> },
    }
}

operations! {
    /// A read of the configurations applied or attempted.
    pub enum RevisionQuery {
        /// Revisions newest first, older than `before` when given.
        "revisions.list" => Revisions { before: Option<u64>, limit: Option<u32> },
        "revisions.get" => Revision { id: u64 },
        /// What changed from `against` to the revision.
        "revisions.diff" => Diff { id: u64, against: DiffBase },
    }
}

operations! {
    /// A read of approval policies and requests.
    pub enum ApprovalQuery {
        "approval_policies.list" => Policies,
        "approval_policies.get" => Policy { id: String },
        /// Requests newest first, asked before `before` when given.
        "approvals.list" => Requests { before: Option<DateTime<Utc>>, limit: Option<u32> },
        "approvals.get" => Request { id: Uuid },
    }
}

/// What a revision is compared with.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffBase {
    /// The revision recorded before it.
    #[default]
    Previous,
    /// The revision the gateway runs.
    Active,
    Draft,
    Revision(u64),
}

impl FromStr for DiffBase {
    type Err = PanelError;

    /// `previous`, `active`, `draft` or a revision number.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "previous" => Self::Previous,
            "active" => Self::Active,
            "draft" => Self::Draft,
            other => Self::Revision(other.parse().map_err(|_| {
                PanelError::invalid_argument(format!(
                    "{other:?} is not previous, active, draft or a revision"
                ))
            })?),
        })
    }
}

/// A read of the configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationQuery {
    Model(ModelQuery),
    Language(LanguageQuery),
    Revision(RevisionQuery),
    Approval(ApprovalQuery),
}

impl ConfigurationQuery {
    pub fn operation(&self) -> &'static str {
        match self {
            Self::Model(query) => query.operation(),
            Self::Language(query) => query.operation(),
            Self::Revision(query) => query.operation(),
            Self::Approval(query) => query.operation(),
        }
    }
}

impl From<ModelQuery> for ConfigurationQuery {
    fn from(query: ModelQuery) -> Self {
        Self::Model(query)
    }
}

impl From<LanguageQuery> for ConfigurationQuery {
    fn from(query: LanguageQuery) -> Self {
        Self::Language(query)
    }
}

impl From<RevisionQuery> for ConfigurationQuery {
    fn from(query: RevisionQuery) -> Self {
        Self::Revision(query)
    }
}

impl From<ApprovalQuery> for ConfigurationQuery {
    fn from(query: ApprovalQuery) -> Self {
        Self::Approval(query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_travel_under_their_operation_names() {
        let query = ConfigurationQuery::from(ModelQuery::Routes { site: Uuid::nil() });
        let encoded = serde_json::to_value(&query).unwrap();
        assert_eq!(encoded["model"]["operation"], "routes.list");
        assert_eq!(query.operation(), "routes.list");
        assert_eq!(
            serde_json::from_value::<ConfigurationQuery>(encoded).unwrap(),
            query
        );
        let plain = serde_json::to_value(ConfigurationQuery::from(LanguageQuery::Plan)).unwrap();
        assert_eq!(
            plain,
            serde_json::json!({"language": {"operation": "config.plan"}})
        );
    }

    #[test]
    fn a_diff_compares_with_what_it_names() {
        assert_eq!("previous".parse::<DiffBase>().unwrap(), DiffBase::Previous);
        assert_eq!("draft".parse::<DiffBase>().unwrap(), DiffBase::Draft);
        assert_eq!("12".parse::<DiffBase>().unwrap(), DiffBase::Revision(12));
        assert!("latest".parse::<DiffBase>().is_err());
    }
}
