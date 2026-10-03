#![forbid(unsafe_code)]

//! Black-box traffic through the data plane: snapshot in, HTTP on the wire.

use base64::Engine;
use gateway_pingora::{
    AdapterOptions, ChallengeDirectory, DataPlane, DataPlaneOptions, DirectorySecrets,
    PingoraGatewayAdapter,
};
use panel_domain::{
    EndpointAddress, EndpointId, NormalizedHost, PathPrefix, RevisionId, RouteId, SiteId,
    UpstreamPoolId,
};
use panel_engine::DataPlaneAdapter;
use panel_ir::{
    template::TEMPLATE_CAPABILITY, CapabilityRequirement, DomainSpec, ListenerRef, RouteAction,
    RouteMatcher, RouteSpec, RuntimeSnapshot, SiteSpec, StaticContentPolicy,
    StrictTransportSecurity, TlsProfile, UpstreamEndpoint, UpstreamPoolSpec, WwwRedirect,
};
use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    num::NonZeroUsize,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

fn free_address() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// An upstream that answers every request with its request head.
async fn echo_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    head.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(&head).await;
            });
        }
    });
    address
}

struct Response {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn exchange(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), request: &str) -> Response {
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response head");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Response {
        status,
        headers,
        body: raw[split + 4..].to_vec(),
    }
}

async fn get(address: SocketAddr, host: Option<&str>, target: &str, extra: &str) -> Response {
    let host = host
        .map(|host| format!("host: {host}\r\n"))
        .unwrap_or_default();
    let mut stream = TcpStream::connect(address).await.unwrap();
    exchange(
        &mut stream,
        &format!("GET {target} HTTP/1.1\r\n{host}{extra}connection: close\r\n\r\n"),
    )
    .await
}

async fn wait_for(address: SocketAddr) {
    for _ in 0..200 {
        if TcpStream::connect(address).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{address} never accepted connections");
}

struct Gateway {
    adapter: Arc<PingoraGatewayAdapter>,
    plane: Arc<DataPlane>,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl Gateway {
    async fn start(options: AdapterOptions, snapshot: RuntimeSnapshot) -> Self {
        let adapter = Arc::new(PingoraGatewayAdapter::with_options(options));
        Self::activate(&adapter, snapshot).await;
        let plane = DataPlane::new(
            Arc::clone(&adapter),
            DataPlaneOptions::new(NonZeroUsize::new(2).unwrap())
                .with_drain_timeout(Duration::from_secs(2)),
        );
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(Arc::clone(&plane).run(async {
            let _ = stopped.await;
        }));
        Self {
            adapter,
            plane,
            stop: Some(stop),
            task,
        }
    }

    async fn activate(adapter: &PingoraGatewayAdapter, mut snapshot: RuntimeSnapshot) {
        snapshot.refresh_content_hash();
        let prepared = adapter.prepare(snapshot).await.unwrap();
        adapter.activate(Arc::new(prepared));
    }

    async fn stop(mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.task.await.unwrap();
    }
}

fn route(id: &str, priority: u32, matcher: RouteMatcher, action: RouteAction) -> RouteSpec {
    RouteSpec::new(
        RouteId::new(id).unwrap(),
        SiteId::new("site").unwrap(),
        priority,
        matcher,
        action,
    )
}

fn prefix(path: &str) -> RouteMatcher {
    RouteMatcher::PathPrefix {
        path: PathPrefix::new(path).unwrap(),
    }
}

fn pool(id: &str, upstreams: &[SocketAddr]) -> UpstreamPoolSpec {
    UpstreamPoolSpec::new(
        UpstreamPoolId::new(id).unwrap(),
        id,
        upstreams
            .iter()
            .enumerate()
            .map(|(index, address)| {
                UpstreamEndpoint::new(
                    EndpointId::new(format!("node-{index}")).unwrap(),
                    EndpointAddress::new(address.ip().to_string(), address.port(), false).unwrap(),
                )
            })
            .collect(),
    )
}

fn proxy(pool: &str) -> RouteAction {
    RouteAction::Proxy {
        upstream_pool_id: UpstreamPoolId::new(pool).unwrap(),
    }
}

fn site(hosts: &[&str]) -> SiteSpec {
    SiteSpec::new(
        SiteId::new("site").unwrap(),
        "site",
        hosts
            .iter()
            .map(|host| DomainSpec::new(NormalizedHost::new(host).unwrap()))
            .collect(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxies_redirects_responds_and_rejects_on_plain_http() {
    let upstream = echo_upstream().await;
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    let mut main = site(&["example.com", "www.example.com", "alias.example.net"]);
    main.domains[0].primary = true;
    main.domains[2].redirect_to_primary = true;
    main.www_redirect = WwwRedirect::RemoveWww;
    snapshot.sites.push(main);
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    snapshot.routes = vec![
        route("app", 10, prefix("/"), proxy("app")),
        route(
            "moved",
            1,
            RouteMatcher::ExactPath {
                path: "/old".into(),
            },
            RouteAction::Redirect {
                location: "https://docs.example.com".into(),
                status: 301,
                preserve_path: true,
            },
        ),
        route(
            "maintenance",
            1,
            prefix("/maintenance"),
            RouteAction::Respond {
                status: 503,
                body: Some("back soon".into()),
                content_type: None,
                retry_after_seconds: Some(120),
            },
        ),
        route(
            "whoami",
            1,
            RouteMatcher::ExactPath {
                path: "/whoami".into(),
            },
            RouteAction::respond(
                200,
                Some(
                    "$method $scheme://$host$uri from $client_ip id=$request_id tenant=$http_x_tenant costs $$5"
                        .into(),
                ),
            ),
        ),
        route(
            "landing",
            1,
            RouteMatcher::ExactPath { path: "/go".into() },
            RouteAction::redirect("https://$host/landing$uri", 302),
        ),
    ];
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new(TEMPLATE_CAPABILITY, "1"));
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let response = get(listen, Some("Example.COM"), "/a/../hello?x=1", "").await;
    assert_eq!(response.status, 200);
    let seen = String::from_utf8(response.body)
        .unwrap()
        .to_ascii_lowercase();
    assert!(seen.starts_with("get /a/../hello?x=1 http/1.1"), "{seen}");
    assert!(seen.contains("x-forwarded-for: 127.0.0.1"), "{seen}");
    assert!(seen.contains("x-forwarded-proto: http"), "{seen}");
    assert!(
        seen.contains("forwarded: for=127.0.0.1;host=example.com;proto=http"),
        "{seen}"
    );
    assert!(seen.contains("via: 1.1 pingora-panel"), "{seen}");

    let response = get(listen, Some("unknown.example"), "/", "").await;
    assert_eq!(response.status, 421);
    assert_eq!(get(listen, None, "/", "").await.status, 400);
    assert_eq!(
        get(listen, Some("example.com"), "/", "host: example.com\r\n")
            .await
            .status,
        400
    );

    let response = get(listen, Some("www.example.com"), "/path?q=1", "").await;
    assert_eq!(response.status, 308);
    assert_eq!(response.headers["location"], "http://example.com/path?q=1");
    let response = get(listen, Some("alias.example.net:8080"), "/x", "").await;
    assert_eq!(response.status, 308);
    assert_eq!(response.headers["location"], "http://example.com:8080/x");

    let response = get(listen, Some("example.com"), "/old?y=2", "").await;
    assert_eq!(response.status, 301);
    assert_eq!(
        response.headers["location"],
        "https://docs.example.com/old?y=2"
    );

    let response = get(listen, Some("example.com"), "/maintenance", "").await;
    assert_eq!(response.status, 503);
    assert_eq!(response.headers["retry-after"], "120");
    assert_eq!(response.body, b"back soon");

    let response = get(
        listen,
        Some("Example.com"),
        "/whoami?q=1",
        "x-request-id: req-42\r\nx-tenant: acme\r\n",
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(
        String::from_utf8(response.body).unwrap(),
        "GET http://example.com/whoami from 127.0.0.1 id=req-42 tenant=acme costs $5"
    );
    let response = get(listen, Some("example.com"), "/go?q=1", "").await;
    assert_eq!(response.status, 302);
    assert_eq!(
        response.headers["location"],
        "https://example.com/landing/go"
    );

    gateway.stop().await;
    assert!(TcpStream::connect(listen).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_site_failover_reload_and_h2c() {
    let upstream = echo_upstream().await;
    let dead = free_address();
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let mut listener = ListenerRef::new("http", listen.to_string());
    listener.default_site_id = Some(SiteId::new("site").unwrap());
    snapshot.listeners.push(listener);
    snapshot.sites.push(site(&["example.com"]));
    let mut app = pool("app", &[dead, upstream]);
    app.endpoints[0].weight = 100;
    snapshot.upstream_pools.push(app);
    snapshot
        .routes
        .push(route("app", 1, prefix("/"), proxy("app")));
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    for _ in 0..3 {
        assert_eq!(
            get(listen, Some("anything.test"), "/", "").await.status,
            200
        );
    }
    let health = gateway.adapter.upstream_health();
    assert!(health[0].endpoints[0].failures >= 1);
    assert!(health[0].endpoints[1].requests >= 3);

    let before = gateway.plane.status().generation;
    let status = gateway.plane.reload().await.unwrap();
    assert_eq!(status.generation, before + 1);
    assert_eq!(get(listen, Some("example.com"), "/", "").await.status, 200);
    let status = gateway
        .plane
        .set_workers(NonZeroUsize::new(1).unwrap())
        .await
        .unwrap();
    assert_eq!(status.workers, 1);
    assert_eq!(status.generation, before + 2);

    let stream = TcpStream::connect(listen).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http2::handshake(
        hyper_util::rt::TokioExecutor::new(),
        hyper_util::rt::TokioIo::new(stream),
    )
    .await
    .unwrap();
    tokio::spawn(connection);
    let request = http::Request::get("http://example.com/h2c")
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.version(), http::Version::HTTP_2);
    drop(sender);
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_content_honours_conditionals_and_ranges() {
    let base = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(base.path().join("site/docs")).unwrap();
    std::fs::write(base.path().join("site/index.html"), "<h1>home</h1>").unwrap();
    std::fs::write(base.path().join("site/docs/a.txt"), "0123456789").unwrap();
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.sites.push(site(&["static.test"]));
    snapshot.static_content.push(StaticContentPolicy {
        id: "files".into(),
        root: "site".into(),
        index_files: vec!["index.html".into()],
        spa_fallback: false,
    });
    snapshot.routes.push(route(
        "files",
        1,
        prefix("/"),
        RouteAction::Static {
            policy_id: "files".into(),
        },
    ));
    let gateway = Gateway::start(
        AdapterOptions::default().with_static_root(base.path()),
        snapshot,
    )
    .await;
    wait_for(listen).await;

    let response = get(listen, Some("static.test"), "/docs/a.txt", "").await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"0123456789");
    assert_eq!(
        response.headers["content-type"],
        "text/plain; charset=utf-8"
    );
    let etag = response.headers["etag"].clone();
    let response = get(
        listen,
        Some("static.test"),
        "/docs/a.txt",
        &format!("if-none-match: {etag}\r\n"),
    )
    .await;
    assert_eq!(response.status, 304);
    assert!(response.body.is_empty());
    let response = get(
        listen,
        Some("static.test"),
        "/docs/a.txt",
        "range: bytes=2-4\r\n",
    )
    .await;
    assert_eq!(response.status, 206);
    assert_eq!(response.body, b"234");
    assert_eq!(response.headers["content-range"], "bytes 2-4/10");
    let response = get(
        listen,
        Some("static.test"),
        "/docs/a.txt",
        "range: bytes=50-\r\n",
    )
    .await;
    assert_eq!(response.status, 416);
    let response = get(listen, Some("static.test"), "/", "").await;
    assert_eq!(response.body, b"<h1>home</h1>");
    let response = get(listen, Some("static.test"), "/docs", "").await;
    assert_eq!(response.status, 301);
    assert_eq!(response.headers["location"], "/docs/");
    assert_eq!(
        get(listen, Some("static.test"), "/%2e%2e/%2e%2e/etc/passwd", "")
            .await
            .status,
        404
    );
    assert_eq!(
        get(listen, Some("static.test"), "/missing", "")
            .await
            .status,
        404
    );
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_http01_challenges_are_answered_before_the_site() {
    let upstream = echo_upstream().await;
    let challenges = tempfile::tempdir().unwrap();
    std::fs::write(challenges.path().join("Token_1-a"), "Token_1-a.thumbprint").unwrap();
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    snapshot.sites.push(site(&["shop.test"]));
    snapshot
        .routes
        .push(route("all", 1, prefix("/"), proxy("app")));
    let gateway = Gateway::start(
        AdapterOptions::default().with_challenges(ChallengeDirectory::new(challenges.path())),
        snapshot,
    )
    .await;
    wait_for(listen).await;

    for host in ["shop.test", "unknown.test"] {
        let answer = get(
            listen,
            Some(host),
            "/.well-known/acme-challenge/Token_1-a",
            "",
        )
        .await;
        assert_eq!(answer.status, 200, "{host}");
        assert_eq!(answer.body, b"Token_1-a.thumbprint");
        assert_eq!(answer.headers["content-type"], "application/octet-stream");
    }
    let passed = get(
        listen,
        Some("shop.test"),
        "/.well-known/acme-challenge/other",
        "",
    )
    .await;
    assert_eq!(passed.status, 200);
    assert!(passed
        .body
        .starts_with(b"GET /.well-known/acme-challenge/other "));
    gateway.stop().await;
}

fn pem(label: &str, der: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<_> = encoded
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!(
        "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
        lines.join("\n")
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tls_listener_selects_certificates_by_sni_and_rejects_misdirected_hosts() {
    let upstream = echo_upstream().await;
    let secrets = tempfile::tempdir().unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for (id, names) in [
        ("main", vec!["example.com"]),
        ("other", vec!["other.example"]),
    ] {
        let certified = rcgen::generate_simple_self_signed(
            names.into_iter().map(String::from).collect::<Vec<_>>(),
        )
        .unwrap();
        roots.add(certified.cert.der().clone()).unwrap();
        std::fs::write(
            secrets.path().join(format!("{id}.crt")),
            pem("CERTIFICATE", certified.cert.der()),
        )
        .unwrap();
        std::fs::write(
            secrets.path().join(format!("{id}.key")),
            pem("PRIVATE KEY", &certified.signing_key.serialize_der()),
        )
        .unwrap();
    }
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    for id in ["main", "other"] {
        snapshot.tls_profiles.push(TlsProfile {
            id: id.into(),
            certificate_secret_id: format!("{id}.crt"),
            private_key_secret_id: format!("{id}.key"),
            min_protocol: "TLSv1.2".into(),
            max_protocol: None,
            cipher_suites: Vec::new(),
            session_resumption: true,
            alpn: BTreeSet::new(),
        });
    }
    let mut listener = ListenerRef::new("https", listen.to_string());
    listener.tls_profile_id = Some("main".into());
    snapshot.listeners.push(listener);
    let mut main = site(&["example.com", "other.example"]);
    main.domains[1].tls_profile_id = Some("other".into());
    snapshot.sites.push(main);
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    snapshot
        .routes
        .push(route("app", 1, prefix("/"), proxy("app")));
    let gateway = Gateway::start(
        AdapterOptions::default().with_secrets(Arc::new(DirectorySecrets::new(secrets.path()))),
        snapshot,
    )
    .await;
    wait_for(listen).await;

    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let connect = |name: &'static str| {
        let connector = connector.clone();
        async move {
            let stream = TcpStream::connect(listen).await.unwrap();
            connector
                .connect(
                    rustls_pki_types::ServerName::try_from(name).unwrap(),
                    stream,
                )
                .await
                .unwrap()
        }
    };

    let mut stream = connect("example.com").await;
    let response = exchange(
        &mut stream,
        "GET / HTTP/1.1\r\nhost: example.com\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(response.status, 200);
    let seen = String::from_utf8(response.body).unwrap();
    assert!(seen.contains("x-forwarded-proto: https"), "{seen}");

    // The handshake succeeds only if the SNI-selected certificate is presented.
    let mut stream = connect("other.example").await;
    let response = exchange(
        &mut stream,
        "GET / HTTP/1.1\r\nhost: other.example\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(response.status, 200);

    let mut stream = connect("example.com").await;
    let response = exchange(
        &mut stream,
        "GET / HTTP/1.1\r\nhost: other.example\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(response.status, 421);
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn renewed_certificates_are_served_without_a_new_revision() {
    let upstream = echo_upstream().await;
    let secrets = tempfile::tempdir().unwrap();
    let issue = || rcgen::generate_simple_self_signed(vec!["example.com".to_owned()]).unwrap();
    let (first, renewed, stranger) = (issue(), issue(), issue());
    let write = |certified: &rcgen::CertifiedKey<rcgen::KeyPair>| {
        std::fs::write(
            secrets.path().join("cert-edge.key"),
            pem("PRIVATE KEY", &certified.signing_key.serialize_der()),
        )
        .unwrap();
        std::fs::write(
            secrets.path().join("cert-edge.pem"),
            pem("CERTIFICATE", certified.cert.der()),
        )
        .unwrap();
    };
    write(&first);
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot.tls_profiles.push(TlsProfile {
        id: "edge".into(),
        certificate_secret_id: "cert-edge.pem".into(),
        private_key_secret_id: "cert-edge.key".into(),
        min_protocol: "TLSv1.2".into(),
        max_protocol: None,
        cipher_suites: Vec::new(),
        session_resumption: true,
        alpn: BTreeSet::new(),
    });
    let mut listener = ListenerRef::new("https", listen.to_string());
    listener.tls_profile_id = Some("edge".into());
    snapshot.listeners.push(listener);
    snapshot.sites.push(site(&["example.com"]));
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    snapshot
        .routes
        .push(route("app", 1, prefix("/"), proxy("app")));
    let gateway = Gateway::start(
        AdapterOptions::default().with_secrets(Arc::new(DirectorySecrets::new(secrets.path()))),
        snapshot,
    )
    .await;
    wait_for(listen).await;

    let mut roots = rustls::RootCertStore::empty();
    for certified in [&first, &renewed] {
        roots.add(certified.cert.der().clone()).unwrap();
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let presented = || {
        let connector = connector.clone();
        async move {
            let stream = TcpStream::connect(listen).await.unwrap();
            let stream = connector
                .connect(
                    rustls_pki_types::ServerName::try_from("example.com").unwrap(),
                    stream,
                )
                .await
                .unwrap();
            stream.get_ref().1.peer_certificates().unwrap()[0].to_vec()
        }
    };

    assert_eq!(presented().await, first.cert.der().to_vec());
    assert!(!gateway.adapter.reload_certificates().await.unwrap());

    write(&renewed);
    assert!(gateway.adapter.reload_certificates().await.unwrap());
    assert_eq!(presented().await, renewed.cert.der().to_vec());

    std::fs::write(
        secrets.path().join("cert-edge.key"),
        pem("PRIVATE KEY", &stranger.signing_key.serialize_der()),
    )
    .unwrap();
    let refused = gateway.adapter.reload_certificates().await.unwrap_err();
    assert!(
        refused.message.contains("does not match"),
        "{}",
        refused.message
    );
    assert_eq!(presented().await, renewed.cert.der().to_vec());
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listener_tls_settings_narrow_handshakes_and_hsts_reaches_https() {
    let upstream = echo_upstream().await;
    let secrets = tempfile::tempdir().unwrap();
    let certified = rcgen::generate_simple_self_signed(vec!["example.com".to_owned()]).unwrap();
    std::fs::write(
        secrets.path().join("edge.crt"),
        pem("CERTIFICATE", certified.cert.der()),
    )
    .unwrap();
    std::fs::write(
        secrets.path().join("edge.key"),
        pem("PRIVATE KEY", &certified.signing_key.serialize_der()),
    )
    .unwrap();
    let (https, http) = (free_address(), free_address());
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot.tls_profiles.push(TlsProfile {
        id: "edge".into(),
        certificate_secret_id: "edge.crt".into(),
        private_key_secret_id: "edge.key".into(),
        min_protocol: "TLSv1.2".into(),
        max_protocol: Some("TLSv1.2".into()),
        cipher_suites: vec!["TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256".into()],
        session_resumption: true,
        alpn: BTreeSet::new(),
    });
    let mut tls = ListenerRef::new("https", https.to_string());
    tls.tls_profile_id = Some("edge".into());
    snapshot.listeners.push(tls);
    snapshot
        .listeners
        .push(ListenerRef::new("http", http.to_string()));
    let mut main = site(&["example.com"]);
    main.hsts = Some(StrictTransportSecurity {
        max_age_seconds: 31_536_000,
        include_subdomains: true,
        preload: false,
    });
    snapshot.sites.push(main);
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    snapshot
        .routes
        .push(route("app", 1, prefix("/app"), proxy("app")));
    let gateway = Gateway::start(
        AdapterOptions::default().with_secrets(Arc::new(DirectorySecrets::new(secrets.path()))),
        snapshot,
    )
    .await;
    wait_for(https).await;
    wait_for(http).await;

    let mut roots = rustls::RootCertStore::empty();
    roots.add(certified.cert.der().clone()).unwrap();
    let connector = |versions: &[&'static rustls::SupportedProtocolVersion]| {
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(versions)
        .unwrap()
        .with_root_certificates(roots.clone())
        .with_no_client_auth();
        tokio_rustls::TlsConnector::from(Arc::new(config))
    };
    let both = connector(&[&rustls::version::TLS12, &rustls::version::TLS13]);
    let connect = || {
        let both = both.clone();
        async move {
            both.connect(
                rustls_pki_types::ServerName::try_from("example.com").unwrap(),
                TcpStream::connect(https).await.unwrap(),
            )
            .await
            .unwrap()
        }
    };

    let mut stream = connect().await;
    let (_, connection) = stream.get_ref();
    assert_eq!(
        connection.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_2)
    );
    assert_eq!(
        connection.negotiated_cipher_suite().unwrap().suite(),
        rustls::CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256
    );
    let policy = "max-age=31536000; includeSubDomains";
    let proxied = exchange(
        &mut stream,
        "GET /app HTTP/1.1\r\nhost: example.com\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(proxied.status, 200);
    assert_eq!(proxied.headers["strict-transport-security"], policy);
    let mut stream = connect().await;
    let generated = exchange(
        &mut stream,
        "GET /missing HTTP/1.1\r\nhost: example.com\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(generated.status, 404);
    assert_eq!(generated.headers["strict-transport-security"], policy);

    let newest_only = connector(&[&rustls::version::TLS13])
        .connect(
            rustls_pki_types::ServerName::try_from("example.com").unwrap(),
            TcpStream::connect(https).await.unwrap(),
        )
        .await;
    assert!(newest_only.is_err());

    let plain = get(http, Some("example.com"), "/app", "").await;
    assert_eq!(plain.status, 200);
    assert!(!plain.headers.contains_key("strict-transport-security"));
    gateway.stop().await;
}
