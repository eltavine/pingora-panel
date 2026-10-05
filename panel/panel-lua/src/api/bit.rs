//! `bit`, with LuaJIT BitOp's semantics: arguments are taken modulo 2^32
//! and results are signed 32-bit numbers.

use mlua::{Lua, Table, Variadic};

/// A number normalized as `bit.tobit` does.
fn tobit(number: f64) -> i32 {
    let wrapped = number.trunc().rem_euclid(4_294_967_296.0);
    (wrapped as u64 as u32) as i32
}

fn fold(numbers: &[f64], start: i32, operation: fn(i32, i32) -> i32) -> f64 {
    f64::from(
        numbers
            .iter()
            .fold(start, |result, &number| operation(result, tobit(number))),
    )
}

pub(super) fn module(lua: &Lua) -> mlua::Result<Table> {
    let bit = lua.create_table()?;
    bit.raw_set(
        "tobit",
        lua.create_function(|_, x: f64| Ok(f64::from(tobit(x))))?,
    )?;
    bit.raw_set(
        "tohex",
        lua.create_function(|_, (x, digits): (f64, Option<i64>)| {
            let digits = digits.unwrap_or(8);
            let width = digits.unsigned_abs().clamp(1, 8) as usize;
            let value = tobit(x) as u32;
            let text = format!("{value:08x}");
            let text = &text[8 - width..];
            Ok(if digits < 0 {
                text.to_ascii_uppercase()
            } else {
                text.to_owned()
            })
        })?,
    )?;
    bit.raw_set(
        "bnot",
        lua.create_function(|_, x: f64| Ok(f64::from(!tobit(x))))?,
    )?;
    bit.raw_set(
        "band",
        lua.create_function(|_, xs: Variadic<f64>| Ok(fold(&xs, -1, |a, b| a & b)))?,
    )?;
    bit.raw_set(
        "bor",
        lua.create_function(|_, xs: Variadic<f64>| Ok(fold(&xs, 0, |a, b| a | b)))?,
    )?;
    bit.raw_set(
        "bxor",
        lua.create_function(|_, xs: Variadic<f64>| Ok(fold(&xs, 0, |a, b| a ^ b)))?,
    )?;
    let shift = |operation: fn(i32, u32) -> i32| {
        move |_: &Lua, (x, n): (f64, f64)| Ok(f64::from(operation(tobit(x), tobit(n) as u32 & 31)))
    };
    bit.raw_set(
        "lshift",
        lua.create_function(shift(|x, n| ((x as u32) << n) as i32))?,
    )?;
    bit.raw_set(
        "rshift",
        lua.create_function(shift(|x, n| ((x as u32) >> n) as i32))?,
    )?;
    bit.raw_set("arshift", lua.create_function(shift(|x, n| x >> n))?)?;
    bit.raw_set(
        "rol",
        lua.create_function(shift(|x, n| (x as u32).rotate_left(n) as i32))?,
    )?;
    bit.raw_set(
        "ror",
        lua.create_function(shift(|x, n| (x as u32).rotate_right(n) as i32))?,
    )?;
    bit.raw_set(
        "bswap",
        lua.create_function(|_, x: f64| Ok(f64::from(tobit(x).swap_bytes())))?,
    )?;
    Ok(bit)
}

#[cfg(test)]
mod tests {
    use super::tobit;

    #[test]
    fn numbers_normalize_as_luajit_bitop_does() {
        assert_eq!(tobit(0xffff_ffff_u32 as f64), -1);
        assert_eq!(tobit(4_294_967_296.0 + 5.0), 5);
        assert_eq!(tobit(-1.0), -1);
        assert_eq!(tobit(2_147_483_648.0), i32::MIN);
    }
}
