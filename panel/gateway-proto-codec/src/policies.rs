//! Optional runtime policy wire conversion.

use crate::optional_string;
use panel_contracts::gateway::v1 as wire;
use panel_errors::{PanelError, Result};
use panel_ir::{
    BasicAuth, CachePolicy, CompressionAlgorithm, CompressionPolicy, CorsPolicy, DirectoryListing,
    HeaderField, HeaderPolicy, LimitedResponse, LuaPolicy, RateLimit, RateLimitKey, RefererRule,
    SecurityPolicy, ServerHeader, StaticCacheRule, StaticContentPolicy, TlsProfile,
};
use std::collections::BTreeMap;

pub(super) fn decode_tls(value: wire::TlsProfile) -> TlsProfile {
    TlsProfile {
        id: value.id,
        certificate_secret_id: value.certificate_ref,
        private_key_secret_id: value.private_key_secret_id,
        min_protocol: value.min_protocol,
        max_protocol: (!value.max_protocol.is_empty()).then_some(value.max_protocol),
        cipher_suites: value.cipher_suites,
        session_resumption: !value.session_resumption_disabled,
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
        max_protocol: value.max_protocol.clone().unwrap_or_default(),
        cipher_suites: value.cipher_suites.clone(),
        session_resumption_disabled: !value.session_resumption,
    }
}

pub(super) fn decode_header_policy(value: wire::HeaderPolicy) -> Result<HeaderPolicy> {
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
    let fields = |fields: Vec<wire::HeaderField>| {
        fields
            .into_iter()
            .map(|field| HeaderField {
                name: field.name,
                value: field.value,
            })
            .collect()
    };
    let server = match value.server {
        None => ServerHeader::Keep,
        Some(server) => match wire::ServerHeaderMode::try_from(server.mode) {
            Ok(wire::ServerHeaderMode::Unspecified | wire::ServerHeaderMode::Keep) => {
                ServerHeader::Keep
            }
            Ok(wire::ServerHeaderMode::Remove) => ServerHeader::Remove,
            Ok(wire::ServerHeaderMode::Replace) => ServerHeader::Replace {
                value: server.value,
            },
            Err(_) => {
                return Err(PanelError::invalid_argument(format!(
                    "HTTP policy {} uses a Server mode this gateway does not know",
                    value.id
                )))
            }
        },
    };
    let compression = value
        .compression
        .map(|compression| {
            Ok::<_, PanelError>(CompressionPolicy {
                algorithms: compression
                    .algorithms
                    .into_iter()
                    .map(
                        |algorithm| match wire::CompressionAlgorithm::try_from(algorithm) {
                            Ok(wire::CompressionAlgorithm::Gzip) => Ok(CompressionAlgorithm::Gzip),
                            Ok(wire::CompressionAlgorithm::Brotli) => {
                                Ok(CompressionAlgorithm::Brotli)
                            }
                            Ok(wire::CompressionAlgorithm::Zstd) => Ok(CompressionAlgorithm::Zstd),
                            _ => Err(PanelError::invalid_argument(format!(
                                "HTTP policy {} uses a coding this gateway does not know",
                                value.id
                            ))),
                        },
                    )
                    .collect::<Result<_>>()?,
                types: compression.types,
                min_bytes: compression.min_bytes,
            })
        })
        .transpose()?;
    Ok(HeaderPolicy {
        request_set,
        request_remove,
        response_set: value.response_set.into_iter().collect(),
        response_remove: value.response_remove.into_iter().collect(),
        request_add: fields(value.request_add),
        response_add: fields(value.response_add),
        server,
        cors: value.cors.map(|cors| CorsPolicy {
            allowed_origins: cors.allowed_origins,
            allowed_methods: cors.allowed_methods,
            allowed_headers: cors.allowed_headers,
            exposed_headers: cors.exposed_headers,
            allow_credentials: cors.allow_credentials,
            max_age_seconds: cors.max_age_seconds,
        }),
        compression,
        id: value.id,
    })
}

pub(super) fn encode_header_policy(value: &HeaderPolicy) -> wire::HeaderPolicy {
    let fields = |fields: &[HeaderField]| {
        fields
            .iter()
            .map(|field| wire::HeaderField {
                name: field.name.clone(),
                value: field.value.clone(),
            })
            .collect()
    };
    wire::HeaderPolicy {
        id: value.id.clone(),
        set: BTreeMap::new().into_iter().collect(),
        remove: Vec::new(),
        request_set: value.request_set.clone().into_iter().collect(),
        request_remove: value.request_remove.iter().cloned().collect(),
        response_set: value.response_set.clone().into_iter().collect(),
        response_remove: value.response_remove.iter().cloned().collect(),
        request_add: fields(&value.request_add),
        response_add: fields(&value.response_add),
        server: match &value.server {
            ServerHeader::Keep => None,
            ServerHeader::Remove => Some(wire::ServerHeader {
                mode: wire::ServerHeaderMode::Remove as i32,
                value: String::new(),
            }),
            ServerHeader::Replace { value } => Some(wire::ServerHeader {
                mode: wire::ServerHeaderMode::Replace as i32,
                value: value.clone(),
            }),
        },
        cors: value.cors.as_ref().map(|cors| wire::CorsPolicy {
            allowed_origins: cors.allowed_origins.clone(),
            allowed_methods: cors.allowed_methods.clone(),
            allowed_headers: cors.allowed_headers.clone(),
            exposed_headers: cors.exposed_headers.clone(),
            allow_credentials: cors.allow_credentials,
            max_age_seconds: cors.max_age_seconds,
        }),
        compression: value
            .compression
            .as_ref()
            .map(|compression| wire::CompressionPolicy {
                algorithms: compression
                    .algorithms
                    .iter()
                    .map(|algorithm| {
                        (match algorithm {
                            CompressionAlgorithm::Gzip => wire::CompressionAlgorithm::Gzip,
                            CompressionAlgorithm::Brotli => wire::CompressionAlgorithm::Brotli,
                            CompressionAlgorithm::Zstd => wire::CompressionAlgorithm::Zstd,
                        }) as i32
                    })
                    .collect(),
                types: compression.types.clone(),
                min_bytes: compression.min_bytes,
            }),
    }
}

pub(super) fn decode_static_content(
    value: wire::StaticContentPolicy,
) -> Result<StaticContentPolicy> {
    let listing = match wire::DirectoryListing::try_from(value.listing) {
        Ok(wire::DirectoryListing::Unspecified) => DirectoryListing::Off,
        Ok(wire::DirectoryListing::Html) => DirectoryListing::Html,
        Ok(wire::DirectoryListing::Json) => DirectoryListing::Json,
        Err(_) => {
            return Err(PanelError::invalid_argument(format!(
                "static content {} lists directories in a way this gateway does not know",
                value.id
            )))
        }
    };
    Ok(StaticContentPolicy {
        id: value.id,
        root: value.root,
        index_files: value.index_files,
        spa_fallback: value.spa_fallback,
        listing,
        media_types: value.media_types.into_iter().collect(),
        default_type: value.default_type,
        cache: value
            .cache
            .into_iter()
            .map(|rule| StaticCacheRule {
                extensions: rule.extensions.into_iter().collect(),
                max_age_seconds: rule.max_age_seconds,
                immutable: rule.immutable,
            })
            .collect(),
    })
}

pub(super) fn encode_static_content(value: &StaticContentPolicy) -> wire::StaticContentPolicy {
    wire::StaticContentPolicy {
        id: value.id.clone(),
        root: value.root.clone(),
        spa_fallback: value.spa_fallback,
        index_files: value.index_files.clone(),
        listing: match value.listing {
            DirectoryListing::Off => wire::DirectoryListing::Unspecified,
            DirectoryListing::Html => wire::DirectoryListing::Html,
            DirectoryListing::Json => wire::DirectoryListing::Json,
        } as i32,
        media_types: value
            .media_types
            .iter()
            .map(|(extension, media_type)| (extension.clone(), media_type.clone()))
            .collect(),
        default_type: value.default_type.clone(),
        cache: value
            .cache
            .iter()
            .map(|rule| wire::StaticCacheRule {
                extensions: rule.extensions.iter().cloned().collect(),
                max_age_seconds: rule.max_age_seconds,
                immutable: rule.immutable,
            })
            .collect(),
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

pub(super) fn decode_security_policy(value: wire::SecurityPolicy) -> Result<SecurityPolicy> {
    let rate_limits = value
        .rate_limits
        .into_iter()
        .map(|limit| {
            let key = match wire::RateLimitKey::try_from(limit.key) {
                Ok(wire::RateLimitKey::ClientAddress) => RateLimitKey::ClientAddress,
                Ok(wire::RateLimitKey::Host) => RateLimitKey::Host,
                Ok(wire::RateLimitKey::Route) => RateLimitKey::Route,
                Ok(wire::RateLimitKey::Header) => RateLimitKey::Header { name: limit.header },
                _ => {
                    return Err(PanelError::invalid_argument(format!(
                        "security policy {} has a rate limit without a key",
                        value.id
                    )))
                }
            };
            Ok(RateLimit {
                key,
                requests: limit.requests,
                per_seconds: limit.per_seconds,
                burst: limit.burst,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(SecurityPolicy {
        id: value.id,
        allowed_cidrs: value.allowed_cidrs.into_iter().collect(),
        denied_cidrs: value.denied_cidrs.into_iter().collect(),
        request_rate_per_second: value.has_request_rate.then_some(value.request_rate),
        allowed_methods: value.allowed_methods.into_iter().collect(),
        denied_path_prefixes: value.denied_path_prefixes,
        denied_user_agents: value.denied_user_agents,
        referer: value.referer.map(|rule| RefererRule {
            allowed_hosts: rule.allowed_hosts,
            allow_empty: rule.allow_empty,
        }),
        basic_auth: value.basic_auth.map(|auth| BasicAuth {
            realm: auth.realm,
            users_secret_id: auth.users_secret_id,
        }),
        max_header_bytes: value.max_header_bytes,
        max_body_bytes: value.max_body_bytes,
        body_timeout_ms: value.body_timeout_ms,
        rate_limits,
        max_concurrent_requests: value.max_concurrent_requests,
        limited_response: value
            .limited_response
            .map(|response| {
                Ok::<_, PanelError>(LimitedResponse {
                    status: u16::try_from(response.status).map_err(|_| {
                        PanelError::invalid_argument("a limited response status is out of range")
                    })?,
                    body: response.body,
                    content_type: optional_string(response.content_type),
                })
            })
            .transpose()?,
    })
}

pub(super) fn encode_security_policy(value: &SecurityPolicy) -> wire::SecurityPolicy {
    wire::SecurityPolicy {
        id: value.id.clone(),
        allowed_cidrs: value.allowed_cidrs.iter().cloned().collect(),
        request_rate: value.request_rate_per_second.unwrap_or_default(),
        denied_cidrs: value.denied_cidrs.iter().cloned().collect(),
        has_request_rate: value.request_rate_per_second.is_some(),
        allowed_methods: value.allowed_methods.iter().cloned().collect(),
        denied_path_prefixes: value.denied_path_prefixes.clone(),
        denied_user_agents: value.denied_user_agents.clone(),
        referer: value.referer.as_ref().map(|rule| wire::RefererRule {
            allowed_hosts: rule.allowed_hosts.clone(),
            allow_empty: rule.allow_empty,
        }),
        basic_auth: value.basic_auth.as_ref().map(|auth| wire::BasicAuth {
            realm: auth.realm.clone(),
            users_secret_id: auth.users_secret_id.clone(),
        }),
        max_header_bytes: value.max_header_bytes,
        max_body_bytes: value.max_body_bytes,
        body_timeout_ms: value.body_timeout_ms,
        rate_limits: value
            .rate_limits
            .iter()
            .map(|limit| {
                let (key, header) = match &limit.key {
                    RateLimitKey::ClientAddress => {
                        (wire::RateLimitKey::ClientAddress, String::new())
                    }
                    RateLimitKey::Host => (wire::RateLimitKey::Host, String::new()),
                    RateLimitKey::Route => (wire::RateLimitKey::Route, String::new()),
                    RateLimitKey::Header { name } => (wire::RateLimitKey::Header, name.clone()),
                    _ => (wire::RateLimitKey::Unspecified, String::new()),
                };
                wire::RateLimit {
                    key: key.into(),
                    header,
                    requests: limit.requests,
                    per_seconds: limit.per_seconds,
                    burst: limit.burst,
                }
            })
            .collect(),
        max_concurrent_requests: value.max_concurrent_requests,
        limited_response: value
            .limited_response
            .as_ref()
            .map(|response| wire::LimitedResponse {
                status: u32::from(response.status),
                body: response.body.clone(),
                content_type: response.content_type.clone().unwrap_or_default(),
            }),
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
