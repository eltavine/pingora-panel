//! `require`: built-in modules and the configuration's `lua/` files, loaded
//! once per VM, each module in an environment of its own.

use super::{
    bit, cell,
    codec::{decode_base64url, encode_base64url},
    exchange, failed, json, require_permission, results, Api, Context,
};
use crate::{
    exchange::Peer,
    program::Compiled,
    vm::{refused, Slot},
};
use md5::{Digest, Md5};
use mlua::{
    chunk::ChunkMode, Function, Lua, LuaString, MultiValue, Table, UserData, UserDataMethods, Value,
};
use parking_lot::Mutex;
use sha1::Sha1;
use sha2::{Sha224, Sha256, Sha384, Sha512};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
    time::Duration,
};

pub(super) fn require(lua: &Lua, context: &Context, ngx: &Table) -> mlua::Result<Function> {
    let loaded = lua.create_table()?;
    let package = lua.create_table()?;
    package.raw_set("loaded", loaded.clone())?;
    package.raw_set("path", "")?;
    package.raw_set("cpath", "")?;
    let globals = lua.globals();
    globals.raw_set("package", package)?;
    let module_meta = lua.create_table()?;
    module_meta.raw_set("__index", globals.clone())?;
    module_meta.set_readonly(true);
    let loading = Arc::new(Mutex::new(HashSet::<String>::new()));
    let modules = Arc::clone(&context.modules);
    let slot = Arc::clone(&context.slot);
    let ngx = ngx.clone();
    lua.create_function(move |lua, name: LuaString| {
        let name = name.to_str()?.to_owned();
        let cached: Value = loaded.raw_get(name.as_str())?;
        if !cached.is_nil() {
            return Ok(cached);
        }
        if !loading.lock().insert(name.clone()) {
            return Err(mlua::Error::runtime(format!(
                "loop or previous error loading module '{name}'"
            )));
        }
        let result = load(lua, &name, &modules, &ngx, &globals, &module_meta, &slot);
        loading.lock().remove(&name);
        let value = match result? {
            Value::Nil => Value::Boolean(true),
            value => value,
        };
        loaded.raw_set(name.as_str(), value.clone())?;
        Ok(value)
    })
}

fn load(
    lua: &Lua,
    name: &str,
    modules: &BTreeMap<String, Compiled>,
    ngx: &Table,
    globals: &Table,
    module_meta: &Table,
    slot: &Arc<Slot>,
) -> mlua::Result<Value> {
    if name == "panel.v1" {
        return panel_v1(lua, module_meta);
    }
    if let Some((_, source)) = IN_LUA.iter().find(|(module, _)| *module == name) {
        let env = lua.create_table()?;
        env.set_metatable(Some(module_meta.clone()))?;
        env.set_safeenv(true);
        return lua
            .load(*source)
            .set_name(format!("={name}"))
            .set_environment(env)
            .into_function()?
            .call::<Value>(());
    }
    if let Some(module) = built_in(lua, name, ngx, globals, slot)? {
        return Ok(module);
    }
    if let Some(reason) = refusal(name) {
        return Err(refused(format!(
            "module '{name}' is not available: {reason}"
        )));
    }
    let Some(compiled) = modules.get(name) else {
        return Err(mlua::Error::runtime(format!(
            "module '{name}' not found: it is neither built in nor a file in the configuration's lua/ directory"
        )));
    };
    let env = lua.create_table()?;
    env.set_metatable(Some(module_meta.clone()))?;
    env.set_safeenv(true);
    lua.load(&compiled.bytecode[..])
        .set_name(format!("={}", compiled.name))
        .set_mode(ChunkMode::Binary)
        .set_environment(env)
        .into_function()?
        .call::<Value>(name)
}

/// `panel.v1`: the Lua of its façade over `ngx`, given the primitives it
/// needs that `ngx` lacks.
fn panel_v1(lua: &Lua, module_meta: &Table) -> mlua::Result<Value> {
    fn hex_digest<D: Digest>(data: &[u8]) -> String {
        hex::encode(D::digest(data))
    }
    fn hmac_sha256(key: &[u8], data: &[u8]) -> String {
        use hmac::{KeyInit, Mac};
        let mut mac =
            hmac::Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes keys of any length");
        mac.update(data);
        hex::encode(mac.finalize().into_bytes())
    }
    fn hmac_sha512(key: &[u8], data: &[u8]) -> String {
        use hmac::{KeyInit, Mac};
        let mut mac =
            hmac::Hmac::<Sha512>::new_from_slice(key).expect("HMAC takes keys of any length");
        mac.update(data);
        hex::encode(mac.finalize().into_bytes())
    }
    let native = lua.create_table()?;
    native.raw_set(
        "sha256",
        lua.create_function(|_, data: LuaString| Ok(hex_digest::<Sha256>(&data.as_bytes())))?,
    )?;
    native.raw_set(
        "sha512",
        lua.create_function(|_, data: LuaString| Ok(hex_digest::<Sha512>(&data.as_bytes())))?,
    )?;
    native.raw_set(
        "hmac_sha256",
        lua.create_function(|_, (key, data): (LuaString, LuaString)| {
            Ok(hmac_sha256(&key.as_bytes(), &data.as_bytes()))
        })?,
    )?;
    native.raw_set(
        "hmac_sha512",
        lua.create_function(|_, (key, data): (LuaString, LuaString)| {
            Ok(hmac_sha512(&key.as_bytes(), &data.as_bytes()))
        })?,
    )?;
    native.raw_set(
        "equal",
        lua.create_function(|_, (left, right): (LuaString, LuaString)| {
            let (left, right) = (left.as_bytes(), right.as_bytes());
            let differ = left
                .iter()
                .zip(right.iter())
                .fold(left.len() ^ right.len(), |differ, (a, b)| {
                    differ | usize::from(a ^ b)
                });
            Ok(differ == 0)
        })?,
    )?;
    native.raw_set(
        "random_bytes",
        lua.create_function(|lua, length: usize| {
            let mut buffer = vec![0u8; length.min(1 << 20)];
            getrandom::fill(&mut buffer)
                .map_err(|_| mlua::Error::runtime("no random source is available"))?;
            lua.create_string(buffer)
        })?,
    )?;
    native.raw_set(
        "uuid",
        lua.create_function(|_, ()| Ok(uuid::Uuid::new_v4().to_string()))?,
    )?;
    native.raw_set(
        "base64",
        lua.create_function(|_, data: LuaString| {
            use base64::Engine;
            Ok(base64::engine::general_purpose::STANDARD.encode(data.as_bytes()))
        })?,
    )?;
    native.raw_set(
        "unbase64",
        lua.create_function(|lua, text: LuaString| {
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(text.as_bytes()) {
                Ok(bytes) => Ok(Value::String(lua.create_string(bytes)?)),
                Err(_) => Ok(Value::Nil),
            }
        })?,
    )?;
    native.raw_set(
        "base64url",
        lua.create_function(|_, data: LuaString| Ok(encode_base64url(&data.as_bytes())))?,
    )?;
    native.raw_set(
        "unbase64url",
        lua.create_function(
            |lua, text: LuaString| match decode_base64url(&text.as_bytes()) {
                Some(bytes) => Ok(Value::String(lua.create_string(bytes)?)),
                None => Ok(Value::Nil),
            },
        )?,
    )?;
    let env = lua.create_table()?;
    env.set_metatable(Some(module_meta.clone()))?;
    env.set_safeenv(true);
    lua.load(include_str!("panel_v1.lua"))
        .set_name("=panel.v1")
        .set_environment(env)
        .into_function()?
        .call::<Value>(native)
}

/// OpenResty's modules that scripts may not load, and why.
const REFUSED: [(&str, &str); 2] = [
    (
        "ngx.pipe",
        "it would start processes on the gateway's host, outside the sandbox",
    ),
    (
        "ffi",
        "native code called through an FFI would run outside the sandbox",
    ),
];

/// lua-resty-core's modules besides `resty.core.base`, which replace `ngx`
/// functions with FFI ones there and leave nothing to do here.
const RESTY_CORE: [&str; 20] = [
    "resty.core",
    "resty.core.base64",
    "resty.core.coroutine",
    "resty.core.ctx",
    "resty.core.exit",
    "resty.core.hash",
    "resty.core.misc",
    "resty.core.ndk",
    "resty.core.param",
    "resty.core.phase",
    "resty.core.regex",
    "resty.core.request",
    "resty.core.response",
    "resty.core.shdict",
    "resty.core.socket",
    "resty.core.time",
    "resty.core.uri",
    "resty.core.utils",
    "resty.core.var",
    "resty.core.worker",
];

/// The built-in modules written in Lua, and their sources.
const IN_LUA: [(&str, &str); 16] = [
    ("resty.core.base", include_str!("resty_core_base.lua")),
    ("resty.lrucache", include_str!("lrucache.lua")),
    ("resty.lrucache.pureffi", include_str!("lrucache.lua")),
    ("tablepool", include_str!("tablepool.lua")),
    ("resty.limit.req", include_str!("limit_req.lua")),
    ("resty.limit.conn", include_str!("limit_conn.lua")),
    ("resty.limit.count", include_str!("limit_count.lua")),
    ("resty.limit.traffic", include_str!("limit_traffic.lua")),
    ("resty.redis", include_str!("redis.lua")),
    ("resty.upload", include_str!("upload.lua")),
    ("resty.memcached", include_str!("memcached.lua")),
    ("resty.mysql", include_str!("mysql.lua")),
    (
        "resty.upstream.healthcheck",
        include_str!("healthcheck.lua"),
    ),
    (
        "resty.websocket.protocol",
        include_str!("websocket_protocol.lua"),
    ),
    (
        "resty.websocket.server",
        include_str!("websocket_server.lua"),
    ),
    (
        "resty.websocket.client",
        include_str!("websocket_client.lua"),
    ),
];

/// Why scripts may not load `name`, an OpenResty module.
pub(crate) fn refusal(name: &str) -> Option<&'static str> {
    REFUSED
        .iter()
        .find(|(refused, _)| *refused == name)
        .map(|(_, reason)| *reason)
}

/// The modules OpenResty scripts commonly load that come with the gateway.
pub(crate) const BUILT_IN: [&str; 73] = [
    "panel.v1",
    "cjson",
    "cjson.safe",
    "bit",
    "table.new",
    "table.clear",
    "table.nkeys",
    "table.isempty",
    "table.isarray",
    "table.clone",
    "resty.core",
    "resty.core.base",
    "resty.core.base64",
    "resty.core.coroutine",
    "resty.core.ctx",
    "resty.core.exit",
    "resty.core.hash",
    "resty.core.misc",
    "resty.core.ndk",
    "resty.core.param",
    "resty.core.phase",
    "resty.core.regex",
    "resty.core.request",
    "resty.core.response",
    "resty.core.shdict",
    "resty.core.socket",
    "resty.core.time",
    "resty.core.uri",
    "resty.core.utils",
    "resty.core.var",
    "resty.core.worker",
    "resty.string",
    "resty.random",
    "resty.aes",
    "resty.md5",
    "resty.sha1",
    "resty.sha224",
    "resty.sha256",
    "resty.sha384",
    "resty.sha512",
    "ngx.re",
    "ngx.base64",
    "ngx.balancer",
    "ngx.semaphore",
    "ngx.resp",
    "ngx.req",
    "ngx.process",
    "resty.lrucache",
    "resty.lrucache.pureffi",
    "resty.websocket.protocol",
    "resty.websocket.server",
    "resty.websocket.client",
    "resty.lock",
    "tablepool",
    "resty.limit.req",
    "resty.limit.conn",
    "resty.limit.count",
    "resty.limit.traffic",
    "resty.redis",
    "resty.dns.resolver",
    "resty.upload",
    "resty.memcached",
    "resty.mysql",
    "ngx.upstream",
    "resty.upstream.healthcheck",
    "ngx.errlog",
    "ngx.ssl",
    "ngx.ssl.clienthello",
    "ngx.ssl.session",
    "ngx.ocsp",
    "ngx.ssl.proxysslcert",
    "ngx.ssl.proxysslverify",
    "ngx.proxyssl",
];

fn built_in(
    lua: &Lua,
    name: &str,
    ngx: &Table,
    globals: &Table,
    slot: &Arc<Slot>,
) -> mlua::Result<Option<Value>> {
    let table_library: Table = globals.raw_get("table")?;
    Ok(Some(match name {
        "cjson" => Value::Table(json::instance(lua, false)?),
        "cjson.safe" => Value::Table(json::instance(lua, true)?),
        "bit" => Value::Table(bit::module(lua)?),
        "table.new" => {
            let create: Function = table_library.raw_get("create")?;
            Value::Function(lua.create_function(
                move |_, (narr, _nrec): (Option<usize>, Option<usize>)| {
                    create.call::<Table>(narr.unwrap_or(0).min(1 << 24))
                },
            )?)
        }
        "table.clear" => table_library.raw_get("clear")?,
        "table.clone" => table_library.raw_get("clone")?,
        "table.nkeys" => Value::Function(
            lua.create_function(|_, table: Table| Ok(table.pairs::<Value, Value>().count()))?,
        ),
        "table.isempty" => Value::Function(lua.create_function(|_, table: Table| {
            Ok(table.pairs::<Value, Value>().next().is_none())
        })?),
        "table.isarray" => Value::Function(lua.create_function(|_, table: Table| {
            let mut count = 0usize;
            for pair in table.pairs::<Value, Value>() {
                match pair?.0 {
                    Value::Integer(index) if index >= 1 => count += 1,
                    _ => return Ok(false),
                }
            }
            Ok(count == table.raw_len())
        })?),
        name if RESTY_CORE.contains(&name) => {
            let module = lua.create_table()?;
            module.raw_set("version", "0.1.31")?;
            Value::Table(module)
        }
        "resty.string" => {
            let module = lua.create_table()?;
            module.raw_set(
                "to_hex",
                lua.create_function(|_, bytes: LuaString| Ok(hex::encode(&*bytes.as_bytes())))?,
            )?;
            module.raw_set(
                "atoi",
                lua.create_function(|_, text: LuaString| {
                    Ok(text
                        .to_str()
                        .ok()
                        .and_then(|text| text.trim().parse::<i64>().ok()))
                })?,
            )?;
            Value::Table(module)
        }
        "resty.random" => {
            let module = lua.create_table()?;
            module.raw_set(
                "bytes",
                lua.create_function(|lua, (length, _strong): (usize, Option<bool>)| {
                    let mut buffer = vec![0u8; length.min(1 << 20)];
                    getrandom::fill(&mut buffer)
                        .map_err(|_| mlua::Error::runtime("no random source is available"))?;
                    lua.create_string(buffer)
                })?,
            )?;
            Value::Table(module)
        }
        "ngx.semaphore" => Value::Table(super::semaphore::module(lua, slot)?),
        "resty.lock" => Value::Table(super::lock::module(lua, ngx, slot)?),
        "resty.aes" => Value::Table(super::aes::module(lua)?),
        "resty.dns.resolver" => Value::Table(super::dns::module(lua, slot)?),
        "ngx.upstream" => Value::Table(super::upstream::module(lua, slot)?),
        "resty.md5" => hasher::<Md5>(lua)?,
        "resty.sha1" => hasher::<Sha1>(lua)?,
        "resty.sha224" => hasher::<Sha224>(lua)?,
        "resty.sha256" => hasher::<Sha256>(lua)?,
        "resty.sha384" => hasher::<Sha384>(lua)?,
        "resty.sha512" => hasher::<Sha512>(lua)?,
        "ngx.re" => ngx.raw_get("re")?,
        "ngx.errlog" => Value::Table(super::errlog::module(lua, slot)?),
        "ngx.ssl" => Value::Table(super::ssl::module(lua, slot)?),
        "ngx.ssl.clienthello" => Value::Table(super::ssl::client_hello(lua, slot)?),
        "ngx.ssl.session" => Value::Table(super::ssl::session(lua, slot)?),
        "ngx.ocsp" => Value::Table(super::ssl::ocsp(lua, slot)?),
        "ngx.ssl.proxysslcert" => Value::Table(super::ssl::proxy_certificate(lua, slot)?),
        "ngx.ssl.proxysslverify" => Value::Table(super::ssl::proxy_verify(lua, slot)?),
        "ngx.proxyssl" => Value::Table(super::ssl::proxy_tls(lua, slot)?),
        "ngx.resp" | "ngx.req" => {
            let module = lua.create_table()?;
            let table: Table = ngx.raw_get(&name[4..])?;
            module.raw_set("add_header", table.raw_get::<Value>("add_header")?)?;
            Value::Table(module)
        }
        "ngx.process" => {
            let module = lua.create_table()?;
            module.raw_set("type", lua.create_function(|_, ()| Ok("worker"))?)?;
            module.raw_set(
                "get_master_pid",
                lua.create_function(|_, ()| Ok(std::process::id()))?,
            )?;
            module.raw_set(
                "enable_privileged_agent",
                lua.create_function(|lua, _: MultiValue| {
                    failed(
                        lua,
                        1,
                        "privileged agent processes are not available: they would run scripts with the gateway's own privileges, outside the sandbox",
                    )
                })?,
            )?;
            module.raw_set(
                "signal_graceful_exit",
                lua.create_function(|lua, ()| {
                    failed(lua, 1, "scripts cannot stop the gateway's workers")
                })?,
            )?;
            Value::Table(module)
        }
        "ngx.base64" => {
            let module = lua.create_table()?;
            module.raw_set(
                "encode_base64url",
                lua.create_function(|_, input: LuaString| Ok(encode_base64url(&input.as_bytes())))?,
            )?;
            module.raw_set(
                "decode_base64url",
                lua.create_function(|lua, input: LuaString| {
                    match decode_base64url(&input.as_bytes()) {
                        Some(decoded) => Ok(results([Value::String(lua.create_string(decoded)?)])),
                        None => failed(lua, 1, "invalid input"),
                    }
                })?,
            )?;
            Value::Table(module)
        }
        "ngx.balancer" => Value::Table(balancer(lua, slot)?),
        _ => return Ok(None),
    }))
}

/// The digest objects of lua-resty-string: `new`, `update`, `final`,
/// `reset`.
struct Hasher<D: Digest>(Mutex<D>);

impl<D: Digest + Clone + Send + 'static> UserData for Hasher<D> {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("update", |_, this, data: LuaString| {
            this.0.lock().update(&*data.as_bytes());
            Ok(true)
        });
        methods.add_method("final", |lua, this, ()| {
            let digest = this.0.lock().clone().finalize();
            lua.create_string(digest.as_slice())
        });
        methods.add_method("reset", |_, this, ()| {
            *this.0.lock() = D::new();
            Ok(true)
        });
    }
}

fn hasher<D: Digest + Clone + Send + 'static>(lua: &Lua) -> mlua::Result<Value> {
    let module = lua.create_table()?;
    module.raw_set(
        "new",
        lua.create_function(|lua, _: Value| lua.create_userdata(Hasher(Mutex::new(D::new()))))?,
    )?;
    Ok(Value::Table(module))
}

/// `ngx.balancer` of lua-resty-core, for `balancer_by_lua`.
fn balancer(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    let s = Arc::clone(slot);
    module.raw_set(
        "set_current_peer",
        lua.create_function(move |lua, (host, port): (LuaString, Option<u16>)| {
            let cell = cell(&s, Api::Balancer)?;
            require_permission(&cell, Api::Balancer, |granted| granted.upstream, "upstream")?;
            let host = host.to_str()?.trim().to_owned();
            let (host, port) = match port {
                Some(port) => (host, port),
                None => match host.rsplit_once(':') {
                    Some((name, port)) if port.parse::<u16>().is_ok() => {
                        (name.to_owned(), port.parse().unwrap_or(80))
                    }
                    _ => return failed(lua, 1, "no port in the peer address"),
                },
            };
            if host.is_empty() || port == 0 {
                return failed(lua, 1, "invalid peer address");
            }
            let mut exchange = cell.exchange.lock();
            exchange.balancer.peer = Some(Peer {
                host: host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_owned(),
                port,
            });
            exchange.changes.peer = true;
            Ok(results([Value::Boolean(true)]))
        })?,
    )?;
    let s = Arc::clone(slot);
    module.raw_set(
        "set_more_tries",
        lua.create_function(move |_, count: u32| {
            let cell = cell(&s, Api::Balancer)?;
            require_permission(&cell, Api::Balancer, |granted| granted.upstream, "upstream")?;
            let mut exchange = cell.exchange.lock();
            exchange.balancer.more_tries = Some(count);
            exchange.changes.peer = true;
            Ok(true)
        })?,
    )?;
    let s = Arc::clone(slot);
    module.raw_set(
        "get_last_failure",
        lua.create_function(move |_, ()| {
            exchange(&s, Api::Balancer, |exchange| {
                Ok(match &exchange.balancer.last_failure {
                    Some((state, status)) => (Some(state.clone()), *status),
                    None => (None, None::<u16>),
                })
            })
        })?,
    )?;
    let s = Arc::clone(slot);
    module.raw_set(
        "set_timeouts",
        lua.create_function(
            move |_, (connect, send, read): (Option<f64>, Option<f64>, Option<f64>)| {
                let cell = cell(&s, Api::Balancer)?;
                require_permission(&cell, Api::Balancer, |granted| granted.upstream, "upstream")?;
                let seconds = |value: Option<f64>| -> mlua::Result<Option<Duration>> {
                    match value {
                        None => Ok(None),
                        Some(value) if value > 0.0 && value.is_finite() => {
                            Ok(Some(Duration::from_secs_f64(value)))
                        }
                        Some(_) => Err(refused("timeouts must be positive seconds")),
                    }
                };
                let timeouts = crate::exchange::PeerTimeouts {
                    connect: seconds(connect)?,
                    send: seconds(send)?,
                    read: seconds(read)?,
                };
                let mut exchange = cell.exchange.lock();
                exchange.balancer.timeouts = timeouts;
                exchange.changes.peer = true;
                Ok(true)
            },
        )?,
    )?;
    Ok(module)
}
