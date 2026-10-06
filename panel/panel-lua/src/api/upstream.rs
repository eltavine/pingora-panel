//! `ngx.upstream`: lua-upstream-nginx-module's view of the configuration's
//! upstreams, and `set_peer_down`, which needs the upstream permission.

use super::{failed, results};
use crate::{
    upstreams::{UpstreamPeer, UpstreamServer, Upstreams},
    vm::{refused, Slot},
};
use mlua::{Lua, MultiValue, Table, Value};
use std::sync::Arc;

/// The upstreams of the runtime a VM belongs to.
pub(crate) struct Handle(pub Arc<dyn Upstreams>);

fn upstreams(lua: &Lua) -> Arc<dyn Upstreams> {
    lua.app_data_ref::<Handle>()
        .map(|handle| Arc::clone(&handle.0))
        .unwrap_or_else(|| Arc::new(crate::upstreams::NoUpstreams))
}

fn server(lua: &Lua, server: &UpstreamServer) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.raw_set("addr", server.addr.as_str())?;
    table.raw_set("name", server.name.as_str())?;
    table.raw_set("weight", server.weight)?;
    table.raw_set("max_fails", server.max_fails)?;
    table.raw_set("fail_timeout", server.fail_timeout)?;
    if server.backup {
        table.raw_set("backup", true)?;
    }
    if server.down {
        table.raw_set("down", true)?;
    }
    Ok(table)
}

fn peer(lua: &Lua, peer: &UpstreamPeer) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.raw_set("id", peer.id)?;
    table.raw_set("name", peer.name.as_str())?;
    table.raw_set("weight", peer.weight)?;
    table.raw_set("current_weight", 0)?;
    table.raw_set("effective_weight", peer.weight)?;
    table.raw_set("fails", peer.fails)?;
    table.raw_set("max_fails", peer.max_fails)?;
    table.raw_set("fail_timeout", peer.fail_timeout)?;
    table.raw_set("conns", peer.conns)?;
    if peer.down {
        table.raw_set("down", true)?;
    }
    Ok(table)
}

fn list<T>(
    lua: &Lua,
    items: Option<Vec<T>>,
    each: fn(&Lua, &T) -> mlua::Result<Table>,
) -> mlua::Result<MultiValue> {
    let Some(items) = items else {
        return failed(lua, 1, "upstream not found");
    };
    let table = lua.create_table_with_capacity(items.len(), 0)?;
    for item in &items {
        table.raw_push(each(lua, item)?)?;
    }
    Ok(results([Value::Table(table)]))
}

/// The `ngx.upstream` module.
pub(super) fn module(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    module.raw_set("_VERSION", "0.07")?;
    module.raw_set(
        "get_upstreams",
        lua.create_function(|lua, ()| lua.create_sequence_from(upstreams(lua).names()))?,
    )?;
    module.raw_set(
        "get_servers",
        lua.create_function(|lua, name: String| list(lua, upstreams(lua).servers(&name), server))?,
    )?;
    module.raw_set(
        "get_primary_peers",
        lua.create_function(|lua, name: String| {
            list(lua, upstreams(lua).peers(&name, false), peer)
        })?,
    )?;
    module.raw_set(
        "get_backup_peers",
        lua.create_function(|lua, name: String| {
            list(lua, upstreams(lua).peers(&name, true), peer)
        })?,
    )?;
    let setting = Arc::clone(slot);
    module.raw_set(
        "set_peer_down",
        lua.create_function(
            move |lua, (name, backup, id, down): (String, bool, usize, bool)| {
                let granted = setting
                    .cell()
                    .is_some_and(|cell| cell.run.lock().permissions.upstream);
                if !granted {
                    return Err(refused(
                        "ngx.upstream.set_peer_down needs the upstream permission (lua_allow upstream)",
                    ));
                }
                match upstreams(lua).set_peer_down(&name, backup, id, down) {
                    Ok(()) => Ok(results([Value::Boolean(true)])),
                    Err(error) => failed(lua, 1, &error),
                }
            },
        )?,
    )?;
    let current = Arc::clone(slot);
    module.raw_set(
        "current_upstream_name",
        lua.create_function(move |lua, ()| {
            let name = current
                .cell()
                .map(|cell| cell.exchange.lock().balancer.upstream.clone())
                .filter(|name| !name.is_empty());
            name.map_or(Ok(Value::Nil), |name| {
                lua.create_string(name).map(Value::String)
            })
        })?,
    )?;
    Ok(module)
}
