//! Snapshot assembly and canonical content verification.

use crate::{decode_hash, encode_hash, policies, routing, upstream};
use panel_contracts::{common::v1 as common, gateway::v1 as wire};
use panel_domain::RevisionId;
use panel_errors::{PanelError, Result};
use panel_ir::{CapabilityRequirement, RuntimeSnapshot};

pub fn decode_snapshot(value: wire::RuntimeSnapshot) -> Result<RuntimeSnapshot> {
    let declared_hash = decode_hash(value.content_hash)?;
    let snapshot = RuntimeSnapshot {
        schema_version: value.schema_version,
        revision_id: RevisionId::new(value.revision_id),
        content_hash: declared_hash,
        listeners: value
            .listeners
            .into_iter()
            .map(routing::decode_listener)
            .collect::<Result<Vec<_>>>()?,
        sites: value
            .sites
            .into_iter()
            .map(routing::decode_site)
            .collect::<Result<Vec<_>>>()?,
        routes: value
            .routes
            .into_iter()
            .map(routing::decode_route)
            .collect::<Result<Vec<_>>>()?,
        upstream_pools: value
            .upstream_pools
            .into_iter()
            .map(upstream::decode_upstream_pool)
            .collect::<Result<Vec<_>>>()?,
        tls_profiles: value
            .tls_profiles
            .into_iter()
            .map(policies::decode_tls)
            .collect(),
        header_policies: value
            .header_policies
            .into_iter()
            .map(policies::decode_header_policy)
            .collect(),
        static_content: value
            .static_content
            .into_iter()
            .map(policies::decode_static_content)
            .collect(),
        cache_policies: value
            .cache_policies
            .into_iter()
            .map(policies::decode_cache_policy)
            .collect(),
        security_policies: value
            .security_policies
            .into_iter()
            .map(policies::decode_security_policy)
            .collect(),
        lua_policies: value
            .lua_policies
            .into_iter()
            .map(policies::decode_lua_policy)
            .collect(),
        required_capabilities: value
            .required_capabilities
            .into_iter()
            .map(|capability| CapabilityRequirement::new(capability.name, capability.version))
            .collect(),
    };
    if !snapshot.has_valid_content_hash() {
        return Err(PanelError::validation_failed(
            "wire snapshot content hash does not match canonical IR",
        ));
    }
    Ok(snapshot)
}

pub fn encode_snapshot(value: &RuntimeSnapshot) -> wire::RuntimeSnapshot {
    wire::RuntimeSnapshot {
        schema_version: value.schema_version.clone(),
        revision_id: value.revision_id.get(),
        content_hash: Some(encode_hash(&value.content_hash)),
        listeners: value
            .listeners
            .iter()
            .map(routing::encode_listener)
            .collect(),
        sites: value.sites.iter().map(routing::encode_site).collect(),
        routes: value.routes.iter().map(routing::encode_route).collect(),
        upstream_pools: value
            .upstream_pools
            .iter()
            .map(upstream::encode_upstream_pool)
            .collect(),
        tls_profiles: value
            .tls_profiles
            .iter()
            .map(policies::encode_tls)
            .collect(),
        header_policies: value
            .header_policies
            .iter()
            .map(policies::encode_header_policy)
            .collect(),
        static_content: value
            .static_content
            .iter()
            .map(policies::encode_static_content)
            .collect(),
        cache_policies: value
            .cache_policies
            .iter()
            .map(policies::encode_cache_policy)
            .collect(),
        security_policies: value
            .security_policies
            .iter()
            .map(policies::encode_security_policy)
            .collect(),
        lua_policies: value
            .lua_policies
            .iter()
            .map(policies::encode_lua_policy)
            .collect(),
        required_capabilities: value
            .required_capabilities
            .iter()
            .map(|capability| common::Capability {
                name: capability.name.clone(),
                version: capability.version.clone(),
            })
            .collect(),
    }
}
