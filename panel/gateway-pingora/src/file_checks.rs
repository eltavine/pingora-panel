//! What the gateway finds in the files it serves from: private keys that
//! others than their owner may read, and static roots or links that lead out
//! of the static content root. Links out of a root are not served; they are
//! reported so operators can remove them.

use crate::secrets::SecretSource;
use panel_ir::RuntimeSnapshot;
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Component, Path},
    time::SystemTime,
};

/// The most entries one static root check looks at.
pub const MAX_STATIC_ENTRIES: u32 = 10_000;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct FileChecks {
    pub checked_at: Option<SystemTime>,
    pub active_revision_id: Option<u64>,
    pub private_keys: Vec<PrivateKeyCheck>,
    pub static_roots: Vec<StaticRootCheck>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PrivateKeyCheck {
    /// The file name in the secret directory.
    pub file: String,
    /// The TLS profiles serving it.
    pub tls_profile_ids: Vec<String>,
    /// Unix permission bits, where the platform has them.
    pub mode: Option<u32>,
    /// Whether only the owner may read or write the file.
    pub owner_only: bool,
    /// Why the file could not be inspected.
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct StaticRootCheck {
    /// The static content the root serves.
    pub id: String,
    pub root: String,
    /// Whether the root resolves to a directory inside the static content
    /// root.
    pub inside: bool,
    /// Links below the root that lead out of it.
    pub escaping_links: Vec<EscapingLink>,
    pub entries_checked: u32,
    /// Whether the root holds more entries than one check looks at.
    pub truncated: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct EscapingLink {
    /// The link, relative to the root.
    pub path: String,
    /// Where it leads.
    pub target: String,
}

pub(crate) fn check(
    snapshot: Option<&RuntimeSnapshot>,
    secrets: &dyn SecretSource,
    static_root: Option<&Path>,
) -> FileChecks {
    let checked_at = Some(SystemTime::now());
    let Some(snapshot) = snapshot else {
        return FileChecks {
            checked_at,
            ..FileChecks::default()
        };
    };
    let mut keys: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for profile in &snapshot.tls_profiles {
        if !profile.private_key_secret_id.is_empty() {
            keys.entry(profile.private_key_secret_id.as_str())
                .or_default()
                .push(profile.id.clone());
        }
    }
    let private_keys = keys
        .into_iter()
        .map(|(file, tls_profile_ids)| {
            let mut check = PrivateKeyCheck {
                file: file.to_owned(),
                tls_profile_ids,
                ..PrivateKeyCheck::default()
            };
            match secrets.permissions(file) {
                Some(Ok(permissions)) => {
                    check.mode = permissions.mode;
                    check.owner_only = permissions.owner_only;
                }
                Some(Err(error)) => check.error = Some(error.message),
                None => check.error = Some("the gateway reads secrets it cannot inspect".into()),
            }
            check
        })
        .collect();
    let static_roots = snapshot
        .static_content
        .iter()
        .map(|policy| check_root(&policy.id, &policy.root, static_root))
        .collect();
    FileChecks {
        checked_at,
        active_revision_id: Some(snapshot.revision_id.get()),
        private_keys,
        static_roots,
    }
}

fn check_root(id: &str, root: &str, base: Option<&Path>) -> StaticRootCheck {
    let mut check = StaticRootCheck {
        id: id.to_owned(),
        root: root.to_owned(),
        ..StaticRootCheck::default()
    };
    let Some(base) = base else {
        check.error = Some("the gateway has no static content root".into());
        return check;
    };
    let relative = Path::new(root);
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        check.error = Some("the root is not a relative path without '.' or '..'".into());
        return check;
    }
    let (Ok(base), Ok(resolved)) = (base.canonicalize(), base.join(relative).canonicalize()) else {
        check.error = Some("the root does not exist".into());
        return check;
    };
    check.inside = resolved.starts_with(&base) && resolved.is_dir();
    if !check.inside {
        return check;
    }
    let mut pending = VecDeque::from([resolved.clone()]);
    while let Some(directory) = pending.pop_front() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if check.entries_checked == MAX_STATIC_ENTRIES {
                check.truncated = true;
                return check;
            }
            check.entries_checked += 1;
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push_back(path);
            } else if kind.is_symlink() {
                if let Ok(target) = path.canonicalize() {
                    if !target.starts_with(&resolved) {
                        check.escaping_links.push(EscapingLink {
                            path: path
                                .strip_prefix(&resolved)
                                .unwrap_or(&path)
                                .display()
                                .to_string(),
                            target: target.display().to_string(),
                        });
                    }
                }
            }
        }
    }
    check
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::secrets::DirectorySecrets;
    use panel_domain::RevisionId;
    use panel_ir::{StaticContentPolicy, TlsProfile};
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn loose_keys_and_links_out_of_roots_are_reported() {
        let secrets = tempfile::tempdir().unwrap();
        for (file, mode) in [("tight.key", 0o600), ("loose.key", 0o644)] {
            let path = secrets.path().join(file);
            std::fs::write(&path, "key").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let base = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let site = base.path().join("site");
        std::fs::create_dir_all(site.join("docs")).unwrap();
        std::fs::write(site.join("docs/page.html"), "page").unwrap();
        symlink(site.join("docs/page.html"), site.join("alias.html")).unwrap();
        symlink(outside.path(), site.join("docs/elsewhere")).unwrap();
        symlink(outside.path(), base.path().join("linked")).unwrap();

        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(7));
        for (id, key) in [
            ("edge", "tight.key"),
            ("legacy", "loose.key"),
            ("old", "missing.key"),
        ] {
            snapshot.tls_profiles.push(TlsProfile {
                id: id.into(),
                certificate_secret_id: "chain.pem".into(),
                private_key_secret_id: key.into(),
                min_protocol: "TLSv1.2".into(),
                max_protocol: None,
                cipher_suites: Vec::new(),
                session_resumption: true,
                alpn: std::collections::BTreeSet::new(),
            });
        }
        for (id, root) in [("site", "site"), ("linked", "linked"), ("gone", "gone")] {
            snapshot.static_content.push(StaticContentPolicy {
                listing: Default::default(),
                media_types: Default::default(),
                default_type: None,
                cache: Vec::new(),
                id: id.into(),
                root: root.into(),
                index_files: Vec::new(),
                spa_fallback: false,
            });
        }
        let checks = check(
            Some(&snapshot),
            &DirectorySecrets::new(secrets.path()),
            Some(base.path()),
        );
        assert_eq!(checks.active_revision_id, Some(7));
        let keys: Vec<_> = checks
            .private_keys
            .iter()
            .map(|key| {
                (
                    key.file.as_str(),
                    key.owner_only,
                    key.mode,
                    key.error.is_some(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("loose.key", false, Some(0o644), false),
                ("missing.key", false, None, true),
                ("tight.key", true, Some(0o600), false),
            ]
        );
        let site = &checks.static_roots[0];
        assert!(site.inside && !site.truncated);
        assert_eq!(site.escaping_links.len(), 1);
        assert_eq!(site.escaping_links[0].path, "docs/elsewhere");
        assert!(
            !checks.static_roots[1].inside,
            "a root linking out is reported"
        );
        assert!(checks.static_roots[2].error.is_some());
        assert!(check(None, &DirectorySecrets::new(secrets.path()), None)
            .private_keys
            .is_empty());
    }
}
