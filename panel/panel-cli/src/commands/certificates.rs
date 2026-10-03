//! Certificates of the inventory; their private keys stay with the panel.

use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub(crate) enum CertificateCommand {
    /// Every certificate with its names and validity.
    List,
    /// A certificate with its fingerprints and chain.
    Show { id: String },
    /// Uploads a PEM chain, leaf first, with the leaf's private key.
    Upload {
        /// Lowercase letters, digits and hyphens in labels separated by dots,
        /// such as example.com.
        id: String,
        /// File with the PEM certificate chain, leaf first.
        #[arg(long)]
        chain: PathBuf,
        /// File with the unencrypted PEM private key: PKCS#8, PKCS#1 or SEC1.
        #[arg(long)]
        key: PathBuf,
    },
    /// Generates a certificate signed by its own new ECDSA P-256 key.
    Generate {
        id: String,
        /// A DNS name, a wildcard such as *.example.com, or an IP address;
        /// repeatable.
        #[arg(long = "name", required = true)]
        names: Vec<String>,
        /// Days the certificate is valid, from 1 to 825.
        #[arg(long, default_value_t = 90)]
        days: u32,
    },
    /// Replaces a certificate's chain and key, such as with a renewed
    /// certificate; profiles that name it serve the new one.
    Replace {
        id: String,
        #[arg(long)]
        chain: PathBuf,
        #[arg(long)]
        key: PathBuf,
    },
    /// Deletes a certificate and its files on the gateway.
    Delete { id: String },
    /// Which of the given hosts a certificate covers.
    Check {
        id: String,
        /// A host name; repeatable.
        #[arg(long = "host", required = true)]
        hosts: Vec<String>,
    },
    /// Checks a chain, and its key when given, without storing anything.
    Inspect {
        #[arg(long)]
        chain: PathBuf,
        #[arg(long)]
        key: Option<PathBuf>,
    },
}

/// A SHA-256 fingerprint as browsers show it, `AB:CD:…`.
fn fingerprint(value: &Value) -> String {
    match value.as_str() {
        Some(hex) if !hex.is_empty() => hex
            .as_bytes()
            .chunks(2)
            .map(|pair| String::from_utf8_lossy(pair).to_uppercase())
            .collect::<Vec<_>>()
            .join(":"),
        _ => text(value),
    }
}

const CERTIFICATES: &[Column] = &[
    ("ID", |certificate| text(&certificate["id"])),
    ("NAMES", |certificate| text(&certificate["names"])),
    ("STATUS", |certificate| text(&certificate["status"])),
    ("NOT AFTER", |certificate| text(&certificate["not_after"])),
    ("ISSUER", |certificate| text(&certificate["issuer"])),
    ("SOURCE", |certificate| text(&certificate["source"])),
];

const CERTIFICATE: &[Column] = &[
    ("ID", |certificate| text(&certificate["id"])),
    ("SOURCE", |certificate| text(&certificate["source"])),
    ("STATUS", |certificate| text(&certificate["status"])),
    ("NAMES", |certificate| text(&certificate["names"])),
    ("SUBJECT", |certificate| text(&certificate["subject"])),
    ("ISSUER", |certificate| text(&certificate["issuer"])),
    ("SERIAL", |certificate| text(&certificate["serial"])),
    ("NOT BEFORE", |certificate| text(&certificate["not_before"])),
    ("NOT AFTER", |certificate| text(&certificate["not_after"])),
    ("KEY", |certificate| {
        format!(
            "{} {} bits",
            text(&certificate["key_algorithm"]),
            text(&certificate["key_bits"])
        )
    }),
    ("SHA-256", |certificate| {
        fingerprint(&certificate["fingerprint"])
    }),
    ("KEY SHA-256", |certificate| {
        fingerprint(&certificate["public_key_fingerprint"])
    }),
    ("CHAIN", |certificate| text(&certificate["chain_length"])),
    ("VERSION", |certificate| text(&certificate["version"])),
];

const INSPECTION: &[Column] = &[
    ("STATUS", |details| text(&details["status"])),
    ("KEY MATCHES", |details| text(&details["key_matches"])),
    ("NAMES", |details| text(&details["names"])),
    ("SUBJECT", |details| text(&details["subject"])),
    ("ISSUER", |details| text(&details["issuer"])),
    ("NOT AFTER", |details| text(&details["not_after"])),
    ("SHA-256", |details| fingerprint(&details["fingerprint"])),
];

const COVERAGE: &[Column] = &[
    ("HOST", |host| text(&host["host"])),
    ("COVERED", |host| text(&host["covered"])),
];

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|error| CliError::Usage(format!("cannot read {}: {error}", path.display())))
}

/// The certificate's current entity tag, for replacing or deleting it.
async fn etag(api: &Api, path: &str) -> Result<String> {
    api.get(path, &[])
        .await?
        .etag
        .ok_or_else(|| CliError::Usage(format!("{path} has no entity tag")))
}

pub async fn run(api: &Api, output: &Output, command: CertificateCommand) -> Result<()> {
    match command {
        CertificateCommand::List => output.list(
            &api.get("/api/v1/certificates", &[]).await?.body,
            CERTIFICATES,
        ),
        CertificateCommand::Show { id } => {
            let certificate = api
                .get(&format!("/api/v1/certificates/{id}"), &[])
                .await?
                .body;
            output.item(&certificate, CERTIFICATE);
        }
        CertificateCommand::Upload { id, chain, key } => {
            let body = json!({
                "source": "upload",
                "id": id,
                "chain": read(&chain)?,
                "key": read(&key)?,
            });
            let certificate = api
                .change(Method::POST, "/api/v1/certificates", Some(&body), None)
                .await?
                .body;
            output.done(&format!("Uploaded certificate {id}"), &certificate);
        }
        CertificateCommand::Generate { id, names, days } => {
            let body = json!({ "source": "self_signed", "id": id, "names": names, "days": days });
            let certificate = api
                .change(Method::POST, "/api/v1/certificates", Some(&body), None)
                .await?
                .body;
            output.done(&format!("Generated certificate {id}"), &certificate);
        }
        CertificateCommand::Replace { id, chain, key } => {
            let path = format!("/api/v1/certificates/{id}");
            let body = json!({ "chain": read(&chain)?, "key": read(&key)? });
            let tag = etag(api, &path).await?;
            let certificate = api
                .change(Method::PUT, &path, Some(&body), Some(&tag))
                .await?
                .body;
            output.done(&format!("Replaced certificate {id}"), &certificate);
        }
        CertificateCommand::Delete { id } => {
            let path = format!("/api/v1/certificates/{id}");
            let tag = etag(api, &path).await?;
            let reply = api
                .change(Method::DELETE, &path, None, Some(&tag))
                .await?
                .body;
            output.done(&format!("Deleted certificate {id}"), &reply);
        }
        CertificateCommand::Check { id, hosts } => {
            let coverage = api
                .get(
                    &format!("/api/v1/certificates/{id}/coverage"),
                    &[("hosts", hosts.join(","))],
                )
                .await?
                .body;
            match output.format {
                crate::output::Format::Json => output.json(&coverage),
                crate::output::Format::Table => output.list(&coverage["hosts"], COVERAGE),
            }
        }
        CertificateCommand::Inspect { chain, key } => {
            let mut body = json!({ "chain": read(&chain)? });
            if let Some(key) = key {
                body["key"] = json!(read(&key)?);
            }
            let details = api
                .post_read("/api/v1/certificate-inspections", &body)
                .await?
                .body;
            output.item(&details, INSPECTION);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_read_like_browsers_show_them() {
        assert_eq!(fingerprint(&json!("0a1bff")), "0A:1B:FF");
        assert_eq!(fingerprint(&Value::Null), "-");
    }
}
