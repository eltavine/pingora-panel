//! `ngx.ssl` and `ngx.ssl.clienthello`: what scripts read of the TLS
//! handshake they run in, and the certificate they present instead of the
//! gateway's.

use super::{failed, results};
use crate::{
    exchange::{Exchange, Phase},
    ssl::{grease, version_name, SUPPORTED_VERSIONS},
    tls::der,
    vm::Slot,
};
use bytes::Bytes;
use mlua::{
    FromLuaMulti, Lua, LuaString, MultiValue, Table, UserData, UserDataRef, Value, Variadic,
};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

/// `status_request` (RFC 6066 §8): the client takes a stapled OCSP
/// response.
const STATUS_REQUEST: u16 = 5;

const NO_PROTOCOLS: &str = "protocol versions come from TLS profiles: the gateway's TLS offers \
                            a listener's versions to every connection";
const NO_SECRETS: &str =
    "scripts are not given the connection's secrets: they would let them decrypt it";
const NO_HANDLES: &str =
    "scripts are not given TLS handles: the gateway's TLS is not OpenSSL, and scripts have no FFI";
const NOT_KEPT: &str = "the gateway's TLS does not keep this for scripts";

/// A certificate chain `parse_pem_cert` or `parse_der_cert` read.
struct Chain(Vec<Vec<u8>>);

impl UserData for Chain {}

/// A private key `parse_pem_priv_key` or `parse_der_priv_key` read.
struct Key(Vec<u8>);

impl UserData for Key {}

/// Runs `action` on the exchange whose code runs, in `only` if given.
fn with<R>(
    slot: &Slot,
    only: Option<Phase>,
    action: impl FnOnce(&mut Exchange) -> R,
) -> mlua::Result<R> {
    let Some(cell) = slot.cell() else {
        return Err(mlua::Error::runtime("no request found"));
    };
    let mut exchange = cell.exchange.lock();
    if only.is_some_and(|phase| phase != exchange.phase) {
        return Err(mlua::Error::runtime("API disabled in the current context"));
    }
    Ok(action(&mut exchange))
}

fn define<A, F>(
    lua: &Lua,
    table: &Table,
    slot: &Arc<Slot>,
    name: &str,
    function: F,
) -> mlua::Result<()>
where
    A: FromLuaMulti,
    F: Fn(&Lua, &Slot, A) -> mlua::Result<MultiValue> + Send + 'static,
{
    let slot = Arc::clone(slot);
    table.raw_set(
        name,
        lua.create_function(move |lua, args: A| function(lua, &slot, args))?,
    )
}

fn done() -> MultiValue {
    results([Value::Boolean(true)])
}

fn text(lua: &Lua, bytes: impl AsRef<[u8]>) -> mlua::Result<Value> {
    Ok(Value::String(lua.create_string(bytes)?))
}

/// The certificates of a DER chain, one after the other.
fn der_chain(mut rest: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut certificates = Vec::new();
    while !rest.is_empty() {
        let (tag, _, after) = der(rest)?;
        if tag != 0x30 {
            return None;
        }
        certificates.push(rest[..rest.len() - after.len()].to_vec());
        rest = after;
    }
    (!certificates.is_empty()).then_some(certificates)
}

fn pem_chain(pem: &[u8]) -> Option<Vec<Vec<u8>>> {
    let certificates = CertificateDer::pem_slice_iter(pem)
        .map(|certificate| certificate.map(|certificate| certificate.to_vec()))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!certificates.is_empty()).then_some(certificates)
}

fn pem_key(pem: &[u8]) -> Option<Vec<u8>> {
    PrivateKeyDer::from_pem_slice(pem)
        .ok()
        .map(|key| key.secret_der().to_vec())
}

fn der_key(der: &[u8]) -> Option<Vec<u8>> {
    PrivateKeyDer::try_from(der)
        .ok()
        .map(|key| key.secret_der().to_vec())
}

fn address(lua: &Lua, address: Option<SocketAddr>) -> mlua::Result<MultiValue> {
    match address {
        Some(SocketAddr::V4(address)) => Ok(results([
            text(lua, address.ip().octets())?,
            text(lua, "inet")?,
        ])),
        Some(SocketAddr::V6(address)) => Ok(results([
            text(lua, address.ip().octets())?,
            text(lua, "inet6")?,
        ])),
        None => failed(lua, 2, "the address is not known"),
    }
}

/// The functions that set the certificate a handshake of `phase` presents:
/// `clear_certs`, `set_der_cert`, `set_der_priv_key`, `set_cert` and
/// `set_priv_key`.
fn certificate_setters(
    lua: &Lua,
    module: &Table,
    slot: &Arc<Slot>,
    phase: Phase,
) -> mlua::Result<()> {
    define(lua, module, slot, "clear_certs", move |_, slot, ()| {
        with(slot, Some(phase), |exchange| {
            let handshake = &mut exchange.handshake;
            handshake.cleared = true;
            handshake.chain = None;
            handshake.key = None;
        })?;
        Ok(done())
    })?;
    define(
        lua,
        module,
        slot,
        "set_der_cert",
        move |lua, slot, der: mlua::LuaString| {
            let set = with(slot, Some(phase), |exchange| {
                der_chain(&der.as_bytes()).map(|chain| exchange.handshake.chain = Some(chain))
            })?;
            set.map_or_else(
                || failed(lua, 1, "the DER certificate chain cannot be read"),
                |()| Ok(done()),
            )
        },
    )?;
    define(
        lua,
        module,
        slot,
        "set_der_priv_key",
        move |lua, slot, der: mlua::LuaString| {
            let set = with(slot, Some(phase), |exchange| {
                der_key(&der.as_bytes()).map(|key| exchange.handshake.key = Some(key))
            })?;
            set.map_or_else(
                || failed(lua, 1, "the DER private key cannot be read"),
                |()| Ok(done()),
            )
        },
    )?;
    define(
        lua,
        module,
        slot,
        "set_cert",
        move |_, slot, chain: UserDataRef<Chain>| {
            with(slot, Some(phase), |exchange| {
                exchange.handshake.chain = Some(chain.0.clone());
            })?;
            Ok(done())
        },
    )?;
    define(
        lua,
        module,
        slot,
        "set_priv_key",
        move |_, slot, key: UserDataRef<Key>| {
            with(slot, Some(phase), |exchange| {
                exchange.handshake.key = Some(key.0.clone());
            })?;
            Ok(done())
        },
    )?;
    Ok(())
}

/// The `ngx.ssl.proxysslcert` module.
pub(super) fn proxy_certificate(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    certificate_setters(lua, &module, slot, Phase::ProxySslCertificate)?;
    Ok(module)
}

/// The `ngx.ssl.proxysslverify` module.
pub(super) fn proxy_verify(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    let verify = Some(Phase::ProxySslVerify);
    define(
        lua,
        &module,
        slot,
        "get_verify_result",
        move |_, slot, ()| {
            let result = with(slot, verify, |exchange| exchange.upstream_tls.verify_result)?;
            Ok(results([Value::Integer(result)]))
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "get_verify_cert",
        move |lua, slot, ()| {
            let chain = with(slot, verify, |exchange| {
                exchange.upstream_tls.chain.concat()
            })?;
            if chain.is_empty() {
                return failed(
                lua,
                1,
                "the upstream's certificate is not known: this Pingora keeps none of its TLS connections' certificates",
            );
            }
            Ok(results([text(lua, chain)?]))
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "set_verify_result",
        move |_, slot, code: Option<i64>| {
            with(slot, verify, |exchange| {
                exchange.upstream_tls.verdict = Some(code.unwrap_or(0));
            })?;
            Ok(done())
        },
    )?;
    Ok(module)
}

/// The `ngx.proxyssl` module: the version of a request's TLS connection to
/// its upstream.
pub(super) fn proxy_tls(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    for (name, version) in [
        ("SSL3_VERSION", 0x0300),
        ("TLS1_VERSION", 0x0301),
        ("TLS1_1_VERSION", 0x0302),
        ("TLS1_2_VERSION", 0x0303),
        ("TLS1_3_VERSION", 0x0304),
    ] {
        module.raw_set(name, version)?;
    }
    define(
        lua,
        &module,
        slot,
        "get_tls1_version",
        |lua, slot, ()| match with(slot, None, |exchange| exchange.upstream_tls.version)? {
            Some(version) => Ok(results([Value::Integer(i64::from(version))])),
            None => failed(
                lua,
                1,
                "the request has no TLS connection to its upstream yet",
            ),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "get_tls1_version_str",
        |lua, slot, ()| match with(slot, None, |exchange| exchange.upstream_tls.version)?
            .and_then(version_name)
        {
            Some(name) => Ok(results([text(lua, name)?])),
            None => failed(
                lua,
                1,
                "the request has no TLS connection to its upstream yet",
            ),
        },
    )?;
    Ok(module)
}

/// The `ngx.ssl` module.
pub(super) fn module(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    certificate_setters(lua, &module, slot, Phase::SslCertificate)?;
    define(
        lua,
        &module,
        slot,
        "verify_client",
        |lua,
         slot,
         (client, depth, trusted): (
            Option<UserDataRef<Chain>>,
            Option<i64>,
            Option<UserDataRef<Chain>>,
        )| {
            with(slot, Some(Phase::SslCertificate), |_| ())?;
            let authorities: Vec<Vec<u8>> = [&client, &trusted]
                .into_iter()
                .flatten()
                .flat_map(|chain| chain.0.iter().cloned())
                .collect();
            if authorities.is_empty() {
                return failed(
                    lua,
                    1,
                    "no certificate is trusted to issue clients': give client_certs or trusted_certs",
                );
            }
            let depth = usize::try_from(depth.unwrap_or(1)).map_err(|_| {
                mlua::Error::runtime(
                    "bad argument #2 to 'verify_client' (depth must not be negative)",
                )
            })?;
            with(slot, Some(Phase::SslCertificate), |exchange| {
                exchange.handshake.client_auth =
                    Some(crate::ssl::ClientAuth { authorities, depth });
            })?;
            Ok(done())
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "cert_pem_to_der",
        |lua, _, pem: mlua::LuaString| match pem_chain(&pem.as_bytes()) {
            Some(chain) => Ok(results([text(lua, chain.concat())?])),
            None => failed(lua, 1, "the PEM certificate chain cannot be read"),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "priv_key_pem_to_der",
        |lua, _, (pem, _): (mlua::LuaString, Option<mlua::LuaString>)| match pem_key(
            &pem.as_bytes(),
        ) {
            Some(key) => Ok(results([text(lua, key)?])),
            None => failed(lua, 1, "the PEM private key cannot be read"),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "parse_pem_cert",
        |lua, _, pem: mlua::LuaString| match pem_chain(&pem.as_bytes()) {
            Some(chain) => Ok(results([Value::UserData(
                lua.create_userdata(Chain(chain))?,
            )])),
            None => failed(lua, 1, "the PEM certificate chain cannot be read"),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "parse_der_cert",
        |lua, _, der: mlua::LuaString| match der_chain(&der.as_bytes()) {
            Some(chain) => Ok(results([Value::UserData(
                lua.create_userdata(Chain(chain))?,
            )])),
            None => failed(lua, 1, "the DER certificate chain cannot be read"),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "parse_pem_priv_key",
        |lua, _, pem: mlua::LuaString| match pem_key(&pem.as_bytes()) {
            Some(key) => Ok(results([Value::UserData(lua.create_userdata(Key(key))?)])),
            None => failed(lua, 1, "the PEM private key cannot be read"),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "parse_der_priv_key",
        |lua, _, der: mlua::LuaString| match der_key(&der.as_bytes()) {
            Some(key) => Ok(results([Value::UserData(lua.create_userdata(Key(key))?)])),
            None => failed(lua, 1, "the DER private key cannot be read"),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "server_name",
        |lua, slot, ()| match with(slot, None, |exchange| {
            exchange.handshake.server_name.clone()
        })? {
            Some(name) => Ok(results([text(lua, name)?])),
            None => Ok(results([Value::Nil])),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "server_port",
        |lua, slot, ()| match with(slot, None, |exchange| {
            exchange.connection.server.map(|server| server.port())
        })? {
            Some(port) => Ok(results([Value::Integer(i64::from(port))])),
            None => failed(lua, 1, "the server port is not known"),
        },
    )?;
    define(lua, &module, slot, "raw_server_addr", |lua, slot, ()| {
        address(
            lua,
            with(slot, None, |exchange| exchange.connection.server)?,
        )
    })?;
    define(lua, &module, slot, "raw_client_addr", |lua, slot, ()| {
        address(
            lua,
            with(slot, None, |exchange| exchange.connection.client)?,
        )
    })?;
    define(
        lua,
        &module,
        slot,
        "get_tls1_version",
        |lua, slot, ()| match with(slot, None, |exchange| exchange.handshake.version)? {
            Some(version) => Ok(results([Value::Integer(i64::from(version))])),
            None => failed(lua, 1, "the TLS version is not known"),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "get_tls1_version_str",
        |lua, slot, ()| match with(slot, None, |exchange| exchange.handshake.version)?
            .and_then(version_name)
        {
            Some(name) => Ok(results([text(lua, name)?])),
            None => failed(lua, 1, "unknown version"),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "get_client_random",
        |lua, slot, wanted: Option<usize>| {
            let random = with(slot, None, |exchange| exchange.handshake.random.clone())?;
            match wanted.unwrap_or(random.len()) {
                0 => Ok(results([Value::Integer(random.len() as i64)])),
                wanted => Ok(results([text(lua, &random[..wanted.min(random.len())])?])),
            }
        },
    )?;
    for (name, reason) in [
        ("get_session_master_key", NO_SECRETS),
        ("get_req_ssl_pointer", NO_HANDLES),
        ("get_upstream_ssl_pointer", NO_HANDLES),
        ("ssl_session_reused", NO_HANDLES),
        ("export_keying_material", NOT_KEPT),
        ("get_server_random", NOT_KEPT),
        ("get_req_shared_ssl_ciphers", NOT_KEPT),
    ] {
        define(
            lua,
            &module,
            slot,
            name,
            move |lua, _, _: Variadic<Value>| failed(lua, 1, reason),
        )?;
    }
    Ok(module)
}

/// The `ngx.ssl.session` module.
pub(super) fn session(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    define(lua, &module, slot, "get_session_id", |lua, slot, ()| {
        let id = with(slot, None, |exchange| {
            matches!(
                exchange.phase,
                Phase::SslSessionFetch | Phase::SslSessionStore
            )
            .then(|| exchange.handshake.session.clone())
        })?;
        match id {
            None => Err(mlua::Error::runtime("API disabled in the current context")),
            Some(None) => failed(lua, 1, "no session ID"),
            Some(Some(id)) => {
                let hex: String = id.iter().map(|byte| format!("{byte:02x}")).collect();
                Ok(results([text(lua, hex)?]))
            }
        }
    })?;
    define(
        lua,
        &module,
        slot,
        "get_serialized_session",
        |lua, slot, ()| match with(slot, Some(Phase::SslSessionStore), |exchange| {
            exchange.handshake.serialized.clone()
        })? {
            Some(session) => Ok(results([text(lua, session)?])),
            None => failed(lua, 1, "no session"),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "set_serialized_session",
        |lua, slot, session: LuaString| {
            if session.as_bytes().is_empty() {
                return failed(lua, 1, "the serialized session is empty");
            }
            with(slot, Some(Phase::SslSessionFetch), |exchange| {
                exchange.handshake.serialized = Some(Bytes::copy_from_slice(&session.as_bytes()));
            })?;
            Ok(done())
        },
    )?;
    Ok(module)
}

/// What `max_len` allows of `value`.
fn within_length(lua: &Lua, value: &[u8], most: Option<usize>) -> mlua::Result<MultiValue> {
    match most {
        Some(most) if value.len() > most => failed(lua, 1, "the result is longer than max_len"),
        _ => Ok(results([text(lua, value)?])),
    }
}

/// The `ngx.ocsp` module.
pub(super) fn ocsp(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    define(
        lua,
        &module,
        slot,
        "get_ocsp_responder_from_der_chain",
        |lua, _, (chain, most): (LuaString, Option<usize>)| {
            let Some(chain) = der_chain(&chain.as_bytes()) else {
                return failed(lua, 1, "the DER certificate chain cannot be read");
            };
            match crate::ocsp::responder(&chain) {
                Ok(url) => within_length(lua, url.as_bytes(), most),
                Err(error) => failed(lua, 1, &error),
            }
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "create_ocsp_request",
        |lua, _, (chain, most): (LuaString, Option<usize>)| {
            let Some(chain) = der_chain(&chain.as_bytes()) else {
                return failed(lua, 1, "the DER certificate chain cannot be read");
            };
            match crate::ocsp::request(&chain) {
                Ok(request) => within_length(lua, &request, most),
                Err(error) => failed(lua, 1, &error),
            }
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "validate_ocsp_response",
        |lua, _, (response, chain, _): (LuaString, LuaString, Option<usize>)| {
            let Some(chain) = der_chain(&chain.as_bytes()) else {
                return failed(lua, 1, "the DER certificate chain cannot be read");
            };
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| {
                    i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
                });
            match crate::ocsp::validate(&response.as_bytes(), &chain, now) {
                Ok(()) => Ok(done()),
                Err(error) => failed(lua, 1, &error),
            }
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "set_ocsp_status_resp",
        |lua, slot, response: LuaString| {
            let asked = with(slot, Some(Phase::SslCertificate), |exchange| {
                exchange.handshake.ocsp = Some(Bytes::copy_from_slice(&response.as_bytes()));
                exchange
                    .handshake
                    .extensions
                    .iter()
                    .any(|(kind, _)| *kind == STATUS_REQUEST)
            })?;
            if asked {
                Ok(done())
            } else {
                Ok(results([Value::Boolean(true), text(lua, "no status req")?]))
            }
        },
    )?;
    Ok(module)
}

/// The `ngx.ssl.clienthello` module.
pub(super) fn client_hello(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    let hello = Some(Phase::SslClientHello);
    define(
        lua,
        &module,
        slot,
        "get_client_hello_server_name",
        move |lua, slot, ()| match with(slot, hello, |exchange| {
            exchange.handshake.server_name.clone()
        })? {
            Some(name) => Ok(results([text(lua, name)?])),
            None => Ok(results([Value::Nil])),
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "get_supported_versions",
        move |lua, slot, ()| {
            let versions = with(slot, hello, |exchange| {
                let handshake = &exchange.handshake;
                handshake
                    .extensions
                    .iter()
                    .any(|(kind, _)| *kind == SUPPORTED_VERSIONS)
                    .then(|| handshake.versions.clone())
            })?;
            let Some(versions) = versions else {
                return Ok(results([Value::Nil]));
            };
            let names = lua.create_sequence_from(versions.into_iter().filter_map(version_name))?;
            Ok(results([Value::Table(names)]))
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "get_client_hello_ciphers",
        move |lua, slot, ()| {
            let ciphers = with(slot, hello, |exchange| exchange.handshake.ciphers.clone())?;
            let ciphers = ciphers.into_iter().filter(|cipher| !grease(*cipher));
            Ok(results([Value::Table(lua.create_sequence_from(ciphers)?)]))
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "get_client_hello_ext_present",
        move |lua, slot, ()| {
            let kinds = with(slot, hello, |exchange| {
                exchange
                    .handshake
                    .extensions
                    .iter()
                    .map(|(kind, _)| *kind)
                    .filter(|kind| !grease(*kind))
                    .collect::<Vec<_>>()
            })?;
            Ok(results([Value::Table(lua.create_sequence_from(kinds)?)]))
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "get_client_hello_ext",
        move |lua, slot, wanted: u16| {
            let data = with(slot, hello, |exchange| {
                exchange
                    .handshake
                    .extensions
                    .iter()
                    .find(|(kind, _)| *kind == wanted)
                    .map(|(_, data)| data.clone())
            })?;
            match data {
                Some(data) => Ok(results([text(lua, data)?])),
                None => Ok(results([Value::Nil])),
            }
        },
    )?;
    define(
        lua,
        &module,
        slot,
        "set_protocols",
        move |lua, slot, _: Variadic<Value>| {
            with(slot, hello, |_| ())?;
            failed(lua, 1, NO_PROTOCOLS)
        },
    )?;
    Ok(module)
}
