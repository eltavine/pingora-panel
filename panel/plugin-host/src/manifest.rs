//! What a manifest must say before its plugin can run (ADR 0044).

use plugin_contracts::{v1::Manifest, PORTS, PROTOCOL_VERSION, SECRETS_CAPABILITY};
use std::time::Duration;

/// The application protocol versions this host speaks.
pub const HOST_PROTOCOL_VERSIONS: [u32; 1] = [PROTOCOL_VERSION];
/// A call's time unless the manifest or an administrator says.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest a call may take.
pub const MOST_CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// The largest `plugin.json` read.
pub const MOST_MANIFEST_BYTES: u64 = 1 << 20;

/// Whether `name` is a plugin name: lowercase letters, digits and hyphens,
/// starting and ending with a letter or digit, at most 64 characters.
pub fn is_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && bytes.first() != Some(&b'-')
        && bytes.last() != Some(&b'-')
}

/// Whether a plugin may ask for `capability`.
pub fn is_capability(capability: &str) -> bool {
    capability == SECRETS_CAPABILITY || PORTS.contains(&capability)
}

/// What is wrong with a manifest on its own, in the order found; nothing
/// when it may run. The executable, the signature and the directory are
/// checked by [`crate::catalog`].
pub fn problems(manifest: &Manifest) -> Vec<String> {
    let mut found = Vec::new();
    if !is_name(&manifest.name) {
        found.push(format!(
            "{:?} is not a plugin name: lowercase letters, digits and hyphens",
            manifest.name
        ));
    }
    if semver::Version::parse(&manifest.version).is_err() {
        found.push(format!(
            "{:?} is not a SemVer 2.0.0 version",
            manifest.version
        ));
    }
    if !manifest
        .protocol_versions
        .iter()
        .any(|version| HOST_PROTOCOL_VERSIONS.contains(version))
    {
        found.push(format!(
            "it speaks protocol versions {:?}, and the host {:?}",
            manifest.protocol_versions, HOST_PROTOCOL_VERSIONS
        ));
    }
    if manifest.ports.is_empty() {
        found.push("it provides no port".into());
    }
    for port in &manifest.ports {
        if !PORTS.contains(&port.as_str()) {
            found.push(format!("{port:?} is not a port: {}", PORTS.join(", ")));
        } else if !manifest.capabilities.contains(port) {
            found.push(format!("it provides {port} without asking for it"));
        }
    }
    for capability in &manifest.capabilities {
        if !is_capability(capability) {
            found.push(format!("{capability:?} is not a capability"));
        }
    }
    if Duration::from_millis(u64::from(manifest.call_timeout_ms)) > MOST_CALL_TIMEOUT {
        found.push(format!(
            "calls may take {} ms, more than {} ms",
            manifest.call_timeout_ms,
            MOST_CALL_TIMEOUT.as_millis()
        ));
    }
    if manifest.executable_sha256.len() != 64
        || !manifest
            .executable_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        found.push("the executable's SHA-256 is not 64 lowercase hexadecimal digits".into());
    }
    if let Err(problem) = crate::settings::schema(&manifest.config_schema) {
        found.push(problem);
    }
    found
}

/// Whether the running plugin describes itself as its signed manifest does.
pub fn describes(signed: &Manifest, running: &Manifest) -> Result<(), String> {
    let sorted = |ports: &[String]| {
        let mut ports = ports.to_vec();
        ports.sort();
        ports
    };
    if running.name != signed.name || running.version != signed.version {
        return Err(format!(
            "the process says it is {} {}, not {} {}",
            running.name, running.version, signed.name, signed.version
        ));
    }
    if sorted(&running.ports) != sorted(&signed.ports) {
        return Err(format!(
            "the process provides {:?}, not {:?}",
            running.ports, signed.ports
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn sample() -> Manifest {
    Manifest {
        name: "example".into(),
        version: "1.0.0".into(),
        executable: "bin/example".into(),
        executable_sha256: "0".repeat(64),
        protocol_versions: vec![1],
        ports: vec!["dns01".into()],
        capabilities: vec!["dns01".into(), SECRETS_CAPABILITY.into()],
        ..Manifest::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_lowercase_tokens() {
        for name in ["dns", "dns-01", "a1"] {
            assert!(is_name(name), "{name}");
        }
        for name in ["", "-dns", "dns-", "DNS", "dns_01", &"a".repeat(65)] {
            assert!(!is_name(name), "{name}");
        }
    }

    #[test]
    fn a_sound_manifest_has_no_problems_and_broken_ones_say_why() {
        assert_eq!(problems(&sample()), Vec::<String>::new());
        let broken = Manifest {
            name: "Example".into(),
            version: "one".into(),
            protocol_versions: vec![7],
            ports: vec!["dns01".into(), "ftp".into()],
            capabilities: vec!["root".into()],
            call_timeout_ms: 90_000,
            executable_sha256: "XYZ".into(),
            config_schema: "{".into(),
            ..sample()
        };
        let found = problems(&broken);
        for expected in [
            "\"Example\" is not a plugin name",
            "\"one\" is not a SemVer 2.0.0 version",
            "it speaks protocol versions [7]",
            "it provides dns01 without asking for it",
            "\"ftp\" is not a port",
            "\"root\" is not a capability",
            "calls may take 90000 ms",
            "the executable's SHA-256",
            "the configuration schema is not JSON",
        ] {
            assert!(
                found.iter().any(|problem| problem.starts_with(expected)),
                "{expected}: {found:#?}"
            );
        }
    }

    #[test]
    fn the_running_plugin_must_be_the_signed_one() {
        let signed = sample();
        assert!(describes(&signed, &signed).is_ok());
        let other = Manifest {
            version: "2.0.0".into(),
            ..sample()
        };
        assert!(describes(&signed, &other).unwrap_err().contains("2.0.0"));
        let more = Manifest {
            ports: vec!["dns01".into(), "secrets".into()],
            ..sample()
        };
        assert!(describes(&signed, &more).is_err());
    }
}
