//! Checks of Lua scripts (ADR 0039): each script has a unique id and the
//! SHA-256 of its source, modules have unique names, every handler names a
//! script of the snapshot within bounded limits, shared dictionaries have
//! names and sizes, and snapshots carrying scripts require the capability.

use panel_domain::ContentHash;
use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::{
    LuaFallback, LuaHandler, LuaVariable, RouteAction, RuntimeSnapshot, LUA_SCRIPTS_CAPABILITY,
};
use std::collections::BTreeSet;

/// The longest a run may take, a minute.
pub const MOST_LUA_TIME_MS: u64 = 60_000;
/// The most function calls and loop iterations a run may make.
pub const MOST_LUA_WORK: u64 = 10_000_000_000;
/// The largest script.
pub const MOST_LUA_SCRIPT_BYTES: usize = 1 << 20;
/// The least and most memory a VM may be given.
pub const LEAST_LUA_MEMORY_BYTES: u64 = 1 << 20;
pub const MOST_LUA_MEMORY_BYTES: u64 = 4 << 30;
/// The smallest and largest shared dictionary.
/// The longest a `lua_socket_*` timeout may be: a day.
pub const MOST_LUA_SOCKET_MS: u64 = 86_400_000;
/// What `lua_socket_buffer_size` may be.
pub const LEAST_LUA_SOCKET_BUFFER: u64 = 1 << 10;
pub const MOST_LUA_SOCKET_BUFFER: u64 = 16 << 20;
/// The most connections `lua_socket_pool_size` may keep for a pool.
pub const MOST_LUA_SOCKET_POOL: u64 = 1 << 16;
/// The most VMs `lua_worker_thread_vm_pool_size` may allow.
pub const MOST_LUA_WORKER_VMS: u64 = 1024;
/// What `lua_capture_error_log` may keep for each VM.
pub const LEAST_LUA_ERROR_LOG_BYTES: u64 = 1 << 10;
pub const MOST_LUA_ERROR_LOG_BYTES: u64 = 256 << 20;
/// The most variables a site or a route may set.
pub const MOST_LUA_VARIABLES: usize = 256;
/// The most intermediate certificates `lua_ssl_verify_depth` may allow, as
/// OpenSSL allows.
pub const MOST_LUA_VERIFY_DEPTH: u32 = 100;
/// The most `lua_max_pending_timers`, `lua_max_running_timers` and
/// `lua_regex_cache_max_entries` may set.
pub const MOST_LUA_TIMERS: u64 = 1 << 20;
pub const MOST_LUA_REGEX_CACHE: u64 = 1 << 20;
pub const LEAST_LUA_DICT_BYTES: u64 = 8 << 10;
pub const MOST_LUA_DICT_BYTES: u64 = 4 << 30;

/// Whether `snapshot` carries scripts.
pub fn uses_lua(snapshot: &RuntimeSnapshot) -> bool {
    !snapshot.lua.is_empty()
        || snapshot.sites.iter().any(|site| !site.lua.is_empty())
        || snapshot
            .routes
            .iter()
            .any(|route| !route.lua.is_empty() || matches!(route.action, RouteAction::Lua { .. }))
        || snapshot
            .upstream_pools
            .iter()
            .any(|pool| pool.balancer.is_some())
}

fn module_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

fn dict_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn check_handler(
    found: &mut Vec<(String, String)>,
    ids: &BTreeSet<&str>,
    resource: String,
    phase: &str,
    handler: &LuaHandler,
) {
    if !ids.contains(handler.script_id.as_str()) {
        found.push((
            resource.clone(),
            format!(
                "{phase} handler names script {}, which the snapshot does not have",
                handler.script_id
            ),
        ));
    }
    if handler.time_limit_ms > MOST_LUA_TIME_MS {
        found.push((
            resource.clone(),
            format!(
                "{phase} handler may run {} ms, longer than {MOST_LUA_TIME_MS}",
                handler.time_limit_ms
            ),
        ));
    }
    if handler.work_limit > MOST_LUA_WORK {
        found.push((
            resource.clone(),
            format!("{phase} handler's work limit is above {MOST_LUA_WORK}"),
        ));
    }
    let sockets = &handler.sockets;
    for (name, ms) in [
        ("lua_socket_connect_timeout", sockets.connect_timeout_ms),
        ("lua_socket_send_timeout", sockets.send_timeout_ms),
        ("lua_socket_read_timeout", sockets.read_timeout_ms),
        ("lua_socket_keepalive_timeout", sockets.keepalive_timeout_ms),
    ] {
        if ms > MOST_LUA_SOCKET_MS {
            found.push((
                resource.clone(),
                format!("{phase} handler's {name} is over {MOST_LUA_SOCKET_MS} ms"),
            ));
        }
    }
    if sockets.buffer_bytes != 0
        && !(LEAST_LUA_SOCKET_BUFFER..=MOST_LUA_SOCKET_BUFFER).contains(&sockets.buffer_bytes)
    {
        found.push((
            resource.clone(),
            format!(
                "{phase} handler's lua_socket_buffer_size is not {LEAST_LUA_SOCKET_BUFFER} to {MOST_LUA_SOCKET_BUFFER} bytes"
            ),
        ));
    }
    let tls = &sockets.tls;
    for (directive, secret) in [
        (
            "lua_ssl_trusted_certificate",
            &tls.trusted_certificate_secret_id,
        ),
        ("lua_ssl_crl", &tls.crl_secret_id),
        ("lua_ssl_certificate", &tls.certificate_secret_id),
        ("lua_ssl_certificate_key", &tls.certificate_key_secret_id),
    ] {
        if secret
            .as_deref()
            .is_some_and(|secret| secret.is_empty() || secret.contains(char::is_control))
        {
            found.push((
                resource.clone(),
                format!("{phase} handler's {directive} names no valid secret"),
            ));
        }
    }
    if tls.crl_secret_id.is_some() && tls.trusted_certificate_secret_id.is_none() {
        found.push((
            resource.clone(),
            format!("{phase} handler's lua_ssl_crl needs lua_ssl_trusted_certificate"),
        ));
    }
    if tls.certificate_secret_id.is_some() != tls.certificate_key_secret_id.is_some() {
        found.push((
            resource.clone(),
            format!(
                "{phase} handler's lua_ssl_certificate and lua_ssl_certificate_key go together"
            ),
        ));
    }
    if tls
        .verify_depth
        .is_some_and(|depth| depth > MOST_LUA_VERIFY_DEPTH)
    {
        found.push((
            resource.clone(),
            format!("{phase} handler's lua_ssl_verify_depth is over {MOST_LUA_VERIFY_DEPTH}"),
        ));
    }
    if let Some(protocol) = tls
        .protocols
        .iter()
        .find(|protocol| !panel_ir::tls::PROTOCOLS.contains(&protocol.as_str()))
    {
        found.push((
            resource.clone(),
            format!("{phase} handler offers {protocol}, which is not TLSv1.2 or TLSv1.3"),
        ));
    }
    if let Some(suite) = tls.cipher_suites.iter().find(|suite| {
        panel_ir::tls::suite_version(suite) != Some(panel_ir::tls::SuiteVersion::Tls12)
    }) {
        found.push((
            resource.clone(),
            format!("{phase} handler offers {suite}, which is not a TLS 1.2 cipher suite"),
        ));
    }
    if sockets.pool_size > MOST_LUA_SOCKET_POOL {
        found.push((
            resource.clone(),
            format!("{phase} handler's lua_socket_pool_size is over {MOST_LUA_SOCKET_POOL}"),
        ));
    }
    if let LuaFallback::Status { status } = handler.on_error {
        if !(200..=599).contains(&status) {
            found.push((
                resource,
                format!("{phase} handler falls back to status {status}, not 200 to 599"),
            ));
        }
    }
}

fn check_variables(found: &mut Vec<(String, String)>, resource: &str, variables: &[LuaVariable]) {
    if variables.len() > MOST_LUA_VARIABLES {
        found.push((
            resource.to_owned(),
            format!("more than {MOST_LUA_VARIABLES} variables are set"),
        ));
    }
    for variable in variables {
        let name = variable.name.as_bytes();
        if !name
            .first()
            .is_some_and(|first| first.is_ascii_alphabetic() || *first == b'_')
            || !name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            found.push((
                resource.to_owned(),
                format!("{:?} is not a variable name", variable.name),
            ));
        }
        if variable.handler.is_none() && !variable.args.is_empty() {
            found.push((
                resource.to_owned(),
                format!("${} has arguments but no handler", variable.name),
            ));
        }
        for arg in &variable.args {
            if let Err(error) = panel_ir::template::parse_template(arg) {
                found.push((
                    resource.to_owned(),
                    format!(
                        "an argument of ${} is not a template: {error}",
                        variable.name
                    ),
                ));
            }
        }
    }
}

/// What is wrong with `snapshot`'s scripts and handlers, with the resource
/// each problem is in.
pub fn problems(snapshot: &RuntimeSnapshot) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let program = &snapshot.lua;
    let mut ids = BTreeSet::new();
    let mut modules = BTreeSet::new();
    for script in &program.scripts {
        let resource = || format!("lua:{}", script.id);
        if script.id.is_empty() {
            found.push(("lua".into(), "a script has no id".into()));
            continue;
        }
        if !ids.insert(script.id.as_str()) {
            found.push((resource(), format!("script {} appears twice", script.id)));
        }
        if script.line == 0 {
            found.push((resource(), format!("script {} starts on line 0", script.id)));
        }
        if script.source.len() > MOST_LUA_SCRIPT_BYTES {
            found.push((
                resource(),
                format!(
                    "script {} is larger than {MOST_LUA_SCRIPT_BYTES} bytes",
                    script.id
                ),
            ));
        }
        if ContentHash::from_bytes(script.source.as_bytes()).as_str() != script.sha256 {
            found.push((
                resource(),
                format!("script {} does not match its SHA-256", script.id),
            ));
        }
        if let Some(module) = &script.module {
            if !module_name_is_valid(module) {
                found.push((resource(), format!("module name {module:?} is invalid")));
            } else if !modules.insert(module.as_str()) {
                found.push((resource(), format!("module {module} is defined twice")));
            }
        }
    }
    if let Some(init) = &program.init {
        check_handler(&mut found, &ids, "lua".into(), "init", init);
    }
    if let Some(exit) = &program.exit_worker {
        check_handler(&mut found, &ids, "lua".into(), "exit_worker", exit);
    }
    if let Some(init) = &program.init_worker {
        check_handler(&mut found, &ids, "lua".into(), "init_worker", init);
    }
    for site in &snapshot.sites {
        for (phase, value) in site.lua.iter() {
            check_handler(&mut found, &ids, site.id.as_str().into(), phase, value);
        }
        check_variables(&mut found, site.id.as_str(), &site.lua.variables);
    }
    for route in &snapshot.routes {
        check_variables(&mut found, route.id.as_str(), &route.lua.variables);
        for (phase, value) in route.lua.iter() {
            if matches!(phase, "server_rewrite" | "ssl_client_hello" | "ssl_cert") {
                found.push((
                    route.id.as_str().into(),
                    format!("a route cannot have a {phase} handler; sites do"),
                ));
            }
            check_handler(&mut found, &ids, route.id.as_str().into(), phase, value);
        }
        if let RouteAction::Lua { handler: value } = &route.action {
            check_handler(&mut found, &ids, route.id.as_str().into(), "content", value);
        }
    }
    for pool in &snapshot.upstream_pools {
        if let Some(value) = &pool.balancer {
            check_handler(&mut found, &ids, pool.id.as_str().into(), "balancer", value);
        }
    }
    if program.memory_limit_bytes != 0
        && !(LEAST_LUA_MEMORY_BYTES..=MOST_LUA_MEMORY_BYTES).contains(&program.memory_limit_bytes)
    {
        found.push((
            "lua".into(),
            format!(
                "VMs get {} bytes, not {LEAST_LUA_MEMORY_BYTES} to {MOST_LUA_MEMORY_BYTES}",
                program.memory_limit_bytes
            ),
        ));
    }
    for (name, value) in [
        ("lua_max_pending_timers", program.max_pending_timers),
        ("lua_max_running_timers", program.max_running_timers),
    ] {
        if value > MOST_LUA_TIMERS {
            found.push((
                "lua".into(),
                format!("{name} {value} is over {MOST_LUA_TIMERS}"),
            ));
        }
    }
    if program
        .regex_cache_max_entries
        .is_some_and(|entries| entries > MOST_LUA_REGEX_CACHE)
    {
        found.push((
            "lua".into(),
            format!("lua_regex_cache_max_entries is over {MOST_LUA_REGEX_CACHE}"),
        ));
    }
    if program.capture_error_log_bytes != 0
        && !(LEAST_LUA_ERROR_LOG_BYTES..=MOST_LUA_ERROR_LOG_BYTES)
            .contains(&program.capture_error_log_bytes)
    {
        found.push((
            "lua".into(),
            format!(
                "lua_capture_error_log is not {LEAST_LUA_ERROR_LOG_BYTES} to {MOST_LUA_ERROR_LOG_BYTES} bytes"
            ),
        ));
    }
    if program.worker_thread_vm_pool_size > MOST_LUA_WORKER_VMS {
        found.push((
            "lua".into(),
            format!("lua_worker_thread_vm_pool_size is over {MOST_LUA_WORKER_VMS}"),
        ));
    }
    if program.regex_match_limit > u64::from(u32::MAX) {
        found.push((
            "lua".into(),
            format!("lua_regex_match_limit is over {}", u32::MAX),
        ));
    }
    let mut dicts = BTreeSet::new();
    for dict in &program.shared_dicts {
        if !dict_name_is_valid(&dict.name) {
            found.push((
                "lua".into(),
                format!("shared dictionary name {:?} is invalid", dict.name),
            ));
        } else if !dicts.insert(dict.name.as_str()) {
            found.push((
                "lua".into(),
                format!("shared dictionary {} is declared twice", dict.name),
            ));
        }
        if dict.capacity_bytes < LEAST_LUA_DICT_BYTES || dict.capacity_bytes > MOST_LUA_DICT_BYTES {
            found.push((
                "lua".into(),
                format!(
                    "shared dictionary {} holds {} bytes, not 8 KiB to {MOST_LUA_DICT_BYTES}",
                    dict.name, dict.capacity_bytes
                ),
            ));
        }
    }
    found
}

pub(crate) fn validate_lua(snapshot: &RuntimeSnapshot, diagnostics: &mut Vec<Diagnostic>) {
    for (resource, problem) in problems(snapshot) {
        diagnostics
            .push(Diagnostic::error(ErrorCode::VALIDATION_FAILED, problem).with_resource(resource));
    }
    let declared = snapshot
        .required_capabilities()
        .iter()
        .any(|capability| capability.name == LUA_SCRIPTS_CAPABILITY);
    if uses_lua(snapshot) && !declared {
        diagnostics.push(
            Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!("Lua scripts are used without requiring {LUA_SCRIPTS_CAPABILITY}"),
            )
            .with_resource("lua"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{RevisionId, RouteId, SiteId, UpstreamPoolId};
    use panel_ir::{
        CapabilityRequirement, LuaHandler, LuaScript, LuaSharedDict, RouteMatcher, RouteSpec,
        UpstreamPoolSpec,
    };

    fn script(id: &str, source: &str) -> LuaScript {
        LuaScript {
            id: id.into(),
            file: "main.conf".into(),
            line: 1,
            source: source.into(),
            sha256: ContentHash::from_bytes(source.as_bytes()).as_str().into(),
            module: None,
        }
    }

    #[test]
    fn handlers_name_scripts_of_the_snapshot_within_bounds() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.lua.scripts = vec![script("a", "return 1"), script("a", "return 2")];
        snapshot.lua.scripts[1].sha256 = "00".repeat(32);
        snapshot.lua.shared_dicts.push(LuaSharedDict {
            name: "bad name".into(),
            capacity_bytes: 1,
        });
        let mut route = RouteSpec::new(
            RouteId::new("r").unwrap(),
            SiteId::new("s").unwrap(),
            1,
            RouteMatcher::ExactPath { path: "/".into() },
            RouteAction::Lua {
                handler: LuaHandler::new("missing"),
            },
        );
        let mut slow = LuaHandler::new("a");
        slow.time_limit_ms = MOST_LUA_TIME_MS + 1;
        route.lua.server_rewrite = Some(slow);
        snapshot.routes.push(route);
        let mut pool = UpstreamPoolSpec::new(UpstreamPoolId::new("p").unwrap(), "p", Vec::new());
        let mut balancer = LuaHandler::new("a");
        balancer.on_error = LuaFallback::Status { status: 999 };
        pool.balancer = Some(balancer);
        snapshot.upstream_pools.push(pool);
        let found: Vec<String> = problems(&snapshot)
            .into_iter()
            .map(|(_, problem)| problem)
            .collect();
        for expected in [
            "script a appears twice",
            "script a does not match its SHA-256",
            "content handler names script missing",
            "a route cannot have a server_rewrite handler",
            "server_rewrite handler may run 60001 ms",
            "balancer handler falls back to status 999",
            "shared dictionary name \"bad name\" is invalid",
            "shared dictionary bad name holds 1 bytes",
        ] {
            assert!(
                found.iter().any(|problem| problem.contains(expected)),
                "{expected}: {found:?}"
            );
        }
    }

    #[test]
    fn scripts_require_the_capability() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.lua.scripts.push(script("a", "return 1"));
        let mut diagnostics = Vec::new();
        validate_lua(&snapshot, &mut diagnostics);
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains(LUA_SCRIPTS_CAPABILITY)));
        snapshot
            .required_capabilities
            .push(CapabilityRequirement::new(LUA_SCRIPTS_CAPABILITY, "1"));
        diagnostics.clear();
        validate_lua(&snapshot, &mut diagnostics);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}
