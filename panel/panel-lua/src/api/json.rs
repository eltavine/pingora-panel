//! `cjson` and `cjson.safe`, with lua-cjson's encoding rules and settings.

use super::number_text;
use mlua::{LightUserData, Lua, LuaString, Table, Value, Variadic};
use parking_lot::Mutex;
use std::{ffi::c_void, sync::Arc};

static EMPTY_ARRAY: u8 = 0;

fn empty_array() -> Value {
    Value::LightUserData(LightUserData(
        std::ptr::from_ref(&EMPTY_ARRAY).cast::<c_void>().cast_mut(),
    ))
}

#[derive(Clone, Copy, Debug)]
struct Settings {
    empty_table_as_object: bool,
    array_mt_on_decode: bool,
    escape_forward_slash: bool,
    sparse_convert: bool,
    sparse_ratio: usize,
    sparse_safe: usize,
    encode_depth: usize,
    decode_depth: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            empty_table_as_object: true,
            array_mt_on_decode: false,
            escape_forward_slash: true,
            sparse_convert: false,
            sparse_ratio: 2,
            sparse_safe: 10,
            encode_depth: 1000,
            decode_depth: 1000,
        }
    }
}

struct Encoder<'a> {
    settings: Settings,
    array_mt: &'a Table,
    empty_array_mt: &'a Table,
    output: Vec<u8>,
}

impl Encoder<'_> {
    fn string(&mut self, bytes: &[u8]) {
        self.output.push(b'"');
        for &byte in bytes {
            match byte {
                b'"' => self.output.extend_from_slice(b"\\\""),
                b'\\' => self.output.extend_from_slice(b"\\\\"),
                b'/' if self.settings.escape_forward_slash => self.output.extend_from_slice(b"\\/"),
                b'\n' => self.output.extend_from_slice(b"\\n"),
                b'\r' => self.output.extend_from_slice(b"\\r"),
                b'\t' => self.output.extend_from_slice(b"\\t"),
                8 => self.output.extend_from_slice(b"\\b"),
                12 => self.output.extend_from_slice(b"\\f"),
                0..=31 | 127 => self
                    .output
                    .extend_from_slice(format!("\\u{byte:04x}").as_bytes()),
                other => self.output.push(other),
            }
        }
        self.output.push(b'"');
    }

    fn number(&mut self, number: f64) -> Result<(), String> {
        if !number.is_finite() {
            return Err("Cannot serialise number: must not be NaN or Infinity".into());
        }
        self.output
            .extend_from_slice(number_text(number).as_bytes());
        Ok(())
    }

    /// The length of `table` as an array, if it is one.
    fn array_length(&self, table: &Table) -> Result<Option<usize>, String> {
        let mut max = 0usize;
        let mut count = 0usize;
        for pair in table.pairs::<Value, Value>() {
            let (key, _) = pair.map_err(|error| error.to_string())?;
            let index = match key {
                Value::Integer(index) if index >= 1 => index as usize,
                Value::Number(index) if index >= 1.0 && index.fract() == 0.0 => index as usize,
                _ => return Ok(None),
            };
            max = max.max(index);
            count += 1;
        }
        if max > self.settings.sparse_safe
            && self.settings.sparse_ratio > 0
            && max > count * self.settings.sparse_ratio
        {
            if self.settings.sparse_convert {
                return Ok(None);
            }
            return Err("Cannot serialise table: excessively sparse array".into());
        }
        Ok(Some(max))
    }

    fn table(&mut self, table: &Table, depth: usize) -> Result<(), String> {
        if depth > self.settings.encode_depth {
            return Err(format!("Cannot serialise, excessive nesting ({depth})"));
        }
        let meta = table.metatable();
        let marked_array = meta
            .as_ref()
            .is_some_and(|meta| meta == self.array_mt || meta == self.empty_array_mt);
        let length = match self.array_length(table)? {
            Some(0) if !marked_array && self.settings.empty_table_as_object => None,
            other => other,
        };
        match length {
            Some(length) => {
                self.output.push(b'[');
                for index in 1..=length {
                    if index > 1 {
                        self.output.push(b',');
                    }
                    let value: Value = table.raw_get(index).map_err(|error| error.to_string())?;
                    self.value(&value, depth + 1)?;
                }
                self.output.push(b']');
            }
            None => {
                self.output.push(b'{');
                let mut first = true;
                for pair in table.pairs::<Value, Value>() {
                    let (key, value) = pair.map_err(|error| error.to_string())?;
                    if !first {
                        self.output.push(b',');
                    }
                    first = false;
                    match &key {
                        Value::String(text) => self.string(&text.as_bytes()),
                        Value::Integer(_) | Value::Number(_) => {
                            let text = super::bytes(&key).unwrap_or_default();
                            self.string(&text);
                        }
                        _ => {
                            return Err(
                                "Cannot serialise table: table key must be a number or string"
                                    .into(),
                            );
                        }
                    }
                    self.output.push(b':');
                    self.value(&value, depth + 1)?;
                }
                self.output.push(b'}');
            }
        }
        Ok(())
    }

    fn value(&mut self, value: &Value, depth: usize) -> Result<(), String> {
        match value {
            Value::Nil => self.output.extend_from_slice(b"null"),
            Value::LightUserData(data) if data.0.is_null() => {
                self.output.extend_from_slice(b"null");
            }
            data @ Value::LightUserData(_) if *data == empty_array() => {
                self.output.extend_from_slice(b"[]");
            }
            Value::Boolean(value) => {
                self.output
                    .extend_from_slice(if *value { b"true" } else { b"false" })
            }
            Value::Integer(number) => self.number(*number as f64)?,
            Value::Number(number) => self.number(*number)?,
            Value::String(text) => self.string(&text.as_bytes()),
            Value::Table(table) => self.table(table, depth)?,
            other => {
                return Err(format!(
                    "Cannot serialise {}: type not supported",
                    other.type_name()
                ));
            }
        }
        Ok(())
    }
}

fn decoded(
    lua: &Lua,
    value: serde_json::Value,
    settings: Settings,
    array_mt: &Table,
    depth: usize,
) -> mlua::Result<Value> {
    if depth > settings.decode_depth {
        return Err(mlua::Error::runtime(format!(
            "Found too many nested data structures ({depth})"
        )));
    }
    Ok(match value {
        serde_json::Value::Null => Value::NULL,
        serde_json::Value::Bool(value) => Value::Boolean(value),
        serde_json::Value::Number(number) => Value::Number(number.as_f64().unwrap_or(f64::NAN)),
        serde_json::Value::String(text) => Value::String(lua.create_string(text)?),
        serde_json::Value::Array(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0)?;
            for item in items {
                table.raw_push(decoded(lua, item, settings, array_mt, depth + 1)?)?;
            }
            if settings.array_mt_on_decode {
                table.set_metatable(Some(array_mt.clone()))?;
            }
            Value::Table(table)
        }
        serde_json::Value::Object(fields) => {
            let table = lua.create_table_with_capacity(0, fields.len())?;
            for (key, value) in fields {
                table.raw_set(key, decoded(lua, value, settings, array_mt, depth + 1)?)?;
            }
            Value::Table(table)
        }
    })
}

fn flag(value: &Value) -> Option<bool> {
    match value {
        Value::Boolean(value) => Some(*value),
        Value::String(text) => match &*text.as_bytes() {
            b"on" | b"true" => Some(true),
            b"off" | b"false" => Some(false),
            _ => None,
        },
        Value::Integer(value) => Some(*value != 0),
        _ => None,
    }
}

/// Sets one of an instance's settings from its arguments and returns it.
type Setter = fn(&mut Settings, &[Value]) -> Option<Value>;

/// A `cjson` instance: `require "cjson"`, `cjson.safe` and `cjson.new()`
/// each have settings of their own.
pub(super) fn instance(lua: &Lua, safe: bool) -> mlua::Result<Table> {
    let settings = Arc::new(Mutex::new(Settings::default()));
    let module = lua.create_table()?;
    let array_mt = lua.create_table()?;
    let empty_array_mt = lua.create_table()?;
    module.raw_set("_NAME", if safe { "cjson.safe" } else { "cjson" })?;
    module.raw_set("_VERSION", "2.1.0.14")?;
    module.raw_set("null", Value::NULL)?;
    module.raw_set("empty_array", empty_array())?;
    module.raw_set("array_mt", array_mt.clone())?;
    module.raw_set("empty_array_mt", empty_array_mt.clone())?;

    let encode_settings = Arc::clone(&settings);
    let (encode_array_mt, encode_empty_mt) = (array_mt.clone(), empty_array_mt.clone());
    module.raw_set(
        "encode",
        lua.create_function(move |lua, value: Value| {
            let mut encoder = Encoder {
                settings: *encode_settings.lock(),
                array_mt: &encode_array_mt,
                empty_array_mt: &encode_empty_mt,
                output: Vec::new(),
            };
            match encoder.value(&value, 1) {
                Ok(()) => Ok(super::results([Value::String(
                    lua.create_string(encoder.output)?,
                )])),
                Err(error) if safe => super::failed(lua, 1, &error),
                Err(error) => Err(mlua::Error::runtime(error)),
            }
        })?,
    )?;
    let decode_settings = Arc::clone(&settings);
    let decode_array_mt = array_mt.clone();
    module.raw_set(
        "decode",
        lua.create_function(move |lua, text: LuaString| {
            let settings = *decode_settings.lock();
            let parsed = serde_json::from_slice::<serde_json::Value>(&text.as_bytes())
                .map_err(|error| format!("Expected value but found invalid token: {error}"))
                .and_then(|value| {
                    decoded(lua, value, settings, &decode_array_mt, 1)
                        .map_err(|error| error.to_string())
                });
            match parsed {
                Ok(value) => Ok(super::results([value])),
                Err(error) if safe => super::failed(lua, 1, &error),
                Err(error) => Err(mlua::Error::runtime(error)),
            }
        })?,
    )?;
    let setters: [(&str, Setter); 6] = [
        ("encode_empty_table_as_object", |settings, args| {
            if let Some(value) = args.first().and_then(flag) {
                settings.empty_table_as_object = value;
            }
            Some(Value::Boolean(settings.empty_table_as_object))
        }),
        ("decode_array_with_array_mt", |settings, args| {
            if let Some(value) = args.first().and_then(flag) {
                settings.array_mt_on_decode = value;
            }
            Some(Value::Boolean(settings.array_mt_on_decode))
        }),
        ("encode_escape_forward_slash", |settings, args| {
            if let Some(value) = args.first().and_then(flag) {
                settings.escape_forward_slash = value;
            }
            Some(Value::Boolean(settings.escape_forward_slash))
        }),
        ("encode_max_depth", |settings, args| {
            if let Some(Value::Integer(depth)) = args.first() {
                settings.encode_depth = (*depth).clamp(1, 1000) as usize;
            }
            Some(Value::Integer(settings.encode_depth as i64))
        }),
        ("decode_max_depth", |settings, args| {
            if let Some(Value::Integer(depth)) = args.first() {
                settings.decode_depth = (*depth).clamp(1, 1000) as usize;
            }
            Some(Value::Integer(settings.decode_depth as i64))
        }),
        ("encode_number_precision", |_, _| Some(Value::Integer(14))),
    ];
    for (name, setter) in setters {
        let settings = Arc::clone(&settings);
        module.raw_set(
            name,
            lua.create_function(move |_, args: Variadic<Value>| {
                Ok(setter(&mut settings.lock(), &args).unwrap_or(Value::Nil))
            })?,
        )?;
    }
    let sparse = Arc::clone(&settings);
    module.raw_set(
        "encode_sparse_array",
        lua.create_function(
            move |_, (convert, ratio, safe_size): (Option<Value>, Option<usize>, Option<usize>)| {
                let mut settings = sparse.lock();
                if let Some(convert) = convert.as_ref().and_then(flag) {
                    settings.sparse_convert = convert;
                }
                if let Some(ratio) = ratio {
                    settings.sparse_ratio = ratio;
                }
                if let Some(safe_size) = safe_size {
                    settings.sparse_safe = safe_size;
                }
                Ok((
                    settings.sparse_convert,
                    settings.sparse_ratio,
                    settings.sparse_safe,
                ))
            },
        )?,
    )?;
    module.raw_set(
        "new",
        lua.create_function(move |lua, ()| instance(lua, safe))?,
    )?;
    Ok(module)
}
