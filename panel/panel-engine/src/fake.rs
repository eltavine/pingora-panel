use crate::ports::{
    AbortReceipt, ActivateRequest, ActivationReceipt, EngineCapabilities, EngineCapability,
    GatewayEngine, GatewayStatus, PrepareReceipt, PrepareRequest, PrepareToken, SnapshotEnvelope,
};
use crate::validate_engine_ir;
use async_trait::async_trait;
use panel_errors::{ErrorCode, PanelError, Result, ValidationReport};
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
use std::collections::{BTreeSet, HashMap};
use tokio::sync::Mutex;
use uuid::Uuid;

const FAKE_ADAPTER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default)]
struct State {
    active: Option<SnapshotEnvelope>,
    prepared: HashMap<PrepareToken, SnapshotEnvelope>,
}

pub struct FakeGatewayEngine {
    capabilities: BTreeSet<EngineCapability>,
    state: Mutex<State>,
}

impl FakeGatewayEngine {
    pub fn new(capabilities: impl IntoIterator<Item = EngineCapability>) -> Self {
        Self {
            capabilities: capabilities.into_iter().collect(),
            state: Mutex::new(State::default()),
        }
    }

    pub fn with_default_capabilities() -> Self {
        Self::new([
            EngineCapability::new("route.host", "1"),
            EngineCapability::new("route.path-prefix", "1"),
            EngineCapability::new("upstream.http", "1"),
            EngineCapability::new("upstream.https", "1"),
            EngineCapability::new("activation.cas", "1"),
        ])
    }

    fn validate_snapshot(&self, snapshot: &RuntimeSnapshot) -> Result<ValidationReport> {
        validate_engine_ir(snapshot, &self.capabilities)
    }

    fn capabilities_value(&self) -> EngineCapabilities {
        EngineCapabilities {
            protocol_version: "pingora.panel.gateway.v1".into(),
            build_version: env!("CARGO_PKG_VERSION").into(),
            schema_version: IR_SCHEMA_VERSION.into(),
            adapter_version: FAKE_ADAPTER_VERSION.into(),
            capabilities: self.capabilities.clone(),
        }
    }
}

#[async_trait]
impl GatewayEngine for FakeGatewayEngine {
    async fn capabilities(&self) -> Result<EngineCapabilities> {
        Ok(self.capabilities_value())
    }

    async fn validate(&self, snapshot: RuntimeSnapshot) -> Result<ValidationReport> {
        self.validate_snapshot(&snapshot)
    }

    async fn prepare(&self, request: PrepareRequest) -> Result<PrepareReceipt> {
        let report = self.validate_snapshot(&request.snapshot)?;
        if !report.valid {
            return Err(PanelError::new(
                ErrorCode::VALIDATION_FAILED,
                "snapshot validation failed",
            )
            .with_diagnostics(report.diagnostics));
        }

        let mut state = self.state.lock().await;
        if let Some(active) = &state.active {
            if request.snapshot.revision_id <= active.snapshot.revision_id {
                return Err(PanelError::conflict(
                    "revision is not newer than the active revision",
                ));
            }
        }
        if state.prepared.values().any(|item| {
            item.snapshot.revision_id == request.snapshot.revision_id
                || item.snapshot.content_hash == request.snapshot.content_hash
        }) {
            return Err(PanelError::conflict(
                "revision or content is already prepared",
            ));
        }

        let token = PrepareToken::new(Uuid::new_v4().to_string());
        let previous_active_hash = state
            .active
            .as_ref()
            .map(|item| item.snapshot.content_hash.clone());
        let receipt = PrepareReceipt {
            revision_id: request.snapshot.revision_id,
            content_hash: request.snapshot.content_hash.clone(),
            adapter_version: FAKE_ADAPTER_VERSION.into(),
            schema_version: request.snapshot.schema_version.clone(),
            prepare_token: token.clone(),
            previous_active_hash,
        };
        state.prepared.insert(
            token,
            SnapshotEnvelope {
                snapshot: request.snapshot,
            },
        );
        Ok(receipt)
    }

    async fn activate(&self, request: ActivateRequest) -> Result<ActivationReceipt> {
        let mut state = self.state.lock().await;
        let current_hash = state
            .active
            .as_ref()
            .map(|item| item.snapshot.content_hash.clone());
        if current_hash != request.expected_active_hash {
            return Err(PanelError::conflict(
                "expected active hash does not match current active hash",
            ));
        }
        let prepared = state
            .prepared
            .get(&request.prepare_token)
            .ok_or_else(|| PanelError::new(ErrorCode::NOT_FOUND, "prepare token was not found"))?;
        if let Some(active) = &state.active {
            if prepared.snapshot.revision_id <= active.snapshot.revision_id {
                return Err(PanelError::conflict("prepared revision is stale"));
            }
        }

        let envelope = state
            .prepared
            .remove(&request.prepare_token)
            .expect("token checked above");
        let receipt = ActivationReceipt {
            revision_id: envelope.snapshot.revision_id,
            content_hash: envelope.snapshot.content_hash.clone(),
            adapter_version: FAKE_ADAPTER_VERSION.into(),
            schema_version: envelope.snapshot.schema_version.clone(),
            prepare_token: request.prepare_token,
            previous_active_hash: current_hash,
        };
        state.active = Some(envelope);
        Ok(receipt)
    }

    async fn abort(&self, token: PrepareToken) -> Result<AbortReceipt> {
        let mut state = self.state.lock().await;
        let envelope = state
            .prepared
            .remove(&token)
            .ok_or_else(|| PanelError::new(ErrorCode::NOT_FOUND, "prepare token was not found"))?;
        Ok(AbortReceipt {
            revision_id: envelope.snapshot.revision_id,
            content_hash: envelope.snapshot.content_hash,
            adapter_version: FAKE_ADAPTER_VERSION.into(),
            schema_version: envelope.snapshot.schema_version,
            prepare_token: token,
            previous_active_hash: state
                .active
                .as_ref()
                .map(|item| item.snapshot.content_hash.clone()),
        })
    }

    async fn status(&self) -> Result<GatewayStatus> {
        let state = self.state.lock().await;
        Ok(GatewayStatus {
            ready: true,
            message: None,
            active_revision_id: state.active.as_ref().map(|item| item.snapshot.revision_id),
            active_hash: state
                .active
                .as_ref()
                .map(|item| item.snapshot.content_hash.clone()),
            prepared_count: state.prepared.len(),
            adapter_version: FAKE_ADAPTER_VERSION.into(),
            schema_version: IR_SCHEMA_VERSION.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActivateRequest, GatewayEngine, PrepareRequest};
    use panel_domain::RevisionId;
    use panel_ir::CapabilityRequirement;
    use std::sync::Arc;

    fn snapshot(revision: u64) -> RuntimeSnapshot {
        RuntimeSnapshot::empty(RevisionId::new(revision))
    }

    #[tokio::test]
    async fn empty_snapshot_validates() {
        let engine = FakeGatewayEngine::with_default_capabilities();
        assert!(engine.validate(snapshot(1)).await.unwrap().valid);
    }

    #[tokio::test]
    async fn unsupported_capability_is_stable_error() {
        let engine = FakeGatewayEngine::with_default_capabilities();
        let mut candidate = snapshot(1);
        candidate
            .required_capabilities
            .push(CapabilityRequirement::new("cache", "1"));
        candidate.refresh_content_hash();
        let error = engine.validate(candidate).await.unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);
    }

    #[tokio::test]
    async fn dangling_route_reference_is_validation_error() {
        let engine = FakeGatewayEngine::with_default_capabilities();
        let mut candidate = snapshot(1);
        candidate.routes.push(panel_ir::RouteSpec {
            id: panel_domain::RouteId::new("route-1").unwrap(),
            site_id: panel_domain::SiteId::new("missing-site").unwrap(),
            priority: 1,
            enabled: true,
            matcher: panel_ir::RouteMatcher::PathPrefix {
                path: panel_domain::PathPrefix::new("/").unwrap(),
            },
            action: panel_ir::RouteAction::Proxy {
                upstream_pool_id: panel_domain::UpstreamPoolId::new("missing-pool").unwrap(),
            },
            retry_policy: None,
            header_policy_id: None,
            cache_policy_id: None,
            security_policy_id: None,
            lua_policy_id: None,
        });
        candidate.refresh_content_hash();
        let report = engine.validate(candidate).await.unwrap();
        assert!(!report.valid);
        assert!(report
            .diagnostics
            .iter()
            .any(|item| item.message.contains("unknown site")));
    }

    #[tokio::test]
    async fn prepare_activate_and_abort_are_atomic() {
        let engine = FakeGatewayEngine::with_default_capabilities();
        let first = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(1),
            })
            .await
            .unwrap();
        let activated = engine
            .activate(ActivateRequest {
                prepare_token: first.prepare_token,
                expected_active_hash: None,
            })
            .await
            .unwrap();
        assert_eq!(
            engine.status().await.unwrap().active_hash,
            Some(activated.content_hash.clone())
        );

        let second = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(2),
            })
            .await
            .unwrap();
        let stale = engine
            .activate(ActivateRequest {
                prepare_token: second.prepare_token.clone(),
                expected_active_hash: None,
            })
            .await
            .unwrap_err();
        assert_eq!(stale.code.as_str(), ErrorCode::CONFLICT);
        assert_eq!(
            engine.status().await.unwrap().active_hash,
            Some(activated.content_hash)
        );
        engine.abort(second.prepare_token.clone()).await.unwrap();
        assert_eq!(
            engine
                .abort(second.prepare_token)
                .await
                .unwrap_err()
                .code
                .as_str(),
            ErrorCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn concurrent_cas_has_one_winner() {
        let engine = Arc::new(FakeGatewayEngine::with_default_capabilities());
        let first = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(1),
            })
            .await
            .unwrap();
        let second = engine
            .prepare(PrepareRequest {
                snapshot: snapshot(2),
            })
            .await
            .unwrap();
        let expected = None;
        let left = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .activate(ActivateRequest {
                        prepare_token: first.prepare_token,
                        expected_active_hash: expected,
                    })
                    .await
            })
        };
        let right = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .activate(ActivateRequest {
                        prepare_token: second.prepare_token,
                        expected_active_hash: None,
                    })
                    .await
            })
        };
        let outcomes = [left.await.unwrap(), right.await.unwrap()];
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(outcomes.iter().filter(|result| result.is_err()).count(), 1);
    }
}
