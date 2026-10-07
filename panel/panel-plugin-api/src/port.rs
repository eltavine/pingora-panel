use crate::{PluginCommand, PluginQuery};
use async_trait::async_trait;
use panel_application::{CommandContext, RequestScope};
use panel_errors::Result;
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroizing;

/// A change, and the entity tag its target must still have (RFC 9110
/// §13.1.1).
#[derive(Clone, Debug, PartialEq)]
pub struct PluginChange {
    pub command: PluginCommand,
    pub if_match: Option<String>,
}

impl PluginChange {
    pub fn new(command: PluginCommand) -> Self {
        Self {
            command,
            if_match: None,
        }
    }

    pub fn if_match(mut self, tag: impl Into<String>) -> Self {
        self.if_match = Some(tag.into());
        self
    }
}

/// A JSON result and, for a single plugin, its entity tag.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginOutput {
    pub content: Vec<u8>,
    pub etag: Option<String>,
}

/// The plugins module: what it found, what it runs and how.
#[async_trait]
pub trait PluginsPort: Send + Sync {
    async fn read(&self, scope: RequestScope, query: PluginQuery) -> Result<PluginOutput>;

    async fn change(&self, context: CommandContext, change: PluginChange) -> Result<PluginOutput>;
}

/// A secret's value: wiped from memory when dropped and never printed.
#[derive(Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}
