use crate::{NewTrustedKey, PluginLimits, Secret};
use panel_application::operations;

operations! {
    /// A read of plugins, their versions, trusted publisher keys and the
    /// secrets kept for plugins' settings.
    pub enum PluginQuery {
        "plugins.list" => Plugins,
        "plugins.get" => Plugin { name: String },
        "plugins.keys.list" => Keys,
        /// The secrets' names; their values are never read back.
        "plugins.secrets.list" => Secrets,
    }
}

operations! {
    /// A change of plugins, trusted publisher keys or kept secrets.
    pub enum PluginCommand {
        /// Reads the plugins directory again.
        "plugins.discover" => Discover,
        /// Grants these capabilities, among those the plugin asks for, in
        /// place of those granted before.
        "plugins.grant" => Grant { name: String, capabilities: Vec<String> },
        /// Replaces the settings, which the plugin's schema must accept.
        "plugins.configure" => Configure { name: String, settings: serde_json::Value },
        "plugins.limit" => Limit { name: String, limits: PluginLimits },
        /// Starts the version named, or the newest validated one.
        "plugins.enable" => Enable { name: String, version: Option<String> },
        "plugins.disable" => Disable { name: String },
        /// Runs another validated version in place of the active one.
        "plugins.upgrade" => Upgrade { name: String, version: String },
        /// Runs the version that ran before the active one.
        "plugins.rollback" => Rollback { name: String },
        "plugins.keys.put" => PutKey { key: NewTrustedKey },
        "plugins.keys.delete" => DeleteKey { id: String },
        /// Seals a secret that settings name as `vault:<name>`.
        "plugins.secrets.put" => PutSecret { name: String, value: Secret },
        "plugins.secrets.delete" => DeleteSecret { name: String },
    }
}

const PLUGINS: &str = "plugins";
const KEYS: &str = "plugin-keys";
const SECRETS: &str = "plugin-secrets";

impl PluginQuery {
    /// What the read is of, for audit records.
    pub fn resource(&self) -> (&'static str, String) {
        match self {
            Self::Plugins => (PLUGINS, String::new()),
            Self::Plugin { name } => (PLUGINS, name.clone()),
            Self::Keys => (KEYS, String::new()),
            Self::Secrets => (SECRETS, String::new()),
        }
    }
}

impl PluginCommand {
    /// What the change is of: its aggregate type and ID.
    pub fn resource(&self) -> (&'static str, String) {
        match self {
            Self::Discover => (PLUGINS, String::new()),
            Self::Grant { name, .. }
            | Self::Configure { name, .. }
            | Self::Limit { name, .. }
            | Self::Enable { name, .. }
            | Self::Disable { name }
            | Self::Upgrade { name, .. }
            | Self::Rollback { name } => (PLUGINS, name.clone()),
            Self::PutKey { key } => (KEYS, key.id.clone()),
            Self::DeleteKey { id } => (KEYS, id.clone()),
            Self::PutSecret { name, .. } | Self::DeleteSecret { name } => (SECRETS, name.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_travel_as_their_names_with_parameters() {
        let command = PluginCommand::Enable {
            name: "dns".into(),
            version: None,
        };
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["operation"], "plugins.enable");
        assert_eq!(json["parameters"]["name"], "dns");
        assert_eq!(command.resource(), (PLUGINS, "dns".to_owned()));
        let secret: PluginCommand = serde_json::from_value(serde_json::json!({
            "operation": "plugins.secrets.put",
            "parameters": {"name": "dns-token", "value": "s3cret"}
        }))
        .unwrap();
        assert!(!format!("{secret:?}").contains("s3cret"));
        assert_eq!(PluginQuery::Keys.operation(), "plugins.keys.list");
    }
}
