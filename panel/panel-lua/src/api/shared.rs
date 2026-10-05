//! `ngx.shared.DICT`: the dictionaries the configuration declares, with
//! lua-nginx-module's methods and return values.

use super::{failed, results, Context};
use crate::shared::{Dict, Scalar, SetMode};
use mlua::{Lua, Table, UserData, UserDataMethods, Value};
use std::{sync::Arc, time::Duration};

struct Handle(Arc<Dict>);

fn key(key: &Value) -> mlua::Result<Vec<u8>> {
    let key = match key {
        Value::String(text) => text.as_bytes().to_vec(),
        Value::Integer(_) | Value::Number(_) => super::bytes(key).unwrap_or_default(),
        Value::Nil => return Err(mlua::Error::runtime("nil key")),
        other => {
            return Err(mlua::Error::runtime(format!(
                "bad key type {}",
                other.type_name()
            )));
        }
    };
    if key.is_empty() {
        return Err(mlua::Error::runtime("empty key"));
    }
    if key.len() > 65535 {
        return Err(mlua::Error::runtime("key too long"));
    }
    Ok(key)
}

fn scalar(value: &Value) -> mlua::Result<Option<Scalar>> {
    Ok(match value {
        Value::Nil => None,
        Value::Boolean(value) => Some(Scalar::Boolean(*value)),
        Value::Integer(value) => Some(Scalar::Number(*value as f64)),
        Value::Number(value) => Some(Scalar::Number(*value)),
        Value::String(text) => Some(Scalar::String(text.as_bytes().to_vec())),
        other => {
            return Err(mlua::Error::runtime(format!(
                "bad value type {}",
                other.type_name()
            )));
        }
    })
}

fn lua_scalar(lua: &Lua, value: Scalar) -> mlua::Result<Value> {
    Ok(match value {
        Scalar::Boolean(value) => Value::Boolean(value),
        Scalar::Number(value) => Value::Number(value),
        Scalar::String(bytes) => Value::String(lua.create_string(bytes)?),
    })
}

fn ttl(seconds: Option<f64>) -> Option<Duration> {
    seconds
        .filter(|seconds| *seconds > 0.0 && seconds.is_finite())
        .map(Duration::from_secs_f64)
}

fn store(
    dict: &Dict,
    (key_value, value, exptime, flags): (Value, Value, Option<f64>, Option<u32>),
    mode: SetMode,
) -> mlua::Result<(bool, Option<&'static str>, bool)> {
    let key = key(&key_value)?;
    let value = scalar(&value)?;
    Ok(
        match dict.set(&key, value, ttl(exptime), flags.unwrap_or(0), mode) {
            Ok(forcible) => (true, None, forcible),
            Err(refusal) => (false, Some(refusal.message()), false),
        },
    )
}

impl UserData for Handle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get", |lua, this, key_value: Value| {
            let key = key(&key_value)?;
            match this.0.get(&key) {
                Ok(Some(got)) => {
                    let mut values = vec![lua_scalar(lua, got.value)?];
                    if got.flags != 0 {
                        values.push(Value::Integer(got.flags.into()));
                    }
                    Ok(results(values))
                }
                Ok(None) => Ok(results([Value::Nil])),
                Err(refusal) => failed(lua, 1, refusal.message()),
            }
        });
        methods.add_method("get_stale", |lua, this, key_value: Value| {
            let key = key(&key_value)?;
            Ok(match this.0.get_stale(&key) {
                Ok(Some(got)) => (
                    lua_scalar(lua, got.value)?,
                    (got.flags != 0).then_some(got.flags),
                    Value::Boolean(got.stale),
                ),
                Ok(None) => return Ok(results([Value::Nil])),
                Err(refusal) => return failed(lua, 1, refusal.message()),
            })
            .map(|(value, flags, stale)| {
                results([
                    value,
                    flags.map_or(Value::Nil, |flags| Value::Integer(flags.into())),
                    stale,
                ])
            })
        });
        for (name, mode) in [
            ("set", SetMode::Set),
            ("safe_set", SetMode::SafeSet),
            ("add", SetMode::Add),
            ("safe_add", SetMode::SafeAdd),
            ("replace", SetMode::Replace),
        ] {
            methods.add_method(name, move |_, this, args| store(&this.0, args, mode));
        }
        methods.add_method("delete", |_, this, key_value: Value| {
            this.0.delete(&key(&key_value)?);
            Ok((true, None::<&str>, false))
        });
        methods.add_method(
            "incr",
            |lua, this, (key_value, by, init, init_ttl): (Value, f64, Option<f64>, Option<f64>)| {
                let key = key(&key_value)?;
                match this.0.incr(&key, by, init, ttl(init_ttl)) {
                    Ok((value, _)) if init.is_none() => Ok(results([Value::Number(value)])),
                    Ok((value, forcible)) => Ok(results([
                        Value::Number(value),
                        Value::Nil,
                        Value::Boolean(forcible),
                    ])),
                    Err(refusal) => failed(lua, 1, refusal.message()),
                }
            },
        );
        for (name, head) in [("lpush", true), ("rpush", false)] {
            methods.add_method(
                name,
                move |lua, this, (key_value, value): (Value, Value)| {
                    let key = key(&key_value)?;
                    let value = match scalar(&value)? {
                        Some(value @ (Scalar::Number(_) | Scalar::String(_))) => value,
                        _ => return Err(mlua::Error::runtime("bad value type")),
                    };
                    match this.0.push(&key, value, head) {
                        Ok(length) => Ok(results([Value::Integer(length as i64)])),
                        Err(refusal) => failed(lua, 1, refusal.message()),
                    }
                },
            );
        }
        for (name, head) in [("lpop", true), ("rpop", false)] {
            methods.add_method(name, move |lua, this, key_value: Value| {
                let key = key(&key_value)?;
                match this.0.pop(&key, head) {
                    Ok(Some(value)) => Ok(results([lua_scalar(lua, value)?])),
                    Ok(None) => Ok(results([Value::Nil])),
                    Err(refusal) => failed(lua, 1, refusal.message()),
                }
            });
        }
        methods.add_method("llen", |lua, this, key_value: Value| {
            match this.0.len(&key(&key_value)?) {
                Ok(length) => Ok(results([Value::Integer(length as i64)])),
                Err(refusal) => failed(lua, 1, refusal.message()),
            }
        });
        methods.add_method("ttl", |lua, this, key_value: Value| {
            match this.0.ttl(&key(&key_value)?) {
                Ok(left) => Ok(results([Value::Number(left.as_millis() as f64 / 1000.0)])),
                Err(refusal) => failed(lua, 1, refusal.message()),
            }
        });
        methods.add_method(
            "expire",
            |lua, this, (key_value, exptime): (Value, f64)| match this
                .0
                .expire(&key(&key_value)?, ttl(Some(exptime)))
            {
                Ok(()) => Ok(results([Value::Boolean(true)])),
                Err(refusal) => failed(lua, 1, refusal.message()),
            },
        );
        methods.add_method("flush_all", |_, this, ()| {
            this.0.flush_all();
            Ok(())
        });
        methods.add_method("flush_expired", |_, this, max: Option<usize>| {
            Ok(this.0.flush_expired(max.unwrap_or(0)))
        });
        methods.add_method("get_keys", |lua, this, max: Option<usize>| {
            let keys = this.0.keys(max.unwrap_or(1024));
            let table = lua.create_table_with_capacity(keys.len(), 0)?;
            for key in keys {
                table.raw_push(lua.create_string(key)?)?;
            }
            Ok(table)
        });
        methods.add_method("capacity", |_, this, ()| Ok(this.0.capacity()));
        methods.add_method("free_space", |_, this, ()| Ok(this.0.free_space()));
    }
}

pub(super) fn table(lua: &Lua, context: &Context) -> mlua::Result<Table> {
    let shared = lua.create_table()?;
    for (name, dict) in &context.dicts {
        shared.raw_set(
            name.as_str(),
            lua.create_userdata(Handle(Arc::clone(dict)))?,
        )?;
    }
    Ok(shared)
}
