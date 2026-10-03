//! Typed argument values: booleans, integers, durations, sizes, addresses and
//! `key=value` parameters.

use panel_dsl::Argument;
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
};

/// `on` or `off`; `true` and `false` are accepted as well.
pub fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "on" | "true" => Some(true),
        "off" | "false" => Some(false),
        _ => None,
    }
}

pub fn print_bool(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

const DURATION_UNITS: [(&str, u64); 5] = [
    ("d", 86_400_000),
    ("h", 3_600_000),
    ("m", 60_000),
    ("s", 1_000),
    ("ms", 1),
];

/// A duration in milliseconds, written as NGINX does: `500ms`, `30s`, `5m`,
/// `1h`, `2d` or a sum such as `1m30s`; a bare number means seconds.
pub fn parse_duration_ms(value: &str) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse::<u64>().ok()?.checked_mul(1_000);
    }
    let mut total: u64 = 0;
    let mut rest = value;
    let mut last_unit = usize::MAX;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let amount: u64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        // `ms` must win over `m`, and units must not repeat or go back up.
        let (index, (unit, factor)) = DURATION_UNITS
            .iter()
            .enumerate()
            .filter(|(_, (unit, _))| rest.starts_with(unit))
            .max_by_key(|(_, (unit, _))| unit.len())?;
        if last_unit != usize::MAX && index <= last_unit {
            return None;
        }
        last_unit = index;
        rest = &rest[unit.len()..];
        total = total.checked_add(amount.checked_mul(*factor)?)?;
    }
    Some(total)
}

/// The largest unit that states `ms` exactly.
pub fn print_duration_ms(ms: u64) -> String {
    if ms == 0 {
        return "0s".into();
    }
    let (unit, factor) = DURATION_UNITS
        .iter()
        .find(|(_, factor)| ms.is_multiple_of(*factor))
        .expect("milliseconds divide everything");
    format!("{}{unit}", ms / factor)
}

const SIZE_UNITS: [(char, u64); 3] = [('g', 1 << 30), ('m', 1 << 20), ('k', 1 << 10)];

/// A size in bytes: a bare number or one with a `k`, `m` or `g` suffix.
pub fn parse_size(value: &str) -> Option<u64> {
    let (digits, factor) = match value.chars().last()? {
        last if last.is_ascii_digit() => (value, 1),
        last => {
            let factor = SIZE_UNITS
                .iter()
                .find(|(unit, _)| *unit == last.to_ascii_lowercase())?
                .1;
            (&value[..value.len() - 1], factor)
        }
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()?.checked_mul(factor)
}

pub fn print_size(bytes: u64) -> String {
    SIZE_UNITS
        .iter()
        .find(|(_, factor)| bytes != 0 && bytes.is_multiple_of(*factor))
        .map_or_else(
            || bytes.to_string(),
            |(unit, factor)| format!("{}{unit}", bytes / factor),
        )
}

/// An IP network such as `10.0.0.0/8` or `2001:db8::/32`; a bare address is
/// a single host.
pub fn parse_cidr(value: &str) -> Option<(IpAddr, u8)> {
    let (address, prefix) = match value.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => (value, None),
    };
    let address: IpAddr = address.parse().ok()?;
    let max = if address.is_ipv4() { 32 } else { 128 };
    let prefix = match prefix {
        Some(prefix) if !prefix.is_empty() && prefix.bytes().all(|byte| byte.is_ascii_digit()) => {
            prefix.parse::<u8>().ok().filter(|prefix| *prefix <= max)?
        }
        Some(_) => return None,
        None => max,
    };
    Some((address, prefix))
}

pub fn parse_socket_address(value: &str) -> Option<SocketAddr> {
    value.parse().ok()
}

/// Arguments split into positional ones and `key=value` parameters. A key is
/// lowercase letters and underscores, so values such as URLs stay positional.
#[derive(Debug, Default)]
pub struct Params<'a> {
    pub positional: Vec<&'a Argument>,
    pub named: BTreeMap<&'a str, (&'a str, &'a Argument)>,
    /// Keys given more than once.
    pub repeated: Vec<&'a Argument>,
}

impl<'a> Params<'a> {
    pub fn split(args: &'a [Argument]) -> Self {
        let mut params = Self::default();
        for arg in args {
            match arg.value.split_once('=') {
                Some((key, value))
                    if !key.is_empty()
                        && key
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte == b'_') =>
                {
                    if params.named.insert(key, (value, arg)).is_some() {
                        params.repeated.push(arg);
                    }
                }
                _ => params.positional.push(arg),
            }
        }
        params
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_follow_nginx() {
        assert_eq!(parse_duration_ms("30"), Some(30_000));
        assert_eq!(parse_duration_ms("500ms"), Some(500));
        assert_eq!(parse_duration_ms("1m30s"), Some(90_000));
        assert_eq!(parse_duration_ms("2d"), Some(172_800_000));
        assert_eq!(parse_duration_ms("1h5ms"), Some(3_600_005));
        for invalid in ["", "s", "5x", "1s1m", "1s1s", "1.5s", "-1s"] {
            assert_eq!(parse_duration_ms(invalid), None, "{invalid}");
        }
        assert_eq!(print_duration_ms(90_000), "90s");
        assert_eq!(print_duration_ms(300_000), "5m");
        assert_eq!(print_duration_ms(1_500), "1500ms");
        assert_eq!(print_duration_ms(0), "0s");
        for ms in [1, 999, 60_000, 3_600_000, 86_400_000, 90_061_001] {
            assert_eq!(parse_duration_ms(&print_duration_ms(ms)), Some(ms));
        }
    }

    #[test]
    fn sizes_use_binary_suffixes() {
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_size("10k"), Some(10_240));
        assert_eq!(parse_size("1M"), Some(1 << 20));
        assert_eq!(parse_size("2g"), Some(2 << 30));
        for invalid in ["", "k", "1.5m", "10t", "-1"] {
            assert_eq!(parse_size(invalid), None, "{invalid}");
        }
        assert_eq!(print_size(10_240), "10k");
        assert_eq!(print_size(1_000), "1000");
        assert_eq!(print_size(0), "0");
    }

    #[test]
    fn networks_and_booleans() {
        assert_eq!(
            parse_cidr("10.0.0.0/8"),
            Some(("10.0.0.0".parse().unwrap(), 8))
        );
        assert_eq!(parse_cidr("2001:db8::/32").unwrap().1, 32);
        assert_eq!(parse_cidr("192.0.2.1").unwrap().1, 32);
        for invalid in ["10.0.0.0/33", "10.0.0.0/", "host/8", "::/129"] {
            assert_eq!(parse_cidr(invalid), None, "{invalid}");
        }
        assert_eq!(parse_bool("on"), Some(true));
        assert_eq!(parse_bool("false"), Some(false));
        assert_eq!(parse_bool("yes"), None);
    }

    #[test]
    fn parameters_are_split_from_positional_arguments() {
        let args: Vec<_> = [
            "10.0.0.1:80",
            "weight=2",
            "backup",
            "https://a.example/?x=1",
            "weight=3",
        ]
        .into_iter()
        .map(Argument::new)
        .collect();
        let params = Params::split(&args);
        assert_eq!(params.positional.len(), 3);
        assert_eq!(params.named["weight"].0, "3");
        assert_eq!(params.repeated.len(), 1);
    }
}
