use crate::IssuedCredentials;
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use rustls_pki_types::{pem::PemObject, CertificateDer};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

/// A service's private key and certificate, as PEM.
pub const IDENTITY_FILE: &str = "identity.pem";
/// The certificates a service trusts for its peers.
pub const TRUST_FILE: &str = "trust.pem";

/// When a certificate is valid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Validity {
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
}

impl Validity {
    /// Renewal is due once two thirds of the lifetime has passed.
    pub fn renewal_due_at(&self) -> DateTime<Utc> {
        self.not_before + (self.not_after - self.not_before) * 2 / 3
    }
}

/// The credential files of one service.
#[derive(Clone, Debug)]
pub struct CredentialFiles {
    directory: PathBuf,
}

impl CredentialFiles {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn identity_path(&self) -> PathBuf {
        self.directory.join(IDENTITY_FILE)
    }

    pub fn trust_path(&self) -> PathBuf {
        self.directory.join(TRUST_FILE)
    }

    /// Replaces the trust bundle, then the identity, each with one atomic
    /// rename; the identity is readable by its owner only.
    pub fn write(&self, issued: &IssuedCredentials, trust_pem: &str) -> Result<()> {
        std::fs::create_dir_all(&self.directory).map_err(|error| {
            PanelError::storage_unavailable(format!(
                "cannot create {}: {error}",
                self.directory.display()
            ))
        })?;
        write_atomic(&self.trust_path(), trust_pem, false)?;
        let identity = format!("{}{}", issued.private_key_pem, issued.certificate_pem);
        write_atomic(&self.identity_path(), &identity, true)
    }

    /// The validity of the identity's certificate, if one is present and
    /// readable.
    pub fn validity(&self) -> Result<Option<Validity>> {
        let identity = match std::fs::read(self.identity_path()) {
            Ok(identity) => identity,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(PanelError::storage_unavailable(format!(
                    "cannot read {}: {error}",
                    self.identity_path().display()
                )))
            }
        };
        let Some(Ok(certificate)) = CertificateDer::pem_slice_iter(&identity).next() else {
            return Ok(None);
        };
        let Ok((_, certificate)) = x509_parser::parse_x509_certificate(&certificate) else {
            return Ok(None);
        };
        let validity = certificate.validity();
        let time =
            |time: x509_parser::time::ASN1Time| DateTime::from_timestamp(time.timestamp(), 0);
        Ok(time(validity.not_before).zip(time(validity.not_after)).map(
            |(not_before, not_after)| Validity {
                not_before,
                not_after,
            },
        ))
    }

    /// Whether the identity is missing, unreadable or due for renewal.
    pub fn renewal_due(&self, now: DateTime<Utc>) -> Result<bool> {
        Ok(self
            .validity()?
            .is_none_or(|validity| now >= validity.renewal_due_at()))
    }
}

/// Writes `contents` to a temporary file, syncs it and renames it over
/// `path`, so readers see either the old or the new contents.
pub(crate) fn write_atomic(path: &Path, contents: &str, private: bool) -> Result<()> {
    let failed = |error: std::io::Error| {
        PanelError::storage_unavailable(format!("cannot write {}: {error}", path.display()))
    };
    let directory = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = directory.join(format!(".{name}.tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if private { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options.open(&temporary).map_err(failed)?;
    file.write_all(contents.as_bytes()).map_err(failed)?;
    file.sync_all().map_err(failed)?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(failed)?;
    if let Ok(directory) = std::fs::File::open(directory) {
        let _ = directory.sync_all();
    }
    Ok(())
}
