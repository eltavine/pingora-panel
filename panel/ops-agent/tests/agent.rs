#![forbid(unsafe_code)]
#![cfg(unix)]

use chrono::Utc;
use ops_agent::AgentConfig;
use panel_context::ServiceName;
use panel_contracts::ops::v1::{
    agent_client::AgentClient, directories_client::DirectoriesClient, AgentDescribeRequest,
    Capability, CapabilityState, DirectoriesUsageRequest, DirectoryKind,
};
use panel_pki::{
    CertificateAuthority, CredentialFiles, TrustDomain, WorkloadIdentity,
    DEFAULT_AUTHORITY_VALIDITY,
};
use panel_tls::TlsCredentials;
use std::{os::unix::fs::MetadataExt, path::Path, time::Duration};
use tonic::{transport::Channel, Code};

fn issue(authority: &CertificateAuthority, root: &Path, service: &str) -> CredentialFiles {
    let files = CredentialFiles::new(root.join(service));
    let issued = authority
        .issue(
            &ServiceName::new(service).unwrap(),
            &[],
            Duration::from_secs(3600),
            Utc::now(),
        )
        .unwrap();
    files.write(&issued, authority.certificate_pem()).unwrap();
    files
}

fn channel(socket: &Path, files: CredentialFiles, service: &str) -> Channel {
    let identity =
        |name: &str| WorkloadIdentity::new(ServiceName::new(name).unwrap(), TrustDomain::default());
    panel_tls::unix_channel(
        socket,
        &identity("ops-agent"),
        TlsCredentials::load(files, identity(service)).unwrap(),
        Duration::from_secs(2),
        Duration::from_secs(5),
    )
    .unwrap()
}

#[tokio::test]
async fn the_agent_answers_panel_api_alone_within_its_configuration() {
    let root = tempfile::tempdir().unwrap();
    let (authority, _) = CertificateAuthority::load_or_create(
        &root.path().join("authority"),
        TrustDomain::default(),
        DEFAULT_AUTHORITY_VALIDITY,
        Utc::now(),
    )
    .unwrap();
    let agent = issue(&authority, root.path(), "ops-agent");
    let panel_api = issue(&authority, root.path(), "panel-api");
    let other = issue(&authority, root.path(), "config-service");
    let logs = root.path().join("logs");
    std::fs::create_dir(&logs).unwrap();
    std::fs::write(logs.join("access.log"), vec![b'x'; 42]).unwrap();
    let me = std::fs::metadata(root.path()).unwrap();
    let socket = root.path().join("a.sock");

    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(ops_agent::serve(
        AgentConfig {
            socket: socket.clone(),
            socket_group: Some(me.gid()),
            peer_users: vec![me.uid()],
            credentials: agent.directory().to_path_buf(),
            trust_domain: TrustDomain::default(),
            directories: vec![(DirectoryKind::Logs, logs.clone())],
            listeners: false,
            gateway_unit: None,
            engines: Vec::new(),
            state: None,
            installation_project: "pingora-panel".into(),
        },
        async move {
            let _ = stopped.await;
        },
    ));
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let mut client = AgentClient::new(channel(&socket, panel_api.clone(), "panel-api"));
    let description = client
        .describe(AgentDescribeRequest::default())
        .await
        .unwrap()
        .into_inner()
        .description
        .unwrap();
    let version = description.version.unwrap();
    assert_eq!(version.component, "ops-agent");
    assert_eq!(version.protocol, "pingora.panel.ops.v1@1..=1");
    assert_eq!(version.capability_set, "directories@1");
    let state = |capability| {
        description
            .capabilities
            .iter()
            .find(|status| status.capability() == capability)
            .map(|status| status.state())
    };
    assert_eq!(
        state(Capability::Directories),
        Some(CapabilityState::Available)
    );
    assert!(matches!(
        state(Capability::Listeners),
        Some(CapabilityState::NotEnabled | CapabilityState::Unsupported)
    ));

    let usage = DirectoriesClient::new(channel(&socket, panel_api, "panel-api"))
        .usage(DirectoriesUsageRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(usage.observed_at.is_some());
    let logs_usage = &usage.directories[0];
    assert_eq!(logs_usage.kind(), DirectoryKind::Logs);
    assert_eq!(logs_usage.path, logs.display().to_string());
    assert_eq!((logs_usage.bytes, logs_usage.files), (42, 1));

    let refused = AgentClient::new(channel(&socket, other, "config-service"))
        .describe(AgentDescribeRequest::default())
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::PermissionDenied);

    stop.send(()).unwrap();
    served.await.unwrap().unwrap();
    assert!(!socket.exists(), "the socket is removed on shutdown");
}
