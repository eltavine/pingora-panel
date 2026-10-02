//! W3C Trace Context (Recommendation, 23 November 2021), carried by HTTP and
//! gRPC requests and by the CloudEvents Distributed Tracing extension.

use serde::{Deserialize, Serialize};

/// Combined `tracestate` length every vendor should propagate.
pub const TRACESTATE_PROPAGATION_LIMIT: usize = 512;
const TRACESTATE_MAX_MEMBERS: usize = 32;
const TRACESTATE_LONG_ENTRY: usize = 128;
const TRACEPARENT_V00_LEN: usize = 55;

/// A validated `traceparent` with its optional `tracestate`.
///
/// Parsing follows the specification's receiver rules: an invalid
/// `traceparent` is ignored, and an invalid `tracestate` is dropped without
/// affecting `traceparent`. Stored values are always valid version `00`.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct TraceContext {
    traceparent: String,
    tracestate: Option<String>,
}

impl TraceContext {
    pub fn parse(traceparent: &str, tracestate: Option<&str>) -> Option<Self> {
        let traceparent = parse_traceparent(traceparent)?;
        Some(Self {
            traceparent,
            tracestate: tracestate.and_then(parse_tracestate),
        })
    }

    pub fn traceparent(&self) -> &str {
        &self.traceparent
    }

    pub fn tracestate(&self) -> Option<&str> {
        self.tracestate.as_deref()
    }

    pub fn trace_id(&self) -> &str {
        &self.traceparent[3..35]
    }

    pub fn parent_id(&self) -> &str {
        &self.traceparent[36..52]
    }

    pub fn sampled(&self) -> bool {
        u8::from_str_radix(&self.traceparent[53..55], 16).is_ok_and(|flags| flags & 1 == 1)
    }
}

fn is_hex(value: &str, lowercase_only: bool) -> bool {
    value.bytes().all(|byte| {
        byte.is_ascii_digit()
            || (b'a'..=b'f').contains(&byte)
            || (!lowercase_only && (b'A'..=b'F').contains(&byte))
    })
}

fn is_all_zero(value: &str) -> bool {
    value.bytes().all(|byte| byte == b'0')
}

fn parse_traceparent(value: &str) -> Option<String> {
    let value = value.trim_matches([' ', '\t']);
    let bytes = value.as_bytes();
    if bytes.len() < TRACEPARENT_V00_LEN || !value.is_ascii() {
        return None;
    }
    let version = &value[0..2];
    if !is_hex(version, true) || bytes[2] != b'-' || version == "ff" {
        return None;
    }
    let current = version == "00";
    if current && bytes.len() != TRACEPARENT_V00_LEN {
        return None;
    }
    if !current && bytes.len() > TRACEPARENT_V00_LEN && bytes[TRACEPARENT_V00_LEN] != b'-' {
        return None;
    }
    let (trace_id, parent_id, flags) = (&value[3..35], &value[36..52], &value[53..55]);
    let ids_valid = is_hex(trace_id, current)
        && is_hex(parent_id, current)
        && is_hex(flags, current)
        && !is_all_zero(trace_id)
        && !is_all_zero(parent_id)
        && bytes[35] == b'-'
        && bytes[52] == b'-';
    if !ids_valid {
        return None;
    }
    if current {
        return Some(value.to_owned());
    }
    // A newer version is rebuilt as the highest known version from the
    // fields this version defines; only the sampled flag carries over.
    let sampled = u8::from_str_radix(flags, 16).ok()? & 1;
    Some(format!(
        "00-{}-{}-{sampled:02x}",
        trace_id.to_ascii_lowercase(),
        parent_id.to_ascii_lowercase()
    ))
}

fn is_key_char(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-*/".contains(&byte)
}

fn is_valid_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    match key.split_once('@') {
        None => {
            (1..=256).contains(&bytes.len())
                && bytes[0].is_ascii_lowercase()
                && bytes.iter().copied().all(is_key_char)
        }
        Some((tenant, system)) => {
            let tenant = tenant.as_bytes();
            let system = system.as_bytes();
            (1..=241).contains(&tenant.len())
                && (tenant[0].is_ascii_lowercase() || tenant[0].is_ascii_digit())
                && tenant.iter().copied().all(is_key_char)
                && (1..=14).contains(&system.len())
                && system[0].is_ascii_lowercase()
                && system.iter().copied().all(is_key_char)
        }
    }
}

fn is_valid_value(value: &str) -> bool {
    let bytes = value.as_bytes();
    let is_nblk = |byte: u8| (0x21..=0x7e).contains(&byte) && byte != b',' && byte != b'=';
    (1..=256).contains(&bytes.len())
        && bytes[..bytes.len() - 1]
            .iter()
            .all(|&byte| byte == b' ' || is_nblk(byte))
        && is_nblk(bytes[bytes.len() - 1])
}

fn parse_tracestate(value: &str) -> Option<String> {
    let mut members: Vec<&str> = Vec::new();
    for raw in value.split(',') {
        let member = raw.trim_matches([' ', '\t']);
        if member.is_empty() {
            continue;
        }
        let (key, entry_value) = member.split_once('=')?;
        if !is_valid_key(key) || !is_valid_value(entry_value) {
            return None;
        }
        if members
            .iter()
            .any(|existing| existing.split_once('=').map(|(k, _)| k) == Some(key))
        {
            return None;
        }
        members.push(member);
    }
    if members.is_empty() || members.len() > TRACESTATE_MAX_MEMBERS {
        return None;
    }
    let combined_len = |members: &[&str]| {
        members.iter().map(|member| member.len()).sum::<usize>() + members.len().saturating_sub(1)
    };
    // Truncate whole entries: long entries first, then from the end.
    if combined_len(&members) > TRACESTATE_PROPAGATION_LIMIT {
        members.retain(|member| member.len() <= TRACESTATE_LONG_ENTRY);
    }
    while combined_len(&members) > TRACESTATE_PROPAGATION_LIMIT {
        members.pop();
    }
    (!members.is_empty()).then(|| members.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn valid_version_00_headers_are_kept() {
        let context =
            TraceContext::parse(VALID, Some("rojo=00f067aa0ba902b7,congo=t61rcWkgMzE")).unwrap();
        assert_eq!(context.traceparent(), VALID);
        assert_eq!(context.trace_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(context.parent_id(), "00f067aa0ba902b7");
        assert!(context.sampled());
        assert_eq!(
            context.tracestate(),
            Some("rojo=00f067aa0ba902b7,congo=t61rcWkgMzE")
        );
        assert!(!TraceContext::parse(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00",
            None
        )
        .unwrap()
        .sampled());
    }

    #[test]
    fn invalid_traceparent_values_are_ignored() {
        for value in [
            "",
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
            "0x-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00_4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "cc-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01.what",
        ] {
            assert!(
                TraceContext::parse(value, None).is_none(),
                "{value:?} must be ignored"
            );
        }
    }

    #[test]
    fn future_versions_are_reduced_to_version_00() {
        let context = TraceContext::parse(
            "cc-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-09-what-the-future-will-be-like",
            None,
        )
        .unwrap();
        assert_eq!(
            context.traceparent(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
        assert_eq!(
            TraceContext::parse(
                "cc-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-08",
                None
            )
            .unwrap()
            .traceparent(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00"
        );
    }

    #[test]
    fn invalid_tracestate_is_dropped_without_losing_traceparent() {
        for tracestate in [
            "Rojo=1",
            "rojo=1,rojo=2",
            "rojo=,congo=2",
            "rojo=a=b",
            "rojo",
            "1rojo=1",
            "tenant@=1",
            "tenant@toolongsystemname=1",
        ] {
            let context = TraceContext::parse(VALID, Some(tracestate)).unwrap();
            assert_eq!(context.traceparent(), VALID);
            assert!(
                context.tracestate().is_none(),
                "{tracestate:?} must be dropped"
            );
        }
        let members = (0..33)
            .map(|index| format!("k{index}=v"))
            .collect::<Vec<_>>();
        assert!(TraceContext::parse(VALID, Some(&members.join(",")))
            .unwrap()
            .tracestate()
            .is_none());
    }

    #[test]
    fn tracestate_whitespace_and_empty_members_are_normalized() {
        let context = TraceContext::parse(VALID, Some(" rojo=1 ,\t, congo@acme=x y ")).unwrap();
        assert_eq!(context.tracestate(), Some("rojo=1,congo@acme=x y"));
        assert!(TraceContext::parse(VALID, Some(" , "))
            .unwrap()
            .tracestate()
            .is_none());
    }

    #[test]
    fn oversized_tracestate_truncates_whole_entries() {
        let long = format!("long={}", "x".repeat(200));
        let mut members = vec![long.clone()];
        members.extend((0..30).map(|index| format!("k{index:02}={}", "v".repeat(12))));
        let context = TraceContext::parse(VALID, Some(&members.join(","))).unwrap();
        let kept = context.tracestate().unwrap();
        assert!(kept.len() <= TRACESTATE_PROPAGATION_LIMIT);
        assert!(!kept.contains(&long));
        assert!(kept.starts_with("k00="));
    }
}
