//! Time: `ngx.time`, `ngx.now`, dates and HTTP and cookie times.

use chrono::{DateTime, Local, TimeZone, Utc};
use mlua::{Lua, LuaString, Table};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

fn utc(seconds: f64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(seconds.floor() as i64, 0).single()
}

/// An HTTP date (RFC 9110 §5.6.7, IMF-fixdate).
pub(crate) fn http_time(seconds: f64) -> Option<String> {
    Some(
        utc(seconds)?
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string(),
    )
}

/// A cookie expiry as nginx writes it: two-digit years until 2037.
pub(crate) fn cookie_time(seconds: f64) -> Option<String> {
    let time = utc(seconds)?;
    Some(
        if time.format("%Y").to_string().parse::<i32>().ok()? > 2037 {
            time.format("%a, %d-%b-%Y %H:%M:%S GMT").to_string()
        } else {
            time.format("%a, %d-%b-%y %H:%M:%S GMT").to_string()
        },
    )
}

pub(super) fn install(lua: &Lua, ngx: &Table) -> mlua::Result<()> {
    ngx.raw_set(
        "time",
        lua.create_function(|_, ()| Ok(now().as_secs() as f64))?,
    )?;
    ngx.raw_set(
        "now",
        lua.create_function(|_, ()| Ok(now().as_millis() as f64 / 1000.0))?,
    )?;
    ngx.raw_set("update_time", lua.create_function(|_, ()| Ok(()))?)?;
    ngx.raw_set(
        "today",
        lua.create_function(|_, ()| Ok(Local::now().format("%Y-%m-%d").to_string()))?,
    )?;
    ngx.raw_set(
        "localtime",
        lua.create_function(|_, ()| Ok(Local::now().format("%Y-%m-%d %H:%M:%S").to_string()))?,
    )?;
    ngx.raw_set(
        "utctime",
        lua.create_function(|_, ()| Ok(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()))?,
    )?;
    ngx.raw_set(
        "http_time",
        lua.create_function(|_, seconds: f64| Ok(http_time(seconds)))?,
    )?;
    ngx.raw_set(
        "cookie_time",
        lua.create_function(|_, seconds: f64| Ok(cookie_time(seconds)))?,
    )?;
    ngx.raw_set(
        "parse_http_time",
        lua.create_function(|_, text: LuaString| {
            Ok(text
                .to_str()
                .ok()
                .and_then(|text| httpdate::parse_http_date(&text).ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|since| since.as_secs() as f64))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_format_as_lua_nginx_module_documents() {
        assert_eq!(
            http_time(1_290_079_655.0).unwrap(),
            "Thu, 18 Nov 2010 11:27:35 GMT"
        );
        assert_eq!(
            cookie_time(1_290_079_655.0).unwrap(),
            "Thu, 18-Nov-10 11:27:35 GMT"
        );
        assert_eq!(
            cookie_time(2_200_000_000.0).unwrap(),
            "Sun, 18-Sep-2039 23:06:40 GMT"
        );
    }
}
