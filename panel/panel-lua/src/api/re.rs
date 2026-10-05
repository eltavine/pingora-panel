//! `ngx.re` on PCRE2, the engine NGINX uses, with compiled expressions
//! cached as `lua_regex_cache_max_entries` does by default.

use super::{failed, results};
use lru::LruCache;
use mlua::{Function, Lua, LuaString, MultiValue, Table, Value};
use parking_lot::Mutex;
use pcre2::bytes::{CaptureLocations, Regex, RegexBuilder};
use std::{
    num::NonZeroUsize,
    sync::{Arc, LazyLock},
};

const CACHE_ENTRIES: usize = 1024;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct Flags {
    anchored: bool,
    caseless: bool,
    duplicate_names: bool,
    multi_line: bool,
    dotall: bool,
    utf: bool,
    extended: bool,
}

fn flags(options: Option<&str>) -> mlua::Result<Flags> {
    let mut flags = Flags::default();
    for option in options.unwrap_or_default().chars() {
        match option {
            'a' => flags.anchored = true,
            'i' => flags.caseless = true,
            'D' => flags.duplicate_names = true,
            'm' => flags.multi_line = true,
            's' => flags.dotall = true,
            'u' => flags.utf = true,
            'U' => flags.utf = true,
            'x' => flags.extended = true,
            // JIT and compile-once are what every expression gets.
            'j' | 'o' => {}
            'd' | 'J' => {
                return Err(mlua::Error::runtime(format!(
                    "regex option \"{option}\" is not available in Pingora Panel"
                )));
            }
            other => {
                return Err(mlua::Error::runtime(format!("unknown flag \"{other}\"")));
            }
        }
    }
    Ok(flags)
}

type Cache = Mutex<LruCache<(String, Flags), Arc<Regex>>>;

static CACHE: LazyLock<Cache> =
    LazyLock::new(|| Mutex::new(LruCache::new(NonZeroUsize::new(CACHE_ENTRIES).unwrap())));

fn compile(pattern: &[u8], flags: Flags) -> Result<Arc<Regex>, String> {
    let pattern = std::str::from_utf8(pattern)
        .map_err(|_| "the regular expression is not valid UTF-8".to_owned())?;
    let key = (pattern.to_owned(), flags);
    if let Some(regex) = CACHE.lock().get(&key) {
        return Ok(Arc::clone(regex));
    }
    let mut text = String::with_capacity(pattern.len() + 10);
    if flags.duplicate_names {
        text.push_str("(?J)");
    }
    if flags.anchored {
        text.push_str("\\G(?:");
        text.push_str(pattern);
        text.push(')');
    } else {
        text.push_str(pattern);
    }
    let mut builder = RegexBuilder::new();
    builder
        .caseless(flags.caseless)
        .multi_line(flags.multi_line)
        .dotall(flags.dotall)
        .extended(flags.extended)
        .utf(flags.utf)
        .jit_if_available(true);
    let regex = Arc::new(builder.build(&text).map_err(|error| error.to_string())?);
    CACHE.lock().put(key, Arc::clone(&regex));
    Ok(regex)
}

fn options_text(options: &Option<LuaString>) -> mlua::Result<Option<String>> {
    options
        .as_ref()
        .map(|options| options.to_str().map(|text| text.to_owned()))
        .transpose()
}

/// The 0-based offset `ctx.pos` asks to start at.
fn start(ctx: &Option<Table>) -> mlua::Result<usize> {
    let Some(ctx) = ctx else {
        return Ok(0);
    };
    let position: Option<i64> = ctx.raw_get("pos")?;
    Ok(usize::try_from(position.unwrap_or(1).max(1) - 1).unwrap_or(0))
}

fn find_at(
    regex: &Regex,
    locations: &mut CaptureLocations,
    subject: &[u8],
    start: usize,
) -> Result<Option<(usize, usize)>, String> {
    if start > subject.len() {
        return Ok(None);
    }
    regex
        .captures_read_at(locations, subject, start)
        .map(|found| found.map(|found| (found.start(), found.end())))
        .map_err(|error| error.to_string())
}

fn captures_table(
    lua: &Lua,
    regex: &Regex,
    locations: &CaptureLocations,
    subject: &[u8],
    flags: Flags,
    reuse: Option<Table>,
) -> mlua::Result<Table> {
    let table = match reuse {
        Some(table) => table,
        None => lua.create_table_with_capacity(locations.len(), 0)?,
    };
    let value = |index: usize| -> mlua::Result<Value> {
        Ok(match locations.get(index) {
            Some((from, to)) => Value::String(lua.create_string(&subject[from..to])?),
            None => Value::Boolean(false),
        })
    };
    for index in 0..locations.len() {
        table.raw_set(index, value(index)?)?;
    }
    for (index, name) in regex.capture_names().iter().enumerate() {
        let Some(name) = name else {
            continue;
        };
        if flags.duplicate_names {
            let matched: Value = match table.raw_get::<Value>(name.as_str())? {
                Value::Table(values) => {
                    if let found @ Value::String(_) = value(index)? {
                        values.raw_push(found)?;
                    }
                    Value::Table(values)
                }
                _ => {
                    let values = lua.create_table()?;
                    if let found @ Value::String(_) = value(index)? {
                        values.raw_push(found)?;
                    }
                    Value::Table(values)
                }
            };
            table.raw_set(name.as_str(), matched)?;
        } else {
            table.raw_set(name.as_str(), value(index)?)?;
        }
    }
    Ok(table)
}

/// Where to look next after a match ending at `end`; past an empty match by
/// one character, which in UTF-8 mode is one code point.
fn next_start(subject: &[u8], from: usize, end: usize, flags: Flags) -> usize {
    if end > from {
        return end;
    }
    let mut next = end + 1;
    if flags.utf {
        while next < subject.len() && subject[next] & 0xc0 == 0x80 {
            next += 1;
        }
    }
    next
}

/// The replacement a template such as `[$0][${1}]$$` gives for a match.
fn expand(
    template: &[u8],
    locations: &CaptureLocations,
    subject: &[u8],
    into: &mut Vec<u8>,
) -> Result<(), String> {
    let mut index = 0;
    while index < template.len() {
        let byte = template[index];
        if byte != b'$' {
            into.push(byte);
            index += 1;
            continue;
        }
        index += 1;
        let (group, length) = match template.get(index) {
            Some(b'$') => {
                into.push(b'$');
                index += 1;
                continue;
            }
            Some(b'{') => {
                let close = template[index..]
                    .iter()
                    .position(|&byte| byte == b'}')
                    .ok_or("the replace template has an unclosed \"${\"")?;
                let digits = &template[index + 1..index + close];
                (digits, close + 1)
            }
            Some(byte) if byte.is_ascii_digit() => {
                let digits = template[index..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_digit())
                    .count();
                (&template[index..index + digits], digits)
            }
            _ => return Err("invalid capturing variable name in the replace template".into()),
        };
        let group: usize = std::str::from_utf8(group)
            .ok()
            .and_then(|digits| digits.parse().ok())
            .ok_or("invalid capturing variable name in the replace template")?;
        if let Some((from, to)) = locations.get(group) {
            into.extend_from_slice(&subject[from..to]);
        }
        index += length;
    }
    Ok(())
}

enum Replace {
    Template(Vec<u8>),
    Function(Function),
}

fn substitute(
    lua: &Lua,
    subject: &[u8],
    pattern: &[u8],
    replace: &Replace,
    options: Option<String>,
    global: bool,
) -> mlua::Result<MultiValue> {
    let flags = flags(options.as_deref())?;
    let regex = match compile(pattern, flags) {
        Ok(regex) => regex,
        Err(error) => return failed(lua, 2, &error),
    };
    let mut locations = regex.capture_locations();
    let mut output = Vec::with_capacity(subject.len());
    let mut position = 0;
    let mut copied = 0;
    let mut count = 0;
    loop {
        let (from, to) = match find_at(&regex, &mut locations, subject, position) {
            Ok(Some(found)) => found,
            Ok(None) => break,
            Err(error) => return failed(lua, 2, &error),
        };
        output.extend_from_slice(&subject[copied..from]);
        match replace {
            Replace::Template(template) => {
                if let Err(error) = expand(template, &locations, subject, &mut output) {
                    return failed(lua, 2, &error);
                }
            }
            Replace::Function(function) => {
                let captures = captures_table(lua, &regex, &locations, subject, flags, None)?;
                let replaced: Value = function.call(captures)?;
                match super::bytes(&replaced) {
                    Some(text) => output.extend_from_slice(&text),
                    None if replaced.is_nil() => {}
                    None => {
                        return Err(mlua::Error::runtime(format!(
                            "attempt to use {} as the replacement",
                            replaced.type_name()
                        )));
                    }
                }
            }
        }
        copied = to;
        count += 1;
        position = next_start(subject, from, to, flags);
        if from == to && from < subject.len() {
            output.extend_from_slice(&subject[to..position.min(subject.len())]);
            copied = position.min(subject.len());
        }
        if !global || position > subject.len() {
            break;
        }
    }
    output.extend_from_slice(&subject[copied.min(subject.len())..]);
    Ok(results([
        Value::String(lua.create_string(output)?),
        Value::Integer(count),
    ]))
}

pub(super) fn table(lua: &Lua) -> mlua::Result<Table> {
    let re = lua.create_table()?;
    re.raw_set(
        "match",
        lua.create_function(
            |lua,
             (subject, pattern, options, ctx, reuse): (
                LuaString,
                LuaString,
                Option<LuaString>,
                Option<Table>,
                Option<Table>,
            )| {
                let flags = flags(options_text(&options)?.as_deref())?;
                let regex = match compile(&pattern.as_bytes(), flags) {
                    Ok(regex) => regex,
                    Err(error) => return failed(lua, 1, &error),
                };
                let subject = subject.as_bytes();
                let mut locations = regex.capture_locations();
                match find_at(&regex, &mut locations, &subject, start(&ctx)?) {
                    Ok(Some((_, to))) => {
                        if let Some(ctx) = &ctx {
                            ctx.raw_set("pos", to as i64 + 1)?;
                        }
                        let table =
                            captures_table(lua, &regex, &locations, &subject, flags, reuse)?;
                        Ok(results([Value::Table(table)]))
                    }
                    Ok(None) => Ok(results([Value::Nil])),
                    Err(error) => failed(lua, 1, &error),
                }
            },
        )?,
    )?;
    re.raw_set(
        "find",
        lua.create_function(
            |lua,
             (subject, pattern, options, ctx, nth): (
                LuaString,
                LuaString,
                Option<LuaString>,
                Option<Table>,
                Option<usize>,
            )| {
                let flags = flags(options_text(&options)?.as_deref())?;
                let regex = match compile(&pattern.as_bytes(), flags) {
                    Ok(regex) => regex,
                    Err(error) => return failed(lua, 2, &error),
                };
                let subject = subject.as_bytes();
                let mut locations = regex.capture_locations();
                match find_at(&regex, &mut locations, &subject, start(&ctx)?) {
                    Ok(Some((_, to))) => {
                        if let Some(ctx) = &ctx {
                            ctx.raw_set("pos", to as i64 + 1)?;
                        }
                        Ok(match locations.get(nth.unwrap_or(0)) {
                            Some((from, to)) => results([
                                Value::Integer(from as i64 + 1),
                                Value::Integer(to as i64),
                            ]),
                            None => results([Value::Nil, Value::Nil]),
                        })
                    }
                    Ok(None) => Ok(results([Value::Nil])),
                    Err(error) => failed(lua, 2, &error),
                }
            },
        )?,
    )?;
    re.raw_set(
        "gmatch",
        lua.create_function(
            |lua, (subject, pattern, options): (LuaString, LuaString, Option<LuaString>)| {
                let flags = flags(options_text(&options)?.as_deref())?;
                let regex = match compile(&pattern.as_bytes(), flags) {
                    Ok(regex) => regex,
                    Err(error) => return failed(lua, 1, &error),
                };
                let subject = subject.as_bytes().to_vec();
                let position = Mutex::new(Some(0usize));
                let iterator = lua.create_function(move |lua, ()| {
                    let mut position = position.lock();
                    let Some(at) = *position else {
                        return Ok(results([Value::Nil]));
                    };
                    let mut locations = regex.capture_locations();
                    match find_at(&regex, &mut locations, &subject, at) {
                        Ok(Some((from, to))) => {
                            let next = next_start(&subject, from, to, flags);
                            *position = (next <= subject.len()).then_some(next);
                            let table =
                                captures_table(lua, &regex, &locations, &subject, flags, None)?;
                            Ok(results([Value::Table(table)]))
                        }
                        Ok(None) => {
                            *position = None;
                            Ok(results([Value::Nil]))
                        }
                        Err(error) => {
                            *position = None;
                            failed(lua, 1, &error)
                        }
                    }
                })?;
                Ok(results([Value::Function(iterator)]))
            },
        )?,
    )?;
    for (name, global) in [("sub", false), ("gsub", true)] {
        re.raw_set(
            name,
            lua.create_function(
                move |lua,
                      (subject, pattern, replace, options): (
                    LuaString,
                    LuaString,
                    Value,
                    Option<LuaString>,
                )| {
                    let replace = match replace {
                        Value::Function(function) => Replace::Function(function),
                        other => Replace::Template(super::bytes(&other).ok_or_else(|| {
                            mlua::Error::runtime("bad argument #3 (string or function expected)")
                        })?),
                    };
                    substitute(
                        lua,
                        &subject.as_bytes(),
                        &pattern.as_bytes(),
                        &replace,
                        options_text(&options)?,
                        global,
                    )
                },
            )?,
        )?;
    }
    re.raw_set(
        "split",
        lua.create_function(
            |lua,
             (subject, pattern, options, ctx, max, reuse): (
                LuaString,
                LuaString,
                Option<LuaString>,
                Option<Table>,
                Option<i64>,
                Option<Table>,
            )| {
                split(
                    lua,
                    &subject.as_bytes(),
                    &pattern.as_bytes(),
                    options,
                    ctx,
                    max,
                    reuse,
                )
            },
        )?,
    )?;
    re.raw_set(
        "opt",
        lua.create_function(
            |_, (option, _value): (LuaString, Value)| match &*option.as_bytes() {
                b"jit_stack_size" | b"jit_stack_size_max" => Ok(()),
                other => Err(mlua::Error::runtime(format!(
                    "unrecognized option name \"{}\"",
                    String::from_utf8_lossy(other)
                ))),
            },
        )?,
    )?;
    Ok(re)
}

fn split(
    lua: &Lua,
    subject: &[u8],
    pattern: &[u8],
    options: Option<LuaString>,
    ctx: Option<Table>,
    max: Option<i64>,
    reuse: Option<Table>,
) -> mlua::Result<MultiValue> {
    let flags = flags(options_text(&options)?.as_deref())?;
    let max = max.filter(|max| *max > 0).map(|max| max as usize);
    let table = match reuse {
        Some(table) => table,
        None => lua.create_table()?,
    };
    let mut pieces: Vec<Vec<u8>> = Vec::new();
    let begin = start(&ctx)?.min(subject.len());
    if pattern.is_empty() {
        let mut at = begin;
        while at < subject.len() {
            if max.is_some_and(|max| pieces.len() + 1 == max) {
                pieces.push(subject[at..].to_vec());
                break;
            }
            let next = next_start(subject, at, at, flags);
            pieces.push(subject[at..next.min(subject.len())].to_vec());
            at = next;
        }
    } else {
        let regex = match compile(pattern, flags) {
            Ok(regex) => regex,
            Err(error) => return failed(lua, 1, &error),
        };
        let mut locations = regex.capture_locations();
        let mut piece_start = begin;
        let mut position = begin;
        loop {
            if max.is_some_and(|max| pieces.len() + 1 >= max) {
                break;
            }
            let (from, to) = match find_at(&regex, &mut locations, subject, position) {
                Ok(Some(found)) => found,
                Ok(None) => break,
                Err(error) => return failed(lua, 1, &error),
            };
            if from == to {
                if from >= subject.len() {
                    break;
                }
                if from == piece_start {
                    position = next_start(subject, from, to, flags);
                    continue;
                }
            }
            pieces.push(subject[piece_start..from].to_vec());
            if let Some((capture_from, capture_to)) = locations.get(1) {
                pieces.push(subject[capture_from..capture_to].to_vec());
            }
            piece_start = to;
            position = next_start(subject, from, to, flags);
        }
        pieces.push(subject[piece_start.min(subject.len())..].to_vec());
    }
    let count = pieces.len();
    for (index, piece) in pieces.into_iter().enumerate() {
        table.raw_set(index + 1, lua.create_string(piece)?)?;
    }
    table.raw_set(count + 1, Value::Nil)?;
    Ok(results([Value::Table(table)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_expand_groups_braces_and_dollars() {
        let regex = compile(b"([0-9])[0-9]", Flags::default()).unwrap();
        let mut locations = regex.capture_locations();
        find_at(&regex, &mut locations, b"hello, 1234", 0).unwrap();
        let mut output = Vec::new();
        expand(b"[$0][${1}]$$", &locations, b"hello, 1234", &mut output).unwrap();
        assert_eq!(output, b"[12][1]$");
    }

    #[test]
    fn options_map_to_pcre2_or_are_refused() {
        assert!(flags(Some("jo")).is_ok());
        assert!(flags(Some("aiDmsuUx")).is_ok());
        assert!(flags(Some("d")).is_err());
        assert!(flags(Some("z")).is_err());
    }
}
