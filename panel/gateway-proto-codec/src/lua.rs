//! Lua scripts and handlers (ADR 0039).

use crate::{optional_string, status_code};
use panel_contracts::gateway::v1 as wire;
use panel_errors::{PanelError, Result};
use panel_ir::{
    LuaFallback, LuaHandler, LuaHandlers, LuaLogLevel, LuaPermissions, LuaProgram, LuaScript,
    LuaSharedDict, LuaSockets, LuaTls, LuaVariable,
};

pub(super) fn decode_program(value: Option<wire::LuaProgram>) -> Result<LuaProgram> {
    let Some(value) = value else {
        return Ok(LuaProgram::default());
    };
    Ok(LuaProgram {
        disabled: value.disabled,
        scripts: value
            .scripts
            .into_iter()
            .map(|script| LuaScript {
                id: script.id,
                file: script.file,
                line: script.line,
                source: script.source,
                sha256: script.sha256,
                module: optional_string(script.module),
            })
            .collect(),
        init: value.init.map(decode_handler).transpose()?,
        init_worker: value.init_worker.map(decode_handler).transpose()?,
        exit_worker: value.exit_worker.map(decode_handler).transpose()?,
        shared_dicts: value
            .shared_dicts
            .into_iter()
            .map(|dict| LuaSharedDict {
                name: dict.name,
                capacity_bytes: dict.capacity_bytes,
            })
            .collect(),
        memory_limit_bytes: value.memory_limit_bytes,
        max_pending_timers: value.max_pending_timers,
        max_running_timers: value.max_running_timers,
        regex_cache_max_entries: value.regex_cache_max_entries,
        regex_match_limit: value.regex_match_limit,
        access_first: value.access_first,
        worker_thread_vm_pool_size: value.worker_thread_vm_pool_size,
        capture_error_log_bytes: value.capture_error_log_bytes,
    })
}

pub(super) fn encode_program(value: &LuaProgram) -> Option<wire::LuaProgram> {
    if value.is_empty() {
        return None;
    }
    Some(wire::LuaProgram {
        disabled: value.disabled,
        scripts: value
            .scripts
            .iter()
            .map(|script| wire::LuaScript {
                id: script.id.clone(),
                file: script.file.clone(),
                line: script.line,
                source: script.source.clone(),
                sha256: script.sha256.clone(),
                module: script.module.clone().unwrap_or_default(),
            })
            .collect(),
        init: value.init.as_ref().map(encode_handler),
        init_worker: value.init_worker.as_ref().map(encode_handler),
        exit_worker: value.exit_worker.as_ref().map(encode_handler),
        shared_dicts: value
            .shared_dicts
            .iter()
            .map(|dict| wire::LuaSharedDict {
                name: dict.name.clone(),
                capacity_bytes: dict.capacity_bytes,
            })
            .collect(),
        memory_limit_bytes: value.memory_limit_bytes,
        max_pending_timers: value.max_pending_timers,
        max_running_timers: value.max_running_timers,
        regex_cache_max_entries: value.regex_cache_max_entries,
        regex_match_limit: value.regex_match_limit,
        access_first: value.access_first,
        worker_thread_vm_pool_size: value.worker_thread_vm_pool_size,
        capture_error_log_bytes: value.capture_error_log_bytes,
    })
}

fn decode_fallback(value: Option<wire::LuaFallback>) -> Result<LuaFallback> {
    let Some(value) = value else {
        return Ok(LuaFallback::Fail);
    };
    match wire::LuaFallbackKind::try_from(value.kind) {
        Ok(wire::LuaFallbackKind::Unspecified) => Ok(LuaFallback::Fail),
        Ok(wire::LuaFallbackKind::Continue) => Ok(LuaFallback::Continue),
        Ok(wire::LuaFallbackKind::Status) => Ok(LuaFallback::Status {
            status: status_code(value.status)?,
        }),
        Err(_) => Err(PanelError::invalid_argument(format!(
            "a Lua fallback is of a kind this gateway does not know: {}",
            value.kind
        ))),
    }
}

fn encode_fallback(value: &LuaFallback) -> Option<wire::LuaFallback> {
    match value {
        LuaFallback::Fail => None,
        LuaFallback::Continue => Some(wire::LuaFallback {
            kind: wire::LuaFallbackKind::Continue.into(),
            status: 0,
        }),
        LuaFallback::Status { status } => Some(wire::LuaFallback {
            kind: wire::LuaFallbackKind::Status.into(),
            status: u32::from(*status),
        }),
    }
}

fn decode_level(value: i32) -> Result<LuaLogLevel> {
    use wire::LuaLogLevel as Wire;
    Ok(match Wire::try_from(value) {
        Ok(Wire::Unspecified | Wire::Notice) => LuaLogLevel::Notice,
        Ok(Wire::Stderr) => LuaLogLevel::Stderr,
        Ok(Wire::Emerg) => LuaLogLevel::Emerg,
        Ok(Wire::Alert) => LuaLogLevel::Alert,
        Ok(Wire::Crit) => LuaLogLevel::Crit,
        Ok(Wire::Error) => LuaLogLevel::Error,
        Ok(Wire::Warn) => LuaLogLevel::Warn,
        Ok(Wire::Info) => LuaLogLevel::Info,
        Ok(Wire::Debug) => LuaLogLevel::Debug,
        Err(_) => {
            return Err(PanelError::invalid_argument(format!(
                "unknown Lua log level {value}"
            )));
        }
    })
}

fn encode_level(value: LuaLogLevel) -> i32 {
    use wire::LuaLogLevel as Wire;
    match value {
        LuaLogLevel::Notice => Wire::Unspecified,
        LuaLogLevel::Stderr => Wire::Stderr,
        LuaLogLevel::Emerg => Wire::Emerg,
        LuaLogLevel::Alert => Wire::Alert,
        LuaLogLevel::Crit => Wire::Crit,
        LuaLogLevel::Error => Wire::Error,
        LuaLogLevel::Warn => Wire::Warn,
        LuaLogLevel::Info => Wire::Info,
        LuaLogLevel::Debug => Wire::Debug,
    }
    .into()
}

pub(super) fn decode_handler(value: wire::LuaHandler) -> Result<LuaHandler> {
    if value.script_id.is_empty() {
        return Err(PanelError::invalid_argument(
            "a Lua handler names no script",
        ));
    }
    let allow = value.allow.unwrap_or_default();
    Ok(LuaHandler {
        script_id: value.script_id,
        time_limit_ms: value.time_limit_ms,
        work_limit: value.work_limit,
        allow: LuaPermissions {
            body: allow.body,
            upstream: allow.upstream,
            network: allow.network,
        },
        on_error: decode_fallback(value.on_error)?,
        log_level: decode_level(value.log_level)?,
        slow_threshold_ms: value.slow_threshold_ms,
        debug: value.debug,
        sockets: value
            .sockets
            .map(|sockets| LuaSockets {
                connect_timeout_ms: sockets.connect_timeout_ms,
                send_timeout_ms: sockets.send_timeout_ms,
                read_timeout_ms: sockets.read_timeout_ms,
                buffer_bytes: sockets.buffer_bytes,
                pool_size: sockets.pool_size,
                keepalive_timeout_ms: sockets.keepalive_timeout_ms,
                quiet: sockets.quiet,
                tls: Box::new(
                    sockets
                        .tls
                        .map(|tls| *tls)
                        .map(|tls| LuaTls {
                            trusted_certificate_secret_id: tls.trusted_certificate_secret_id,
                            crl_secret_id: tls.crl_secret_id,
                            certificate_secret_id: tls.certificate_secret_id,
                            certificate_key_secret_id: tls.certificate_key_secret_id,
                            verify_depth: tls.verify_depth,
                            protocols: tls.protocols,
                            cipher_suites: tls.cipher_suites,
                        })
                        .unwrap_or_default(),
                ),
            })
            .unwrap_or_default(),
        keep_underscores: value.keep_underscores,
        no_default_type: value.no_default_type,
        read_body_first: value.read_body_first,
        check_client_abort: value.check_client_abort,
    })
}

pub(super) fn encode_handler(value: &LuaHandler) -> wire::LuaHandler {
    wire::LuaHandler {
        script_id: value.script_id.clone(),
        time_limit_ms: value.time_limit_ms,
        work_limit: value.work_limit,
        allow: (!value.allow.is_none()).then_some(wire::LuaPermissions {
            body: value.allow.body,
            upstream: value.allow.upstream,
            network: value.allow.network,
        }),
        on_error: encode_fallback(&value.on_error),
        log_level: encode_level(value.log_level),
        slow_threshold_ms: value.slow_threshold_ms,
        debug: value.debug,
        sockets: (!value.sockets.is_default()).then_some(wire::LuaSockets {
            connect_timeout_ms: value.sockets.connect_timeout_ms,
            send_timeout_ms: value.sockets.send_timeout_ms,
            read_timeout_ms: value.sockets.read_timeout_ms,
            buffer_bytes: value.sockets.buffer_bytes,
            pool_size: value.sockets.pool_size,
            keepalive_timeout_ms: value.sockets.keepalive_timeout_ms,
            quiet: value.sockets.quiet,
            tls: (!value.sockets.tls.is_default()).then(|| {
                let tls = &value.sockets.tls;
                Box::new(wire::LuaTls {
                    trusted_certificate_secret_id: tls.trusted_certificate_secret_id.clone(),
                    crl_secret_id: tls.crl_secret_id.clone(),
                    certificate_secret_id: tls.certificate_secret_id.clone(),
                    certificate_key_secret_id: tls.certificate_key_secret_id.clone(),
                    verify_depth: tls.verify_depth,
                    protocols: tls.protocols.clone(),
                    cipher_suites: tls.cipher_suites.clone(),
                })
            }),
        }),
        keep_underscores: value.keep_underscores,
        no_default_type: value.no_default_type,
        read_body_first: value.read_body_first,
        check_client_abort: value.check_client_abort,
    }
}

pub(super) fn decode_handlers(value: Option<wire::LuaHandlers>) -> Result<LuaHandlers> {
    let Some(value) = value else {
        return Ok(LuaHandlers::default());
    };
    let handler = |handler: Option<wire::LuaHandler>| handler.map(decode_handler).transpose();
    Ok(LuaHandlers {
        server_rewrite: handler(value.server_rewrite)?,
        rewrite: handler(value.rewrite)?,
        access: handler(value.access)?,
        precontent: handler(value.precontent)?,
        header_filter: handler(value.header_filter)?,
        body_filter: handler(value.body_filter)?,
        log: handler(value.log)?,
        ssl_client_hello: handler(value.ssl_client_hello)?,
        ssl_cert: handler(value.ssl_cert)?,
        variables: value
            .variables
            .into_iter()
            .map(|variable| {
                Ok(LuaVariable {
                    name: variable.name,
                    value: variable.value,
                    handler: handler(variable.handler)?,
                    args: variable.args,
                })
            })
            .collect::<Result<_>>()?,
    })
}

pub(super) fn encode_handlers(value: &LuaHandlers) -> Option<wire::LuaHandlers> {
    if value.is_empty() {
        return None;
    }
    let handler = |handler: &Option<LuaHandler>| handler.as_ref().map(encode_handler);
    Some(wire::LuaHandlers {
        server_rewrite: handler(&value.server_rewrite),
        rewrite: handler(&value.rewrite),
        access: handler(&value.access),
        precontent: handler(&value.precontent),
        header_filter: handler(&value.header_filter),
        body_filter: handler(&value.body_filter),
        log: handler(&value.log),
        ssl_client_hello: handler(&value.ssl_client_hello),
        ssl_cert: handler(&value.ssl_cert),
        variables: value
            .variables
            .iter()
            .map(|variable| wire::LuaVariable {
                name: variable.name.clone(),
                value: variable.value.clone(),
                handler: handler(&variable.handler),
                args: variable.args.clone(),
            })
            .collect(),
    })
}
