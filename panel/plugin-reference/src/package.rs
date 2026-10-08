//! The reference plugin as a plugin version: its manifest, its executable
//! and a signature made as minisign makes one, Ed25519 over the BLAKE2b-512
//! of `plugin.json`, then over that signature and its trusted comment.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use blake2::{Blake2b512, Digest as _};
use plugin_contracts::{
    v1::{Manifest, Resources},
    PORTS, PROTOCOL_VERSION, SECRETS_CAPABILITY,
};
use ring::{
    rand::{SecureRandom, SystemRandom},
    signature::{Ed25519KeyPair, KeyPair},
};
use sha2::Sha256;
use std::path::{Path, PathBuf};

pub const NAME: &str = "reference";
pub const EXECUTABLE: &str = "bin/pingora-panel-reference-plugin";

/// The settings the reference plugin takes.
pub const SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "delay_ms": {"type": "integer", "minimum": 0, "maximum": 120000},
    "refuse": {"type": "boolean"},
    "exit_after_ms": {"type": "integer", "minimum": 1},
    "token": {"type": "string", "format": "secret-reference"},
    "secrets": {"type": "object", "additionalProperties": {"type": "string"}},
    "containers": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["name", "image"],
        "additionalProperties": false,
        "properties": {
          "name": {"type": "string"},
          "image": {"type": "string"},
          "running": {"type": "boolean"}
        }
      }
    }
  }
}"#;

/// A publisher's minisign key.
pub struct Publisher {
    pair: Ed25519KeyPair,
    id: [u8; 8],
}

impl Publisher {
    /// A new key from the system's random numbers.
    pub fn generate() -> Self {
        let random = SystemRandom::new();
        let mut seed = [0; 32];
        let mut id = [0; 8];
        random
            .fill(&mut seed)
            .and_then(|()| random.fill(&mut id))
            .expect("the system provides random numbers");
        Self::from_seed(seed, id)
    }

    pub fn from_seed(seed: [u8; 32], id: [u8; 8]) -> Self {
        Self {
            pair: Ed25519KeyPair::from_seed_unchecked(&seed).expect("any 32 bytes are a seed"),
            id,
        }
    }

    /// The public key's Base64 line, as in `minisign.pub`.
    pub fn public_key(&self) -> String {
        let mut bytes = b"Ed".to_vec();
        bytes.extend_from_slice(&self.id);
        bytes.extend_from_slice(self.pair.public_key().as_ref());
        STANDARD.encode(bytes)
    }

    /// The key's ID as minisign prints it.
    pub fn key_id(&self) -> String {
        format!("{:016X}", u64::from_le_bytes(self.id))
    }

    /// A `.minisig` file for `content`, named `file` in its trusted comment.
    pub fn sign(&self, content: &[u8], file: &str) -> String {
        let signature = self.pair.sign(&Blake2b512::digest(content));
        let mut line = b"ED".to_vec();
        line.extend_from_slice(&self.id);
        line.extend_from_slice(signature.as_ref());
        let comment = format!("file:{file}");
        let mut global = signature.as_ref().to_vec();
        global.extend_from_slice(comment.as_bytes());
        format!(
            "untrusted comment: signature from pingora-panel-reference-plugin\n{}\ntrusted comment: {comment}\n{}\n",
            STANDARD.encode(line),
            STANDARD.encode(self.pair.sign(&global))
        )
    }
}

/// The reference plugin's manifest at `version`, before its executable's
/// digest is known.
pub fn manifest(version: &str) -> Manifest {
    let mut capabilities: Vec<String> = PORTS.iter().map(|port| (*port).to_owned()).collect();
    capabilities.push(SECRETS_CAPABILITY.to_owned());
    Manifest {
        name: NAME.into(),
        version: version.into(),
        publisher: "Pingora Panel".into(),
        description: "Provides every port, keeping what it is given in its own directory, to check an installation and as an example.".into(),
        executable: EXECUTABLE.into(),
        executable_sha256: String::new(),
        protocol_versions: vec![PROTOCOL_VERSION],
        ports: PORTS.iter().map(|port| (*port).to_owned()).collect(),
        capabilities,
        resources: Some(Resources {
            memory_bytes: 512 << 20,
            cpu_seconds: 0,
            open_files: 256,
            concurrency: 8,
        }),
        call_timeout_ms: 10_000,
        config_schema: SCHEMA.into(),
        homepage: String::new(),
    }
}

/// Installs `executable` as `manifest`'s version under `root`, signed by
/// `publisher`, and returns the version's directory.
pub fn install(
    root: &Path,
    executable: &Path,
    manifest: &Manifest,
    publisher: &Publisher,
) -> std::io::Result<PathBuf> {
    let directory = root.join(&manifest.name).join(&manifest.version);
    let target = directory.join(&manifest.executable);
    std::fs::create_dir_all(target.parent().unwrap_or(&directory))?;
    std::fs::copy(executable, &target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
    }
    let manifest = Manifest {
        executable_sha256: hex::encode(Sha256::digest(std::fs::read(&target)?)),
        ..manifest.clone()
    };
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    std::fs::write(directory.join("plugin.json"), &bytes)?;
    std::fs::write(
        directory.join("plugin.json.minisig"),
        publisher.sign(&bytes, "plugin.json"),
    )?;
    Ok(directory)
}
