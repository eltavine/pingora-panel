//! Delivery of certificates to the gateway's secret directory.

use panel_certificates::CertificateId;
use panel_errors::{PanelError, Result};
use std::{
    collections::BTreeSet,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const PREFIX: &str = "cert-";

/// A certificate as the gateway reads it.
pub struct Delivery {
    pub id: CertificateId,
    pub chain: String,
    pub key: Zeroizing<Vec<u8>>,
}

/// The gateway's secret directory. Certificates are written as
/// `cert-<id>.pem` and `cert-<id>.key`, readable only by their owner,
/// through a temporary file and a rename so the gateway never reads half a
/// file; files without the `cert-` prefix are never touched.
#[derive(Clone, Debug)]
pub struct SecretDirectory {
    path: PathBuf,
}

impl SecretDirectory {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn deliver(&self, delivery: Delivery) -> Result<()> {
        let path = self.path.clone();
        blocking(move || write_certificate(&path, &delivery)).await
    }

    pub async fn remove(&self, id: &CertificateId) -> Result<()> {
        let path = self.path.clone();
        let files = [id.key_file(), id.chain_file()];
        blocking(move || {
            for file in files {
                match fs::remove_file(path.join(file)) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
            }
            sync_directory(&path)
        })
        .await
    }

    /// Writes the certificates whose files differ from `certificates` and
    /// removes the files of certificates that no longer exist; returns how
    /// many files changed.
    pub async fn reconcile(&self, certificates: Vec<Delivery>) -> Result<usize> {
        let path = self.path.clone();
        blocking(move || {
            let mut changed = 0;
            let mut expected = BTreeSet::new();
            for delivery in &certificates {
                let chain = delivery.id.chain_file();
                let key = delivery.id.key_file();
                if !same(&path.join(&chain), delivery.chain.as_bytes())?
                    || !same(&path.join(&key), &delivery.key)?
                {
                    write_certificate(&path, delivery)?;
                    changed += 2;
                }
                expected.insert(chain);
                expected.insert(key);
            }
            for entry in fs::read_dir(&path)? {
                let name = entry?.file_name();
                let Some(name) = name.to_str() else { continue };
                let delivered =
                    name.starts_with(PREFIX) && (name.ends_with(".pem") || name.ends_with(".key"));
                if delivered && !expected.contains(name) {
                    fs::remove_file(path.join(name))?;
                    changed += 1;
                }
            }
            if changed > 0 {
                sync_directory(&path)?;
            }
            Ok(changed)
        })
        .await
    }
}

async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|_| PanelError::internal("certificate delivery stopped"))?
        .map_err(|error| {
            PanelError::unavailable(format!(
                "certificates cannot be delivered to the gateway's secret directory: {error}"
            ))
        })
}

/// The key goes first: a reader between the two renames sees a key that
/// does not match its chain and keeps what it had.
fn write_certificate(directory: &Path, delivery: &Delivery) -> io::Result<()> {
    write_atomically(directory, &delivery.id.key_file(), &delivery.key)?;
    write_atomically(
        directory,
        &delivery.id.chain_file(),
        delivery.chain.as_bytes(),
    )?;
    sync_directory(directory)
}

fn write_atomically(directory: &Path, name: &str, contents: &[u8]) -> io::Result<()> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(".pending-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o600));
    }
    let mut file = builder.tempfile_in(directory)?;
    file.write_all(contents)?;
    file.as_file().sync_all()?;
    file.persist(directory.join(name))
        .map_err(|error| error.error)?;
    Ok(())
}

fn same(path: &Path, contents: &[u8]) -> io::Result<bool> {
    match fs::read(path) {
        Ok(existing) => Ok(Zeroizing::new(existing).as_slice() == contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}
