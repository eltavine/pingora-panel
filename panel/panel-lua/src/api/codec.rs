//! Escaping, query arguments, base64, digests and SQL quoting, as
//! lua-nginx-module does them.

use super::bytes;
use base64::{
    alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
    Engine,
};
use hmac::{Hmac, KeyInit, Mac};
use md5::{Digest, Md5};
use mlua::{Lua, LuaString, Table, Value};
use sha1::Sha1;

/// Bytes nginx escapes in a whole URI (`NGX_ESCAPE_URI`): space, `#`, `%`,
/// `?`, controls and bytes above 0x7e.
const URI: [u32; 8] = [
    0xffff_ffff,
    0x8000_0029,
    0x0000_0000,
    0x8000_0000,
    0xffff_ffff,
    0xffff_ffff,
    0xffff_ffff,
    0xffff_ffff,
];

/// Bytes escaped in a URI component: everything but RFC 3986's unreserved
/// characters.
const URI_COMPONENT: [u32; 8] = [
    0xffff_ffff,
    0xfc00_9fff,
    0x7800_0001,
    0xb800_0001,
    0xffff_ffff,
    0xffff_ffff,
    0xffff_ffff,
    0xffff_ffff,
];

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn escaped(table: &[u32; 8], byte: u8) -> bool {
    table[usize::from(byte >> 5)] & (1 << (byte & 0x1f)) != 0
}

pub(crate) fn escape_uri(input: &[u8], component: bool) -> Vec<u8> {
    let table = if component { &URI_COMPONENT } else { &URI };
    let mut output = Vec::with_capacity(input.len());
    for &byte in input {
        if escaped(table, byte) {
            output.extend_from_slice(&[
                b'%',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 15)],
            ]);
        } else {
            output.push(byte);
        }
    }
    output
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decodes `%XX` and `+`, keeping malformed escapes as they are.
pub(crate) fn unescape_uri(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        match input[index] {
            b'+' => output.push(b' '),
            b'%' => {
                let decoded = input
                    .get(index + 1)
                    .and_then(|&high| hex_value(high))
                    .zip(input.get(index + 2).and_then(|&low| hex_value(low)));
                if let Some((high, low)) = decoded {
                    output.push(high << 4 | low);
                    index += 3;
                    continue;
                }
                output.push(b'%');
            }
            byte => output.push(byte),
        }
        index += 1;
    }
    output
}

/// A query's arguments: a key and its value, `None` for a key without `=`.
pub(crate) type Arguments = Vec<(Vec<u8>, Option<Vec<u8>>)>;

/// A query parsed as lua-nginx-module parses it: keys and values unescaped,
/// a key without `=` taking `true`, empty keys dropped. The flag says
/// arguments past `max` (zero for no limit) were dropped.
pub(crate) fn parse_args(query: &[u8], max: usize) -> (Arguments, bool) {
    let mut arguments = Vec::new();
    for part in query.split(|&byte| byte == b'&') {
        if part.is_empty() {
            continue;
        }
        if max != 0 && arguments.len() == max {
            return (arguments, true);
        }
        let (key, value) = match part.iter().position(|&byte| byte == b'=') {
            Some(at) => (&part[..at], Some(unescape_uri(&part[at + 1..]))),
            None => (part, None),
        };
        if key.is_empty() {
            continue;
        }
        arguments.push((unescape_uri(key), value));
    }
    (arguments, false)
}

/// Arguments as a Lua table: repeated keys hold a table of their values.
pub(crate) fn args_table(lua: &Lua, arguments: Arguments) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    for (key, value) in arguments {
        let key = lua.create_string(&key)?;
        let value = match value {
            Some(value) => Value::String(lua.create_string(&value)?),
            None => Value::Boolean(true),
        };
        match table.raw_get::<Value>(&key)? {
            Value::Nil => table.raw_set(key, value)?,
            Value::Table(values) => values.raw_push(value)?,
            first => {
                let values = lua.create_table()?;
                values.raw_push(first)?;
                values.raw_push(value)?;
                table.raw_set(key, values)?;
            }
        }
    }
    Ok(table)
}

fn encode_arg(output: &mut Vec<u8>, key: &[u8], value: &Value) -> mlua::Result<()> {
    let text = match value {
        Value::Boolean(false) => return Ok(()),
        Value::Boolean(true) => None,
        other => Some(bytes(other).ok_or_else(|| {
            mlua::Error::runtime(format!(
                "attempt to use {} as query arg value",
                other.type_name()
            ))
        })?),
    };
    if !output.is_empty() {
        output.push(b'&');
    }
    output.extend_from_slice(&escape_uri(key, true));
    if let Some(text) = text {
        output.push(b'=');
        output.extend_from_slice(&escape_uri(&text, true));
    }
    Ok(())
}

/// A table encoded as a query, keys in order so the result is stable.
pub(crate) fn encode_args(table: &Table) -> mlua::Result<Vec<u8>> {
    let mut entries: Vec<(Vec<u8>, Value)> = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Some(key) = bytes(&key) else {
            return Err(mlua::Error::runtime(format!(
                "attempt to use {} as query arg key",
                key.type_name()
            )));
        };
        entries.push((key, value));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut output = Vec::new();
    for (key, value) in entries {
        match value {
            Value::Table(values) => {
                for value in values.sequence_values::<Value>() {
                    encode_arg(&mut output, &key, &value?)?;
                }
            }
            other => encode_arg(&mut output, &key, &other)?,
        }
    }
    Ok(output)
}

const LENIENT: GeneralPurposeConfig = GeneralPurposeConfig::new()
    .with_encode_padding(true)
    .with_decode_padding_mode(DecodePaddingMode::Indifferent);
const STANDARD: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, LENIENT);
const UNPADDED: GeneralPurpose =
    GeneralPurpose::new(&alphabet::STANDARD, LENIENT.with_encode_padding(false));
const URL_SAFE: GeneralPurpose =
    GeneralPurpose::new(&alphabet::URL_SAFE, LENIENT.with_encode_padding(false));

pub(crate) fn encode_base64(input: &[u8], padding: bool) -> String {
    if padding {
        STANDARD.encode(input)
    } else {
        UNPADDED.encode(input)
    }
}

pub(crate) fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    STANDARD.decode(input).ok()
}

/// Base64 in MIME bodies: characters outside the alphabet are skipped.
fn decode_base64_mime(input: &[u8]) -> Option<Vec<u8>> {
    let kept: Vec<u8> = input
        .iter()
        .copied()
        .filter(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
        .collect();
    STANDARD.decode(kept).ok()
}

pub(crate) fn encode_base64url(input: &[u8]) -> String {
    URL_SAFE.encode(input)
}

pub(crate) fn decode_base64url(input: &[u8]) -> Option<Vec<u8>> {
    URL_SAFE.decode(input).ok()
}

/// MySQL string literal quoting, as `ngx.quote_sql_str` does it.
fn quote_sql(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len() + 2);
    output.push(b'\'');
    for &byte in input {
        match byte {
            0 => output.extend_from_slice(b"\\0"),
            8 => output.extend_from_slice(b"\\b"),
            b'\n' => output.extend_from_slice(b"\\n"),
            b'\r' => output.extend_from_slice(b"\\r"),
            b'\t' => output.extend_from_slice(b"\\t"),
            26 => output.extend_from_slice(b"\\Z"),
            b'\\' => output.extend_from_slice(b"\\\\"),
            b'\'' => output.extend_from_slice(b"\\'"),
            b'"' => output.extend_from_slice(b"\\\""),
            other => output.push(other),
        }
    }
    output.push(b'\'');
    output
}

pub(super) fn install(lua: &Lua, ngx: &Table) -> mlua::Result<()> {
    ngx.raw_set(
        "escape_uri",
        lua.create_function(|lua, (input, kind): (Value, Option<i64>)| {
            let component = match kind.unwrap_or(2) {
                0 => false,
                2 => true,
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "bad argument #2 to 'escape_uri' (\"type\" {other} out of range)"
                    )));
                }
            };
            match bytes(&input) {
                Some(input) => Ok(Value::String(
                    lua.create_string(escape_uri(&input, component))?,
                )),
                None if input.is_nil() => Ok(Value::String(lua.create_string("")?)),
                None => Err(mlua::Error::runtime(
                    "bad argument #1 to 'escape_uri' (string expected)",
                )),
            }
        })?,
    )?;
    ngx.raw_set(
        "unescape_uri",
        lua.create_function(|lua, input: Value| {
            let input = bytes(&input).unwrap_or_default();
            lua.create_string(unescape_uri(&input))
        })?,
    )?;
    ngx.raw_set(
        "encode_args",
        lua.create_function(|lua, table: Table| lua.create_string(encode_args(&table)?))?,
    )?;
    ngx.raw_set(
        "decode_args",
        lua.create_function(|lua, (query, max): (LuaString, Option<usize>)| {
            let (arguments, truncated) = parse_args(&query.as_bytes(), max.unwrap_or(100));
            super::maybe_truncated(lua, args_table(lua, arguments)?, truncated)
        })?,
    )?;
    ngx.raw_set(
        "encode_base64",
        lua.create_function(|_, (input, no_padding): (Value, Option<bool>)| {
            Ok(encode_base64(
                &bytes(&input).unwrap_or_default(),
                !no_padding.unwrap_or(false),
            ))
        })?,
    )?;
    ngx.raw_set(
        "decode_base64",
        lua.create_function(|lua, input: LuaString| {
            decode_base64(&input.as_bytes())
                .map(|decoded| lua.create_string(decoded))
                .transpose()
        })?,
    )?;
    ngx.raw_set(
        "decode_base64mime",
        lua.create_function(|lua, input: LuaString| {
            decode_base64_mime(&input.as_bytes())
                .map(|decoded| lua.create_string(decoded))
                .transpose()
        })?,
    )?;
    ngx.raw_set(
        "md5",
        lua.create_function(|_, input: Value| {
            Ok(hex::encode(Md5::digest(bytes(&input).unwrap_or_default())))
        })?,
    )?;
    ngx.raw_set(
        "md5_bin",
        lua.create_function(|lua, input: Value| {
            lua.create_string(Md5::digest(bytes(&input).unwrap_or_default()))
        })?,
    )?;
    ngx.raw_set(
        "sha1_bin",
        lua.create_function(|lua, input: Value| {
            lua.create_string(Sha1::digest(bytes(&input).unwrap_or_default()))
        })?,
    )?;
    ngx.raw_set(
        "hmac_sha1",
        lua.create_function(|lua, (key, input): (LuaString, Value)| {
            let mut mac = Hmac::<Sha1>::new_from_slice(&key.as_bytes())
                .map_err(|_| mlua::Error::runtime("bad HMAC key"))?;
            mac.update(&bytes(&input).unwrap_or_default());
            lua.create_string(mac.finalize().into_bytes())
        })?,
    )?;
    for name in ["crc32_short", "crc32_long"] {
        ngx.raw_set(
            name,
            lua.create_function(|_, input: Value| {
                Ok(f64::from(crc32fast::hash(
                    &bytes(&input).unwrap_or_default(),
                )))
            })?,
        )?;
    }
    ngx.raw_set(
        "quote_sql_str",
        lua.create_function(|lua, input: Value| {
            lua.create_string(quote_sql(&bytes(&input).unwrap_or_default()))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_escape_all_but_unreserved_characters() {
        assert_eq!(
            escape_uri(b"a b/c?d~e-f.g_h", true),
            b"a%20b%2Fc%3Fd~e-f.g_h"
        );
        assert_eq!(escape_uri(b"/a b?c#d", false), b"/a%20b%3Fc%23d");
        assert_eq!(escape_uri("é".as_bytes(), true), b"%C3%A9");
    }

    #[test]
    fn unescaping_decodes_plus_and_keeps_malformed_escapes() {
        assert_eq!(unescape_uri(b"b%20r56+7"), b"b r56 7");
        assert_eq!(unescape_uri(b"100%"), b"100%");
        assert_eq!(unescape_uri(b"%zz%41"), b"%zzA");
    }

    #[test]
    fn arguments_parse_as_lua_nginx_module_parses_them() {
        let (arguments, truncated) = parse_args(b"a%20b=1%61+2&foo&bar=&=x&&c=3", 0);
        assert!(!truncated);
        assert_eq!(
            arguments,
            vec![
                (b"a b".to_vec(), Some(b"1a 2".to_vec())),
                (b"foo".to_vec(), None),
                (b"bar".to_vec(), Some(Vec::new())),
                (b"c".to_vec(), Some(b"3".to_vec())),
            ]
        );
        assert!(parse_args(b"a=1&b=2&c=3", 2).1);
    }

    #[test]
    fn sql_strings_quote_as_mysql_expects() {
        assert_eq!(quote_sql(b"it's \"a\"\n\\"), b"'it\\'s \\\"a\\\"\\n\\\\'");
    }

    #[test]
    fn base64_decodes_with_or_without_padding() {
        assert_eq!(encode_base64(b"hi", true), "aGk=");
        assert_eq!(encode_base64(b"hi", false), "aGk");
        assert_eq!(decode_base64(b"aGk").unwrap(), b"hi");
        assert_eq!(decode_base64(b"aGk=").unwrap(), b"hi");
        assert!(decode_base64(b"a!k=").is_none());
        assert_eq!(decode_base64_mime(b"aG\r\nk=").unwrap(), b"hi");
    }
}
