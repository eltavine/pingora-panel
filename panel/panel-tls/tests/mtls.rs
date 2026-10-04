#![forbid(unsafe_code)]

use chrono::{Duration as Span, Utc};
use panel_context::ServiceName;
use panel_pki::{
    CertificateAuthority, CredentialFiles, TrustDomain, WorkloadIdentity,
    DEFAULT_AUTHORITY_VALIDITY,
};
use panel_tls::{channel, incoming, PeerPolicy, TlsCredentials};
use rustls_pki_types::{CertificateDer, ServerName};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::net::{TcpListener, TcpStream};
use tonic::transport::Server;
use tonic_health::pb::{health_client::HealthClient, HealthCheckRequest};

struct Installation {
    root: PathBuf,
    authority: CertificateAuthority,
}

impl Installation {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "panel-tls-{name}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let (authority, _) = CertificateAuthority::load_or_create(
            &root.join("authority"),
            TrustDomain::default(),
            DEFAULT_AUTHORITY_VALIDITY,
            Utc::now(),
        )
        .unwrap();
        Self { root, authority }
    }

    fn credentials(&self, service: &str) -> Arc<TlsCredentials> {
        let files = self.issue(service);
        TlsCredentials::load(files, identity(service)).unwrap()
    }

    fn issue(&self, service: &str) -> CredentialFiles {
        let files = CredentialFiles::new(self.root.join(service));
        let issued = self
            .authority
            .issue(
                &ServiceName::new(service).unwrap(),
                &[],
                Duration::from_secs(3600),
                Utc::now(),
            )
            .unwrap();
        files
            .write(&issued, self.authority.certificate_pem())
            .unwrap();
        files
    }
}

impl Drop for Installation {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn identity(service: &str) -> WorkloadIdentity {
    WorkloadIdentity::new(ServiceName::new(service).unwrap(), TrustDomain::default())
}

async fn serve(credentials: Arc<TlsCredentials>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    tokio::spawn(
        Server::builder()
            .layer(PeerPolicy::new(TrustDomain::default()))
            .add_service(health)
            .serve_with_incoming(incoming(listener, credentials, Duration::from_secs(5))),
    );
    address
}

async fn check(address: &str, peer: &str, client: Arc<TlsCredentials>) -> Result<(), String> {
    let channel = channel(
        address,
        &identity(peer),
        client,
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .unwrap();
    HealthClient::new(channel)
        .check(HealthCheckRequest {
            service: String::new(),
        })
        .await
        .map(drop)
        .map_err(|status| format!("{status:?}"))
}

/// The certificate the server presents to a fresh connection.
async fn presented(address: &str, client: &Arc<TlsCredentials>) -> CertificateDer<'static> {
    let tcp = TcpStream::connect(address).await.unwrap();
    let connector = tokio_rustls::TlsConnector::from(client.client_config());
    let stream = connector
        .connect(
            ServerName::try_from(identity("config-service").dns_name()).unwrap(),
            tcp,
        )
        .await
        .unwrap();
    stream.get_ref().1.peer_certificates().unwrap()[0]
        .clone()
        .into_owned()
}

#[tokio::test]
async fn services_authenticate_each_other_by_identity() {
    let installation = Installation::new("identity");
    let address = serve(installation.credentials("config-service")).await;
    let client = installation.credentials("panel-api");
    check(&address, "config-service", Arc::clone(&client))
        .await
        .unwrap();

    let impostor = check(&address, "gatewayd", Arc::clone(&client)).await;
    assert!(impostor.is_err(), "the server is not the gateway");

    let foreign = Installation::new("foreign");
    assert!(
        check(&address, "config-service", foreign.credentials("panel-api"))
            .await
            .is_err()
    );

    let anonymous = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .with_root_certificates({
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(
                rustls_pki_types::pem::PemObject::from_pem_slice(
                    installation.authority.certificate_pem().as_bytes(),
                )
                .unwrap(),
            )
            .unwrap();
        roots
    })
    .with_no_client_auth();
    let tcp = TcpStream::connect(&address).await.unwrap();
    let handshake = tokio_rustls::TlsConnector::from(Arc::new(anonymous))
        .connect(
            ServerName::try_from(identity("config-service").dns_name()).unwrap(),
            tcp,
        )
        .await;
    let refused = match handshake {
        Err(_) => true,
        Ok(mut stream) => {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let _ = stream.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").await;
            let mut buffer = [0u8; 1];
            !matches!(stream.read(&mut buffer).await, Ok(1))
        }
    };
    assert!(refused, "a client without a certificate is refused");
}

#[cfg(unix)]
async fn serve_unix(path: &std::path::Path, users: Vec<u32>, credentials: Arc<TlsCredentials>) {
    let listener = tokio::net::UnixListener::bind(path).unwrap();
    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    tokio::spawn(
        Server::builder()
            .layer(PeerPolicy::new(TrustDomain::default()))
            .add_service(health)
            .serve_with_incoming(panel_tls::incoming_unix(
                listener,
                users,
                credentials,
                Duration::from_secs(5),
            )),
    );
}

#[cfg(unix)]
async fn check_unix(
    path: &std::path::Path,
    peer: &str,
    client: Arc<TlsCredentials>,
) -> Result<(), String> {
    let channel = panel_tls::unix_channel(
        path,
        &identity(peer),
        client,
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .unwrap();
    HealthClient::new(channel)
        .check(HealthCheckRequest {
            service: String::new(),
        })
        .await
        .map(drop)
        .map_err(|status| format!("{status:?}"))
}

#[cfg(unix)]
#[tokio::test]
async fn unix_sockets_admit_listed_users_then_authenticate_identities() {
    use std::os::unix::fs::MetadataExt;

    let installation = Installation::new("unix");
    let server = installation.credentials("ops-agent");
    let client = installation.credentials("panel-api");
    let me = std::fs::metadata(&installation.root).unwrap().uid();

    let admitted = installation.root.join("a.sock");
    serve_unix(&admitted, vec![me], Arc::clone(&server)).await;
    check_unix(&admitted, "ops-agent", Arc::clone(&client))
        .await
        .unwrap();
    assert!(
        check_unix(&admitted, "gatewayd", Arc::clone(&client))
            .await
            .is_err(),
        "the server is not the gateway"
    );

    let refused = installation.root.join("r.sock");
    serve_unix(&refused, vec![me.wrapping_add(1)], server).await;
    assert!(
        check_unix(&refused, "ops-agent", client).await.is_err(),
        "a user off the list is refused before its handshake"
    );
}

#[tokio::test]
async fn rotated_credentials_reach_new_connections_without_a_restart() {
    let installation = Installation::new("rotation");
    let server = installation.credentials("config-service");
    let address = serve(Arc::clone(&server)).await;
    let client = installation.credentials("panel-api");
    let before = presented(&address, &client).await;
    assert!(
        !server.reload().unwrap(),
        "unchanged files are not reloaded"
    );

    tokio::time::sleep(Duration::from_millis(20)).await;
    installation.issue("config-service");
    assert!(server.reload().unwrap());
    let after = presented(&address, &client).await;
    assert_ne!(before, after);
    check(&address, "config-service", client).await.unwrap();

    let validity = CredentialFiles::new(installation.root.join("config-service"))
        .validity()
        .unwrap()
        .unwrap();
    assert!(validity.not_after > Utc::now() + Span::minutes(50));
}
