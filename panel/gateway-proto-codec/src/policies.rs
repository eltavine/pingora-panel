//! Optional runtime policy wire conversion.

use panel_contracts::gateway::v1 as wire;
use panel_ir::{
    CachePolicy, HeaderPolicy, LuaPolicy, SecurityPolicy, StaticContentPolicy, TlsProfile,
};
use std::collections::BTreeMap;

pub(super) fn decode_tls(value: wire::TlsProfile) -> TlsProfile {
    TlsProfile {
        id: value.id,
        certificate_secret_id: value.certificate_ref,
        private_key_secret_id: value.private_key_secret_id,
        min_protocol: value.min_protocol,
        alpn: value.alpn.into_iter().collect(),
    }
}

pub(super) fn encode_tls(value: &TlsProfile) -> wire::TlsProfile {
    wire::TlsProfile {
        id: value.id.clone(),
        certificate_ref: value.certificate_secret_id.clone(),
        private_key_secret_id: value.private_key_secret_id.clone(),
        min_protocol: value.min_protocol.clone(),
        alpn: value.alpn.iter().cloned().collect(),
    }
}

pub(super) fn decode_header_policy(value: wire::HeaderPolicy) -> HeaderPolicy {
    let request_set = if value.request_set.is_empty() {
        value.set.into_iter().collect()
    } else {
        value.request_set.into_iter().collect()
    };
    let request_remove = if value.request_remove.is_empty() {
        value.remove.into_iter().collect()
    } else {
        value.request_remove.into_iter().collect()
    };
    HeaderPolicy {
        id: value.id,
        request_set,
        request_remove,
        response_set: value.response_set.into_iter().collect(),
        response_remove: value.response_remove.into_iter().collect(),
    }
}

pub(super) fn encode_header_policy(value: &HeaderPolicy) -> wire::HeaderPolicy {
    wire::HeaderPolicy {
        id: value.id.clone(),
        set: BTreeMap::new().into_iter().collect(),
        remove: Vec::new(),
        request_set: value.request_set.clone().into_iter().collect(),
        request_remove: value.request_remove.iter().cloned().collect(),
        response_set: value.response_set.clone().into_iter().collect(),
        response_remove: value.response_remove.iter().cloned().collect(),
    }
}

pub(super) fn decode_static_content(value: wire::StaticContentPolicy) -> StaticContentPolicy {
    StaticContentPolicy {
        id: value.id,
        root: value.root,
        index_files: value.index_files,
        spa_fallback: value.spa_fallback,
    }
}

pub(super) fn encode_static_content(value: &StaticContentPolicy) -> wire::StaticContentPolicy {
    wire::StaticContentPolicy {
        id: value.id.clone(),
        root: value.root.clone(),
        spa_fallback: value.spa_fallback,
        index_files: value.index_files.clone(),
    }
}

pub(super) fn decode_cache_policy(value: wire::CachePolicy) -> CachePolicy {
    CachePolicy {
        id: value.id,
        enabled: value.enabled,
        ttl_seconds: value.ttl_seconds,
        vary_headers: value.vary_headers.into_iter().collect(),
    }
}

pub(super) fn encode_cache_policy(value: &CachePolicy) -> wire::CachePolicy {
    wire::CachePolicy {
        id: value.id.clone(),
        enabled: value.enabled,
        ttl_seconds: value.ttl_seconds,
        vary_headers: value.vary_headers.iter().cloned().collect(),
    }
}

pub(super) fn decode_security_policy(value: wire::SecurityPolicy) -> SecurityPolicy {
    SecurityPolicy {
        id: value.id,
        allowed_cidrs: value.allowed_cidrs.into_iter().collect(),
        denied_cidrs: value.denied_cidrs.into_iter().collect(),
        request_rate_per_second: value.has_request_rate.then_some(value.request_rate),
    }
}

pub(super) fn encode_security_policy(value: &SecurityPolicy) -> wire::SecurityPolicy {
    wire::SecurityPolicy {
        id: value.id.clone(),
        allowed_cidrs: value.allowed_cidrs.iter().cloned().collect(),
        request_rate: value.request_rate_per_second.unwrap_or_default(),
        denied_cidrs: value.denied_cidrs.iter().cloned().collect(),
        has_request_rate: value.request_rate_per_second.is_some(),
    }
}

pub(super) fn decode_lua_policy(value: wire::LuaPolicy) -> LuaPolicy {
    LuaPolicy {
        id: value.id,
        script_secret_id: value.script_ref,
        instruction_limit: value.instruction_limit,
        timeout_ms: value.timeout_ms,
        memory_limit_bytes: value.memory_limit_bytes,
        capabilities: value.capabilities.into_iter().collect(),
    }
}

pub(super) fn encode_lua_policy(value: &LuaPolicy) -> wire::LuaPolicy {
    wire::LuaPolicy {
        id: value.id.clone(),
        script_ref: value.script_secret_id.clone(),
        capabilities: value.capabilities.iter().cloned().collect(),
        instruction_limit: value.instruction_limit,
        timeout_ms: value.timeout_ms,
        memory_limit_bytes: value.memory_limit_bytes,
    }
}
