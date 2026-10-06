//! `ngx.socket.udp`: datagram cosockets, granted with the TCP ones by the
//! network permission. `receive` reads one datagram of at most 8192 bytes,
//! as lua-nginx-module does.

use super::{
    results,
    socket::{allowed, defaults, failed, log_failure, milliseconds, payload, reason, within},
    Api,
};
use crate::vm::Slot;
use mlua::{Lua, Table, UserData, UserDataMethods, Value};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::net::UdpSocket as Socket;

const MOST_DATAGRAM: usize = 8192;

struct UdpSocket {
    slot: Arc<Slot>,
    socket: Option<Socket>,
    /// The address `bind` chose to send from.
    local: Option<IpAddr>,
    timeout: Duration,
}

fn one() -> mlua::MultiValue {
    results([Value::Integer(1)])
}

impl UserData for UdpSocket {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("bind", |lua, this, address: String| {
            allowed(&this.slot, Api::Udp)?;
            match address.parse() {
                Ok(address) => {
                    this.local = Some(address);
                    Ok(one())
                }
                Err(_) => failed(lua, "bad address"),
            }
        });
        methods.add_async_method_mut(
            "setpeername",
            |lua, mut this, (host, port): (String, Option<u16>)| async move {
                allowed(&this.slot, Api::Udp)?;
                this.socket = None;
                let Some(port) = port else {
                    return failed(&lua, "unix domain sockets are not available");
                };
                let resolved =
                    within(this.timeout, tokio::net::lookup_host((host.as_str(), port))).await;
                let peer = match resolved.map(|mut found| found.next()) {
                    Ok(Some(peer)) => peer,
                    Ok(None) => return failed(&lua, &format!("{host} could not be resolved")),
                    Err(error) => {
                        return failed(&lua, &format!("{host} could not be resolved ({error})"))
                    }
                };
                let local = this.local.unwrap_or(if peer.is_ipv4() {
                    Ipv4Addr::UNSPECIFIED.into()
                } else {
                    Ipv6Addr::UNSPECIFIED.into()
                });
                let socket = match Socket::bind(SocketAddr::new(local, 0)).await {
                    Ok(socket) => socket,
                    Err(error) => return failed(&lua, &reason(&error)),
                };
                if let Err(error) = socket.connect(peer).await {
                    return failed(&lua, &reason(&error));
                }
                this.socket = Some(socket);
                Ok(one())
            },
        );
        methods.add_async_method_mut("send", |lua, this, data: Value| async move {
            allowed(&this.slot, Api::Udp)?;
            let data = payload(data)?;
            let Some(socket) = &this.socket else {
                return failed(&lua, "closed");
            };
            match within(this.timeout, socket.send(&data)).await {
                Ok(_) => Ok(one()),
                Err(error) => {
                    log_failure(&this.slot, "udp", "send", &error);
                    failed(&lua, &error)
                }
            }
        });
        methods.add_async_method_mut("receive", |lua, this, size: Option<usize>| async move {
            allowed(&this.slot, Api::Udp)?;
            let Some(socket) = &this.socket else {
                return failed(&lua, "closed");
            };
            let size = size
                .filter(|size| *size > 0)
                .map_or(MOST_DATAGRAM, |size| size.min(MOST_DATAGRAM));
            let mut buffer = vec![0; size];
            match within(this.timeout, socket.recv(&mut buffer)).await {
                Ok(read) => Ok(results([Value::String(
                    lua.create_string(&buffer[..read])?,
                )])),
                Err(error) => {
                    log_failure(&this.slot, "udp", "receive", &error);
                    failed(&lua, &error)
                }
            }
        });
        methods.add_method_mut("settimeout", |_, this, ms: Option<f64>| {
            if let Some(limit) = milliseconds(ms) {
                this.timeout = limit;
            }
            Ok(())
        });
        methods.add_method_mut("close", |lua, this, ()| match this.socket.take() {
            Some(_) => Ok(one()),
            None => failed(lua, "closed"),
        });
    }
}

pub(super) fn install(lua: &Lua, ngx: &Table, slot: &Arc<Slot>) -> mlua::Result<()> {
    let socket: Table = ngx.raw_get("socket")?;
    let slot = Arc::clone(slot);
    socket.raw_set(
        "udp",
        lua.create_function(move |lua, ()| {
            lua.create_userdata(UdpSocket {
                slot: Arc::clone(&slot),
                socket: None,
                local: None,
                timeout: defaults(&slot).read_timeout,
            })
        })?,
    )?;
    // lua-nginx-module's alias of the TCP cosocket.
    socket.raw_set("stream", socket.raw_get::<Value>("tcp")?)?;
    Ok(())
}
