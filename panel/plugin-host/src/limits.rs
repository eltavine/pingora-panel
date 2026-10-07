//! What a plugin's process may use: its manifest's needs, then what an
//! administrator sets, within the host's bounds (ADR 0044).

use crate::manifest::{DEFAULT_CALL_TIMEOUT, MOST_CALL_TIMEOUT};
use plugin_contracts::v1::Manifest;
use serde::{Deserialize, Serialize};

pub const DEFAULT_MEMORY_BYTES: u64 = 512 << 20;
pub const LEAST_MEMORY_BYTES: u64 = 16 << 20;
pub const MOST_MEMORY_BYTES: u64 = 16 << 30;
pub const DEFAULT_OPEN_FILES: u32 = 1024;
pub const MOST_OPEN_FILES: u32 = 65_536;
pub const DEFAULT_CONCURRENCY: u32 = 16;
pub const MOST_CONCURRENCY: u32 = 1024;
/// Whether this host enforces the limits on processes.
pub const ENFORCED: bool = cfg!(target_os = "linux");

/// Limits of a plugin; zero leaves a value to the manifest or the default.
/// CPU time has no default: a plugin runs as long as it is enabled.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub memory_bytes: u64,
    pub cpu_seconds: u32,
    pub open_files: u32,
    pub concurrency: u32,
    pub call_timeout_ms: u32,
}

fn first<T: PartialEq + Default + Copy>(values: [T; 2], default: T) -> T {
    values
        .into_iter()
        .find(|value| *value != T::default())
        .unwrap_or(default)
}

impl Limits {
    /// What `overrides` set, then what the manifest asks, then the defaults.
    pub fn effective(manifest: &Manifest, overrides: &Self) -> Self {
        let asked = manifest.resources.unwrap_or_default();
        Self {
            memory_bytes: first(
                [overrides.memory_bytes, asked.memory_bytes],
                DEFAULT_MEMORY_BYTES,
            )
            .clamp(LEAST_MEMORY_BYTES, MOST_MEMORY_BYTES),
            cpu_seconds: first([overrides.cpu_seconds, asked.cpu_seconds], 0),
            open_files: first([overrides.open_files, asked.open_files], DEFAULT_OPEN_FILES)
                .clamp(16, MOST_OPEN_FILES),
            concurrency: first(
                [overrides.concurrency, asked.concurrency],
                DEFAULT_CONCURRENCY,
            )
            .clamp(1, MOST_CONCURRENCY),
            call_timeout_ms: first(
                [overrides.call_timeout_ms, manifest.call_timeout_ms],
                DEFAULT_CALL_TIMEOUT.as_millis() as u32,
            )
            .clamp(1, MOST_CALL_TIMEOUT.as_millis() as u32),
        }
    }

    /// What is wrong with limits an administrator sets.
    pub fn problems(&self) -> Vec<String> {
        let mut found = Vec::new();
        if self.memory_bytes != 0
            && !(LEAST_MEMORY_BYTES..=MOST_MEMORY_BYTES).contains(&self.memory_bytes)
        {
            found.push(format!(
                "memory must be between {LEAST_MEMORY_BYTES} and {MOST_MEMORY_BYTES} bytes"
            ));
        }
        if self.open_files > MOST_OPEN_FILES || (self.open_files != 0 && self.open_files < 16) {
            found.push(format!(
                "open files must be between 16 and {MOST_OPEN_FILES}"
            ));
        }
        if self.concurrency > MOST_CONCURRENCY {
            found.push(format!("at most {MOST_CONCURRENCY} calls may be in flight"));
        }
        if u128::from(self.call_timeout_ms) > MOST_CALL_TIMEOUT.as_millis() {
            found.push(format!(
                "calls may take at most {} ms",
                MOST_CALL_TIMEOUT.as_millis()
            ));
        }
        found
    }
}

/// Bounds the process `pid`: its address space, its CPU time when set and
/// its open files, with no core dumps.
#[cfg(target_os = "linux")]
pub fn apply(pid: u32, limits: &Limits) -> std::io::Result<()> {
    use rustix::process::{prlimit, Pid, Resource, Rlimit};
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| std::io::Error::other("the plugin has no process ID"))?;
    let set = |resource: Resource, value: u64| {
        prlimit(
            Some(pid),
            resource,
            Rlimit {
                current: Some(value),
                maximum: Some(value),
            },
        )
        .map(|_| ())
        .map_err(std::io::Error::from)
    };
    set(Resource::As, limits.memory_bytes)?;
    if limits.cpu_seconds > 0 {
        set(Resource::Cpu, u64::from(limits.cpu_seconds))?;
    }
    set(Resource::Nofile, u64::from(limits.open_files))?;
    set(Resource::Core, 0)
}

/// Resource limits are enforced on Linux only; elsewhere they are reported
/// as not enforced.
#[cfg(not(target_os = "linux"))]
pub fn apply(_: u32, _: &Limits) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_contracts::v1::Resources;

    #[test]
    fn overrides_then_the_manifest_then_defaults_within_bounds() {
        let manifest = Manifest {
            resources: Some(Resources {
                memory_bytes: 128 << 20,
                concurrency: 4,
                ..Resources::default()
            }),
            call_timeout_ms: 5_000,
            ..Manifest::default()
        };
        let limits = Limits::effective(&manifest, &Limits::default());
        assert_eq!(
            limits,
            Limits {
                memory_bytes: 128 << 20,
                cpu_seconds: 0,
                open_files: DEFAULT_OPEN_FILES,
                concurrency: 4,
                call_timeout_ms: 5_000,
            }
        );
        let overridden = Limits::effective(
            &manifest,
            &Limits {
                memory_bytes: 1 << 40,
                call_timeout_ms: 30_000,
                ..Limits::default()
            },
        );
        assert_eq!(overridden.memory_bytes, MOST_MEMORY_BYTES);
        assert_eq!(overridden.call_timeout_ms, 30_000);
        assert!(Limits::default().problems().is_empty());
        assert_eq!(
            Limits {
                memory_bytes: 1,
                open_files: 1,
                concurrency: 5_000,
                call_timeout_ms: 61_000,
                ..Limits::default()
            }
            .problems()
            .len(),
            4
        );
    }
}
