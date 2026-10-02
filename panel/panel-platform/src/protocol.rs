use panel_errors::{PanelError, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The revisions of one protocol major version that an implementation speaks.
///
/// A protocol is a versioned Protobuf package such as
/// `pingora.panel.gateway.v1`. Within a major version every revision only
/// adds to the previous one, so two parties can use the highest revision both
/// support. An implementation raises `min_revision` only after the revisions
/// below it have been retired.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RangeFields", into = "RangeFields")]
pub struct ProtocolRange {
    name: String,
    min_revision: u32,
    max_revision: u32,
}

#[derive(Serialize, Deserialize)]
struct RangeFields {
    name: String,
    min_revision: u32,
    max_revision: u32,
}

impl TryFrom<RangeFields> for ProtocolRange {
    type Error = PanelError;

    fn try_from(value: RangeFields) -> Result<Self> {
        Self::new(value.name, value.min_revision, value.max_revision)
    }
}

impl From<ProtocolRange> for RangeFields {
    fn from(value: ProtocolRange) -> Self {
        Self {
            name: value.name,
            min_revision: value.min_revision,
            max_revision: value.max_revision,
        }
    }
}

impl ProtocolRange {
    pub fn new(name: impl Into<String>, min_revision: u32, max_revision: u32) -> Result<Self> {
        let name = name.into();
        if !is_versioned_package(&name) {
            return Err(PanelError::invalid_argument(format!(
                "protocol `{name}` is not a versioned package such as `pingora.panel.gateway.v1`"
            )));
        }
        if min_revision == 0 || min_revision > max_revision {
            return Err(PanelError::invalid_argument(format!(
                "protocol `{name}` revisions must satisfy 1 <= min <= max"
            )));
        }
        Ok(Self {
            name,
            min_revision,
            max_revision,
        })
    }

    /// Speaks revisions `1..=revision`.
    pub fn up_to(name: impl Into<String>, revision: u32) -> Result<Self> {
        Self::new(name, 1, revision)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn min_revision(&self) -> u32 {
        self.min_revision
    }

    pub fn max_revision(&self) -> u32 {
        self.max_revision
    }

    /// The highest revision both ranges speak.
    pub fn negotiate(&self, remote: &ProtocolRange) -> Result<NegotiatedProtocol> {
        if self.name != remote.name {
            return Err(PanelError::unsupported_capability(format!(
                "cannot negotiate `{}` with `{}`",
                self.name, remote.name
            )));
        }
        let lowest = self.min_revision.max(remote.min_revision);
        let highest = self.max_revision.min(remote.max_revision);
        if lowest > highest {
            return Err(PanelError::unsupported_capability(format!(
                "no common revision of `{}`: local {self}, remote {remote}",
                self.name
            )));
        }
        Ok(NegotiatedProtocol {
            name: self.name.clone(),
            revision: highest,
        })
    }

    /// Negotiates with the matching range among those a peer advertises.
    pub fn negotiate_with_any(&self, remote: &[ProtocolRange]) -> Result<NegotiatedProtocol> {
        remote
            .iter()
            .find(|range| range.name == self.name)
            .ok_or_else(|| {
                PanelError::unsupported_capability(format!("peer does not speak `{}`", self.name))
            })?
            .negotiate(self)
    }
}

impl fmt::Display for ProtocolRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}@{}..={}",
            self.name, self.min_revision, self.max_revision
        )
    }
}

/// The revision of a protocol two parties agreed to use.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct NegotiatedProtocol {
    name: String,
    revision: u32,
}

impl NegotiatedProtocol {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn revision(&self) -> u32 {
        self.revision
    }

    /// Whether additions made in `revision` may be relied on.
    pub fn supports(&self, revision: u32) -> bool {
        self.revision >= revision
    }
}

/// Dot-separated lowercase identifiers ending in a `v<major>` segment.
fn is_versioned_package(name: &str) -> bool {
    let segments = name.split('.').collect::<Vec<_>>();
    let identifier = |segment: &str| {
        segment
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    };
    let major = segments
        .last()
        .and_then(|segment| segment.strip_prefix('v'));
    name.len() <= 128
        && segments.len() >= 2
        && segments.iter().all(|segment| identifier(segment))
        && major.is_some_and(|major| {
            !major.is_empty()
                && !major.starts_with('0')
                && major.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_errors::ErrorCode;

    const GATEWAY: &str = "pingora.panel.gateway.v1";

    #[test]
    fn negotiation_picks_the_highest_common_revision() {
        let client = ProtocolRange::up_to(GATEWAY, 3).unwrap();
        let server = ProtocolRange::new(GATEWAY, 2, 5).unwrap();
        let agreed = client.negotiate(&server).unwrap();
        assert_eq!(agreed.revision(), 3);
        assert!(agreed.supports(2) && agreed.supports(3) && !agreed.supports(4));
        assert_eq!(server.negotiate(&client).unwrap(), agreed);
    }

    #[test]
    fn disjoint_ranges_and_unknown_protocols_fail_fast() {
        let old_client = ProtocolRange::up_to(GATEWAY, 1).unwrap();
        let server = ProtocolRange::new(GATEWAY, 2, 2).unwrap();
        let error = old_client.negotiate(&server).unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);
        assert!(error
            .message
            .contains("local pingora.panel.gateway.v1@1..=1"));

        let platform = ProtocolRange::up_to("pingora.panel.platform.v1", 1).unwrap();
        let error = platform.negotiate_with_any(&[server]).unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);
    }

    #[test]
    fn ranges_name_a_versioned_package_and_an_ordered_revision_span() {
        for name in [
            "pingora.panel.gateway.v1",
            "a.v12",
            "pingora.panel.common_types.v2",
        ] {
            assert!(ProtocolRange::up_to(name, 1).is_ok(), "{name}");
        }
        for name in [
            "gateway",
            "v1",
            "pingora.panel.gateway",
            "pingora.Panel.v1",
            "a.v0",
            "a.v01",
            "a..v1",
        ] {
            assert!(ProtocolRange::up_to(name, 1).is_err(), "{name}");
        }
        assert!(ProtocolRange::new(GATEWAY, 0, 1).is_err());
        assert!(ProtocolRange::new(GATEWAY, 3, 2).is_err());
        assert!(serde_json::from_str::<ProtocolRange>(
            r#"{"name":"pingora.panel.gateway.v1","min_revision":2,"max_revision":1}"#
        )
        .is_err());
    }
}
