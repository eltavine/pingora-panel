//! Plugin versions in the plugins directory, each `<name>/<version>/`
//! holding `plugin.json`, its executable and `plugin.json.minisig`, with
//! what keeps each from running (ADR 0044).

use crate::{
    manifest::{self, MOST_MANIFEST_BYTES},
    signature::{self, TrustedKey},
};
use plugin_contracts::v1::Manifest;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const MANIFEST_FILE: &str = "plugin.json";
pub const SIGNATURE_FILE: &str = "plugin.json.minisig";

/// A plugin version found, with what was learned of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub name: String,
    pub version: String,
    pub directory: PathBuf,
    pub manifest: Option<Manifest>,
    /// The ID of the trusted key that signed the manifest.
    pub signed_by: Option<String>,
    /// Why the version cannot run; empty when it is validated.
    pub problems: Vec<String>,
}

impl Found {
    pub fn is_valid(&self) -> bool {
        self.problems.is_empty() && self.manifest.is_some()
    }

    /// The executable's path, once the version is validated.
    pub fn executable(&self) -> Option<PathBuf> {
        let manifest = self.manifest.as_ref().filter(|_| self.is_valid())?;
        Some(self.directory.join(&manifest.executable))
    }
}

/// Every version under `root`, by name and then by SemVer precedence; a
/// missing directory holds none.
pub fn discover(root: &Path, keys: &[TrustedKey]) -> std::io::Result<Vec<Found>> {
    let mut found = Vec::new();
    let plugins = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(error),
    };
    for plugin in plugins {
        let plugin = plugin?;
        if !plugin.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = plugin.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        for version in std::fs::read_dir(plugin.path())? {
            let version = version?;
            if !version.file_type()?.is_dir() {
                continue;
            }
            let Some(number) = version.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            found.push(inspect(&name, &number, &version.path(), keys));
        }
    }
    found.sort_by(|left, right| {
        left.name.cmp(&right.name).then_with(|| {
            match (
                semver::Version::parse(&left.version),
                semver::Version::parse(&right.version),
            ) {
                (Ok(left), Ok(right)) => left.cmp(&right),
                _ => left.version.cmp(&right.version),
            }
        })
    });
    Ok(found)
}

/// What one version's directory holds and lacks.
pub fn inspect(name: &str, version: &str, directory: &Path, keys: &[TrustedKey]) -> Found {
    let mut found = Found {
        name: name.to_owned(),
        version: version.to_owned(),
        directory: directory.to_owned(),
        manifest: None,
        signed_by: None,
        problems: Vec::new(),
    };
    let bytes = match read_limited(&directory.join(MANIFEST_FILE)) {
        Ok(bytes) => bytes,
        Err(problem) => {
            found.problems.push(problem);
            return found;
        }
    };
    let manifest: Manifest = match serde_json::from_slice(&bytes) {
        Ok(manifest) => manifest,
        Err(error) => {
            found
                .problems
                .push(format!("plugin.json is not a manifest: {error}"));
            return found;
        }
    };
    found.problems.extend(manifest::problems(&manifest));
    if manifest.name != name || manifest.version != version {
        found.problems.push(format!(
            "plugin.json names {} {}, not the directory's {name} {version}",
            manifest.name, manifest.version
        ));
    }
    match std::fs::read_to_string(directory.join(SIGNATURE_FILE)) {
        Ok(signature) => match signature::signer(&bytes, &signature, keys) {
            Ok(key) => found.signed_by = Some(key.id.clone()),
            Err(problem) => found.problems.push(problem),
        },
        Err(_) => found
            .problems
            .push("plugin.json is not signed: plugin.json.minisig is missing".into()),
    }
    if let Err(problem) = executable(directory, &manifest) {
        found.problems.push(problem);
    }
    found.manifest = Some(manifest);
    found
}

fn read_limited(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("plugin.json cannot be read: {error}"))?;
    let mut bytes = Vec::new();
    file.take(MOST_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("plugin.json cannot be read: {error}"))?;
    if bytes.len() as u64 > MOST_MANIFEST_BYTES {
        return Err(format!(
            "plugin.json is larger than {MOST_MANIFEST_BYTES} bytes"
        ));
    }
    Ok(bytes)
}

/// Whether the manifest's executable is a file of the version's directory
/// with the named digest, which the host may run.
fn executable(directory: &Path, manifest: &Manifest) -> Result<(), String> {
    let relative = Path::new(&manifest.executable);
    if manifest.executable.is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "the executable {:?} is not a path inside the version's directory",
            manifest.executable
        ));
    }
    let path = directory.join(relative);
    let resolved = path.canonicalize().map_err(|error| {
        format!(
            "the executable {} cannot be found: {error}",
            manifest.executable
        )
    })?;
    let root = directory
        .canonicalize()
        .map_err(|error| format!("the version's directory cannot be read: {error}"))?;
    if !resolved.starts_with(&root) {
        return Err(format!(
            "the executable {} leads out of the version's directory",
            manifest.executable
        ));
    }
    let metadata = std::fs::metadata(&resolved).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err(format!(
            "the executable {} is not a file",
            manifest.executable
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "the executable {} may not be run",
                manifest.executable
            ));
        }
    }
    let digest = sha256(&resolved).map_err(|error| error.to_string())?;
    if digest != manifest.executable_sha256 {
        return Err(format!(
            "the executable's SHA-256 is {digest}, not the manifest's {}",
            manifest.executable_sha256
        ));
    }
    Ok(())
}

/// A file's SHA-256 in lowercase hexadecimal.
pub fn sha256(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
pub(crate) mod testing {
    //! Plugin versions written to a directory, signed with a test key.

    use super::*;
    use crate::signature::testing::SigningKey;

    pub fn install(root: &Path, manifest: &Manifest, key: &SigningKey) -> PathBuf {
        let directory = root.join(&manifest.name).join(&manifest.version);
        std::fs::create_dir_all(directory.join("bin")).unwrap();
        let executable = directory.join(&manifest.executable);
        std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let manifest = Manifest {
            executable_sha256: sha256(&executable).unwrap(),
            ..manifest.clone()
        };
        let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        std::fs::write(directory.join(MANIFEST_FILE), &bytes).unwrap();
        std::fs::write(directory.join(SIGNATURE_FILE), key.sign(&bytes)).unwrap();
        directory
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::install, *};
    use crate::{manifest::sample, signature::testing::SigningKey};

    fn trusted(key: &SigningKey) -> Vec<TrustedKey> {
        vec![TrustedKey {
            id: "acme".into(),
            public_key: key.public_key(),
            comment: String::new(),
        }]
    }

    #[test]
    fn versions_are_found_in_order_and_validated() {
        let root = tempfile::tempdir().unwrap();
        let key = SigningKey::new(3);
        for version in ["1.10.0", "1.2.0"] {
            install(
                root.path(),
                &Manifest {
                    version: version.into(),
                    ..sample()
                },
                &key,
            );
        }
        let found = discover(root.path(), &trusted(&key)).unwrap();
        let versions: Vec<&str> = found.iter().map(|found| found.version.as_str()).collect();
        assert_eq!(versions, ["1.2.0", "1.10.0"]);
        assert!(found.iter().all(Found::is_valid), "{found:#?}");
        assert_eq!(found[0].signed_by.as_deref(), Some("acme"));
        assert!(found[0].executable().unwrap().ends_with("bin/example"));
        assert!(discover(&root.path().join("missing"), &[])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn each_flaw_keeps_a_version_from_running() {
        let root = tempfile::tempdir().unwrap();
        let key = SigningKey::new(3);
        let directory = install(root.path(), &sample(), &key);

        let unsigned = inspect("example", "1.0.0", &directory, &[]);
        assert_eq!(
            unsigned.problems,
            ["plugin.json is not signed by a trusted key"]
        );

        std::fs::write(directory.join("bin/example"), b"#!/bin/sh\nexit 1\n").unwrap();
        let changed = inspect("example", "1.0.0", &directory, &trusted(&key));
        assert!(
            changed.problems[0].starts_with("the executable's SHA-256 is"),
            "{changed:#?}"
        );

        let renamed = inspect("other", "1.0.0", &directory, &trusted(&key));
        assert!(renamed
            .problems
            .iter()
            .any(|problem| problem.contains("not the directory's other")));

        std::fs::remove_file(directory.join(SIGNATURE_FILE)).unwrap();
        let bare = inspect("example", "1.0.0", &directory, &trusted(&key));
        assert!(bare
            .problems
            .iter()
            .any(|problem| problem.contains("minisig is missing")));

        let escaping = install(
            root.path(),
            &Manifest {
                version: "2.0.0".into(),
                executable: "../../outside".into(),
                ..sample()
            },
            &key,
        );
        let escaping = inspect("example", "2.0.0", &escaping, &trusted(&key));
        assert!(escaping
            .problems
            .iter()
            .any(|problem| problem.contains("not a path inside the version's directory")));
    }
}
