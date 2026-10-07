//! Container engines of the host agent and of plugins (ADR 0044). The
//! engines a plugin provides through its container engine port are named
//! `<plugin>.<engine>`; the agent's never hold a dot, so each call goes to
//! whichever provides its engine.

use async_trait::async_trait;
use ops_grpc_client::OpsAgentClient;
use panel_application::{
    CommandContext, ContainerAction, ContainerChange, ContainerDetail, ContainerEngine,
    ContainerFilter, ContainerList, ContainerLogQuery, ContainerLogStart, ContainerLogTail,
    ContainerLogs, ContainerStatsList, ContainersPort, RequestScope,
};
use panel_errors::{PanelError, Result};
use panel_plugin_api::{PluginList, PluginQuery, PluginState, PluginsPort};
use std::sync::Arc;
use tonic::transport::Channel;

/// The port, and the grant, of container engines.
const PORT: &str = "containers";

pub struct ContainerEngines {
    agent: Option<Arc<dyn ContainersPort>>,
    plugins: Arc<dyn PluginsPort>,
    channel: Channel,
}

impl ContainerEngines {
    /// The agent's engines when there is an agent, and those of the
    /// plugins the plugins module at `channel` runs.
    pub fn new(
        agent: Option<Arc<dyn ContainersPort>>,
        plugins: Arc<dyn PluginsPort>,
        channel: Channel,
    ) -> Self {
        Self {
            agent,
            plugins,
            channel,
        }
    }

    /// The port of `engine` and what it calls the engine.
    fn route(&self, engine: String) -> Result<(Arc<dyn ContainersPort>, String)> {
        match engine.split_once('.') {
            Some((plugin, inner)) => Ok((
                Arc::new(OpsAgentClient::for_plugin(self.channel.clone(), plugin)?),
                inner.to_owned(),
            )),
            None => match &self.agent {
                Some(agent) => Ok((Arc::clone(agent), engine)),
                None => Err(PanelError::not_found(format!(
                    "there is no container engine {engine}: no host agent manages engines here"
                ))),
            },
        }
    }

    /// The running plugins granted to provide container engines.
    async fn providers(&self, scope: &RequestScope) -> Vec<String> {
        let Ok(output) = self.plugins.read(scope.clone(), PluginQuery::Plugins).await else {
            return Vec::new();
        };
        let Ok(list) = serde_json::from_slice::<PluginList>(&output.content) else {
            return Vec::new();
        };
        list.plugins
            .into_iter()
            .filter(|plugin| {
                plugin.state == PluginState::Enabled
                    && plugin.grants.iter().any(|grant| grant == PORT)
                    && plugin
                        .versions
                        .iter()
                        .find(|version| Some(&version.version) == plugin.active_version.as_ref())
                        .is_some_and(|version| version.ports.iter().any(|port| port == PORT))
            })
            .map(|plugin| plugin.name)
            .collect()
    }
}

#[async_trait]
impl ContainersPort for ContainerEngines {
    async fn engines(&self, scope: RequestScope) -> Result<Vec<ContainerEngine>> {
        let mut engines = match &self.agent {
            Some(agent) => agent.engines(scope.clone()).await?,
            None => Vec::new(),
        };
        for plugin in self.providers(&scope).await {
            let client = OpsAgentClient::for_plugin(self.channel.clone(), &plugin)?;
            match client.engines(scope.clone()).await {
                Ok(provided) => {
                    engines.extend(provided.into_iter().map(|engine| ContainerEngine {
                        id: format!("{plugin}.{}", engine.id),
                        ..engine
                    }))
                }
                Err(error) => tracing::warn!(
                    %plugin,
                    error = %error.message,
                    "a plugin's container engines do not answer"
                ),
            }
        }
        Ok(engines)
    }

    async fn set_engine(
        &self,
        context: CommandContext,
        engine: String,
        enabled: bool,
    ) -> Result<ContainerEngine> {
        let (port, inner) = self.route(engine.clone())?;
        let mut changed = port.set_engine(context, inner, enabled).await?;
        changed.id = engine;
        Ok(changed)
    }

    async fn containers(
        &self,
        scope: RequestScope,
        engine: String,
        filter: ContainerFilter,
    ) -> Result<ContainerList> {
        let (port, inner) = self.route(engine)?;
        port.containers(scope, inner, filter).await
    }

    async fn inspect(
        &self,
        scope: RequestScope,
        engine: String,
        container: String,
    ) -> Result<ContainerDetail> {
        let (port, inner) = self.route(engine)?;
        port.inspect(scope, inner, container).await
    }

    async fn act(
        &self,
        context: CommandContext,
        engine: String,
        container: String,
        action: ContainerAction,
    ) -> Result<ContainerChange> {
        let (port, inner) = self.route(engine)?;
        port.act(context, inner, container, action).await
    }

    async fn logs(
        &self,
        scope: RequestScope,
        engine: String,
        container: String,
        query: ContainerLogQuery,
    ) -> Result<ContainerLogs> {
        let (port, inner) = self.route(engine)?;
        port.logs(scope, inner, container, query).await
    }

    async fn follow_logs(
        &self,
        scope: RequestScope,
        engine: String,
        container: String,
        start: ContainerLogStart,
    ) -> Result<ContainerLogTail> {
        let (port, inner) = self.route(engine)?;
        port.follow_logs(scope, inner, container, start).await
    }

    async fn stats(
        &self,
        scope: RequestScope,
        engine: String,
        container: Option<String>,
    ) -> Result<ContainerStatsList> {
        let (port, inner) = self.route(engine)?;
        port.stats(scope, inner, container).await
    }
}
