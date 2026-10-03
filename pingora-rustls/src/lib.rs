// Copyright 2026 Cloudflare, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! This module contains all the rustls specific pingora integration for things
//! like loading certificates and private keys

#![warn(clippy::all)]

use std::fs::File;
use std::io;
use std::path::Path;

use log::warn;
pub use no_debug::{Ellipses, NoDebug, WithTypeInfo};
use pingora_error::{Error, ErrorType, OrErr, Result};

pub use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
pub use rustls::server::{
    ClientCertVerifierBuilder, ClientHello, ResolvesServerCert, WebPkiClientVerifier,
};
pub use rustls::sign;
pub use rustls::{
    client::WebPkiServerVerifier, crypto::CryptoProvider, version, CertificateError, ClientConfig,
    DigitallySignedStruct, Error as RusTlsError, KeyLogFile, RootCertStore, ServerConfig,
    SignatureScheme, Stream,
};

/// Install the default `ring` CryptoProvider for rustls.
///
/// rustls 0.23+ requires an explicit provider. This function installs `ring`
/// as the process-level default. Safe to call multiple times — subsequent
/// calls are no-ops.
pub fn install_default_crypto_provider() {
    let _ = CryptoProvider::install_default(rustls::crypto::ring::default_provider());
}
use rustls_pki_types::pem::{self, PemObject, SectionKind};
pub use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
pub use tokio_rustls::client::TlsStream as ClientTlsStream;
pub use tokio_rustls::server::TlsStream as ServerTlsStream;
pub use tokio_rustls::{Accept, Connect, TlsAcceptor, TlsConnector, TlsStream};

// This allows to skip certificate verification. Be highly cautious.
pub use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};

/// Open the given file from disk and use the pingora Error type instead of the
/// std::io version
fn load_file<P>(path: P) -> Result<File>
where
    P: AsRef<Path>,
{
    File::open(path).or_err(ErrorType::FileReadError, "Failed to load file")
}

/// Read every PEM object of type `T` from the file at the given path, skipping
/// sections of other types
fn load_pem_objects<T, P>(path: P, context: &'static str) -> Result<Vec<T>>
where
    T: PemObject,
    P: AsRef<Path>,
{
    T::pem_reader_iter(load_file(path)?)
        .map(|item_res| item_res.or_err(ErrorType::InvalidCert, context))
        .collect()
}

/// Read the first private key of the file at the given path, if it has one
fn load_private_key<P>(path: P, context: &'static str) -> Result<Option<PrivateKeyDer<'static>>>
where
    P: AsRef<Path>,
{
    match PrivateKeyDer::from_pem_reader(load_file(path)?) {
        Ok(key) => Ok(Some(key)),
        Err(pem::Error::NoItemsFound) => Ok(None),
        Err(e) => Err(e).or_err(ErrorType::InvalidCert, context),
    }
}

/// Load the platform's trusted root certificates, including those named by the
/// `SSL_CERT_FILE` and `SSL_CERT_DIR` environment variables.
///
/// Certificates that load are returned even if others fail, and each failure is
/// logged; an error is returned only when failures leave nothing to return.
pub fn load_native_certs() -> io::Result<Vec<CertificateDer<'static>>> {
    let loaded = rustls_native_certs::load_native_certs();
    for error in &loaded.errors {
        warn!("Failed to load native certificates: {error}");
    }
    match loaded.errors.into_iter().next() {
        Some(error) if loaded.certs.is_empty() => Err(io::Error::other(error)),
        _ => Ok(loaded.certs),
    }
}

/// Load the certificates from the given pem file path into the given
/// certificate store
pub fn load_ca_file_into_store<P>(path: P, cert_store: &mut RootCertStore) -> Result<()>
where
    P: AsRef<Path>,
{
    let sections: Vec<(SectionKind, Vec<u8>)> =
        load_pem_objects(path, "Certificate in pem file could not be read")?;
    for (kind, content) in sections {
        // only loading certificates, handling a CA file
        let SectionKind::Certificate = kind else {
            return Error::e_explain(
                ErrorType::InvalidCert,
                "Pem file contains un-loadable certificate type",
            );
        };
        cert_store.add(CertificateDer::from(content)).or_err(
            ErrorType::InvalidCert,
            "Failed to load X509 certificate into root store",
        )?;
    }

    Ok(())
}

/// Attempt to load the native cas into the given root-certificate store
pub fn load_platform_certs_incl_env_into_store(ca_certs: &mut RootCertStore) -> Result<()> {
    // this includes handling of ENV vars SSL_CERT_FILE & SSL_CERT_DIR
    for cert in load_native_certs()
        .or_err(ErrorType::InvalidCert, "Failed to load native certificates")?
        .into_iter()
    {
        ca_certs.add(cert).or_err(
            ErrorType::InvalidCert,
            "Failed to load native certificate into root store",
        )?;
    }

    Ok(())
}

/// Load the certificates and private key files
pub fn load_certs_and_key_files<'a>(
    cert: &str,
    key: &str,
) -> Result<Option<(Vec<CertificateDer<'a>>, PrivateKeyDer<'a>)>> {
    let certs: Vec<CertificateDer<'static>> =
        load_pem_objects(cert, "Certificate in pem file could not be read")?;
    // PKCS#1, PKCS#8 and SEC1 keys are the supported private key types
    let private_key_opt = load_private_key(key, "Certificate in pem file could not be read")?;

    if let (Some(private_key), false) = (private_key_opt, certs.is_empty()) {
        Ok(Some((certs, private_key)))
    } else {
        Ok(None)
    }
}

/// Load the certificate
pub fn load_pem_file_ca(path: &String) -> Result<Vec<u8>> {
    let cas: Vec<CertificateDer<'static>> =
        load_pem_objects(path, "Failed to load certificate from file")?;

    Ok(cas.first().map(|ca| ca.to_vec()).unwrap_or_default())
}

pub fn load_pem_file_private_key(path: &String) -> Result<Vec<u8>> {
    Ok(
        load_private_key(path, "Failed to load private key from file")?
            .map(|key| key.secret_der().to_vec())
            .unwrap_or_default(),
    )
}

pub fn hash_certificate(cert: &CertificateDer) -> Vec<u8> {
    let hash = ring::digest::digest(&ring::digest::SHA256, cert.as_ref());
    hash.as_ref().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_file(name: &str) -> String {
        format!(
            "{}/../pingora-core/tests/keys/{name}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    #[test]
    fn loads_certificates_and_keys() {
        let (certs, key) = load_certs_and_key_files(&key_file("server.crt"), &key_file("key.pem"))
            .unwrap()
            .unwrap();
        assert_eq!(certs.len(), 1);
        assert!(matches!(key, PrivateKeyDer::Sec1(_)));
        assert_eq!(
            load_pem_file_ca(&key_file("server.crt")).unwrap(),
            certs[0].to_vec()
        );
        assert_eq!(
            load_pem_file_private_key(&key_file("key.pem")).unwrap(),
            key.secret_der()
        );
    }

    #[test]
    fn absent_items_are_empty_rather_than_errors() {
        assert!(
            load_certs_and_key_files(&key_file("server.crt"), &key_file("public.pem"))
                .unwrap()
                .is_none()
        );
        assert!(load_pem_file_private_key(&key_file("public.pem"))
            .unwrap()
            .is_empty());
        assert!(load_pem_file_ca(&key_file("key.pem")).unwrap().is_empty());
    }

    #[test]
    fn ca_files_hold_only_certificates() {
        let mut store = RootCertStore::empty();
        load_ca_file_into_store(key_file("server.crt"), &mut store).unwrap();
        assert_eq!(store.len(), 1);
        for other in ["key.pem", "public.pem", "server.csr"] {
            let error = load_ca_file_into_store(key_file(other), &mut store).unwrap_err();
            assert_eq!(error.etype(), &ErrorType::InvalidCert);
        }
        let missing = load_ca_file_into_store(key_file("missing.crt"), &mut store).unwrap_err();
        assert_eq!(missing.etype(), &ErrorType::FileReadError);
    }
}
