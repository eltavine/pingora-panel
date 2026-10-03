#![forbid(unsafe_code)]

use rustls::{
    crypto::ring::default_provider,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
    ServerConfig,
};
use std::{process::Command, sync::Arc};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// An HTTPS panel whose certificate the system does not trust: the CLI
/// speaks TLS to it and refuses the certificate rather than the scheme.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_panels_need_a_certificate_the_system_trusts() {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let config = ServerConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certified.cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                certified.signing_key.serialize_der(),
            )),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let _ = acceptor.accept(stream).await;
        }
    });
    let home = tempfile::tempdir().unwrap();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_ppanel"))
            .args([
                "--api",
                &format!("https://localhost:{port}"),
                "--token",
                "ppat_x",
                "whoami",
            ])
            .env("PPANEL_CONFIG_DIR", home.path())
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(6), "{stderr}");
    assert!(stderr.contains("certificate"), "{stderr}");
}
