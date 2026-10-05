#![forbid(unsafe_code)]

//! Black-box traffic through the data plane: snapshot in, HTTP on the wire.

use base64::Engine;
use gateway_pingora::{
    register_configuration, AdapterOptions, ChallengeDirectory, DataPlane, DataPlaneOptions,
    DirectorySecrets, GatewayMetrics, Logs, PingoraGatewayAdapter,
};
use panel_domain::{
    EndpointAddress, EndpointId, NormalizedHost, PathPrefix, RevisionId, RouteId, SiteId,
    UpstreamPoolId,
};
use panel_engine::DataPlaneAdapter;
use panel_ir::{
    logging::LOGGING_CAPABILITY, template::TEMPLATE_CAPABILITY, AccessLogFormat, BasicAuth,
    CapabilityRequirement, CircuitBreaker, DomainSpec, ListenerRef, RateLimit, RateLimitKey,
    RefererRule, RetryBudget, RetryCondition, RetryPolicy, RouteAction, RouteMatcher, RouteSpec,
    RuntimeSnapshot, SecurityPolicy, SiteSpec, StaticContentPolicy, StrictTransportSecurity,
    TlsProfile, UpstreamEndpoint, UpstreamPoolSpec, UpstreamQueue, WwwRedirect,
    REQUEST_SECURITY_CAPABILITY, UPSTREAM_RESILIENCE_CAPABILITY,
};
use panel_metrics::Metrics;
use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    num::NonZeroUsize,
    path::Path,
    sync::{
        atomic::{AtomicU16, AtomicUsize, Ordering::SeqCst},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

/// A free loopback address no other test here was given, as the system may
/// hand out a port again once it is released.
fn free_address() -> SocketAddr {
    static GIVEN: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());
    loop {
        let address = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let mut given = GIVEN.lock().unwrap();
        if !given.contains(&address.port()) {
            given.push(address.port());
            return address;
        }
    }
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

/// An upstream that reads whatever it is sent and never answers.
async fn silent_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0; 1024];
                while stream.read(&mut buffer).await.unwrap_or(0) > 0 {}
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
        Self::start_with(options, snapshot, |plane| plane).await
    }

    async fn start_with(
        options: AdapterOptions,
        snapshot: RuntimeSnapshot,
        configure: impl FnOnce(DataPlaneOptions) -> DataPlaneOptions,
    ) -> Self {
        let adapter = Arc::new(PingoraGatewayAdapter::with_options(options));
        Self::activate(&adapter, snapshot).await;
        let plane_options = configure(
            DataPlaneOptions::new(NonZeroUsize::new(2).unwrap())
                .with_drain_timeout(Duration::from_secs(2)),
        );
        let plane = DataPlane::new(Arc::clone(&adapter), plane_options);
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

/// Waits for the metrics to contain `sample`, as requests are measured after
/// their responses are sent.
async fn measured(metrics: &Metrics, sample: &str) {
    for _ in 0..200 {
        if metrics.encode().contains(sample) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no sample {sample} in\n{}", metrics.encode());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_and_upstream_attempts_are_measured() {
    let upstream = echo_upstream().await;
    let unreachable = free_address();
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.sites.push(site(&["example.com"]));
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    snapshot.upstream_pools.push(pool("down", &[unreachable]));
    snapshot.routes = vec![
        route("app", 10, prefix("/"), proxy("app")),
        route("down", 1, prefix("/down"), proxy("down")),
    ];
    let mut metrics = Metrics::new();
    let gateway_metrics = GatewayMetrics::register(&mut metrics);
    let gateway = Gateway::start_with(AdapterOptions::default(), snapshot, |plane| {
        plane.with_metrics(gateway_metrics)
    })
    .await;
    register_configuration(&mut metrics, Arc::clone(&gateway.adapter));
    wait_for(listen).await;
    let text = metrics.encode();
    assert!(
        text.contains("pingora_panel_gateway_config_revision 1\n"),
        "{text}"
    );
    assert!(
        text.contains("# TYPE pingora_panel_gateway_config_activated_timestamp_seconds gauge"),
        "{text}"
    );

    assert_eq!(
        get(listen, Some("example.com"), "/hello", "").await.status,
        200
    );
    assert_eq!(
        get(listen, Some("example.com"), "/down", "").await.status,
        502
    );

    measured(
        &metrics,
        "http_server_request_duration_seconds_count{http_request_method=\"GET\",\
         url_scheme=\"http\",http_response_status_code=\"200\",\
         network_protocol_version=\"1.1\",error_type=\"\",site=\"site\",route=\"app\"} 1",
    )
    .await;
    measured(
        &metrics,
        "http_server_request_duration_seconds_count{http_request_method=\"GET\",\
         url_scheme=\"http\",http_response_status_code=\"502\",\
         network_protocol_version=\"1.1\",error_type=\"connect_refused\",site=\"site\",\
         route=\"down\"} 1",
    )
    .await;
    measured(
        &metrics,
        "pingora_panel_gateway_domain_requests_total{site=\"site\",domain=\"example.com\"} 2",
    )
    .await;
    measured(
        &metrics,
        "pingora_panel_gateway_upstream_connections_total{upstream=\"app\",reused=\"false\"} 1",
    )
    .await;
    measured(
        &metrics,
        &format!(
            "http_client_request_duration_seconds_count{{http_request_method=\"GET\",\
             server_address=\"127.0.0.1\",server_port=\"{}\",http_response_status_code=\"200\",\
             error_type=\"\",upstream=\"app\"}} 1",
            upstream.port()
        ),
    )
    .await;
    measured(
        &metrics,
        &format!(
            "http_client_request_duration_seconds_count{{http_request_method=\"GET\",\
             server_address=\"127.0.0.1\",server_port=\"{}\",http_response_status_code=\"\",\
             error_type=\"connect_refused\",upstream=\"down\"}} 1",
            unreachable.port()
        ),
    )
    .await;
    measured(
        &metrics,
        "http_server_active_requests{http_request_method=\"GET\",url_scheme=\"http\"} 0",
    )
    .await;
    measured(
        &metrics,
        "pingora_panel_gateway_open_connections{listener=\"http\"} 0",
    )
    .await;
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_accepted_before_a_reload_is_answered() {
    let upstream = echo_upstream().await;
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.sites.push(site(&["example.com"]));
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    snapshot
        .routes
        .push(route("app", 1, prefix("/"), proxy("app")));
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let mut early = TcpStream::connect(listen).await.unwrap();
    // The serving generation accepts it, and its request comes only after
    // the generation has been replaced.
    tokio::time::sleep(Duration::from_millis(200)).await;
    gateway.plane.reload().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let response = exchange(
        &mut early,
        "GET / HTTP/1.1\r\nhost: example.com\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(response.status, 200);
    gateway.stop().await;
    assert!(TcpStream::connect(listen).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_idle_between_requests_closes_when_replaced() {
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.sites.push(site(&["example.com"]));
    snapshot.routes.push(route(
        "hello",
        1,
        prefix("/"),
        RouteAction::respond(200, Some("hello".into())),
    ));
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let mut kept = TcpStream::connect(listen).await.unwrap();
    kept.write_all(b"GET / HTTP/1.1\r\nhost: example.com\r\n\r\n")
        .await
        .unwrap();
    let mut answered = Vec::new();
    while !answered.ends_with(b"hello") {
        let mut chunk = [0; 512];
        let read = kept.read(&mut chunk).await.unwrap();
        assert!(read > 0, "the connection closed before its answer");
        answered.extend_from_slice(&chunk[..read]);
    }
    gateway.plane.reload().await.unwrap();
    // Well before the two seconds the replaced generation may drain for.
    let read = tokio::time::timeout(Duration::from_secs(1), kept.read(&mut [0; 64]))
        .await
        .expect("the idle connection is closed")
        .unwrap();
    assert_eq!(read, 0);
    assert_eq!(get(listen, Some("example.com"), "/", "").await.status, 200);
    gateway.stop().await;
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

/// Sends a raw request and reads the answer.
async fn send(address: SocketAddr, request: &str) -> Response {
    let mut stream = TcpStream::connect(address).await.unwrap();
    exchange(&mut stream, request).await
}

fn forwarded(target: &str, client: &str, extra: &str) -> String {
    format!(
        "GET {target} HTTP/1.1\r\nhost: shop.test\r\nx-forwarded-for: {client}\r\n{extra}connection: close\r\n\r\n"
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn security_policies_refuse_limit_and_authenticate() {
    let upstream = echo_upstream().await;
    let secrets = tempfile::tempdir().unwrap();
    let hash = bcrypt::hash("s3cret", 4).unwrap();
    std::fs::write(
        secrets.path().join("staff.htpasswd"),
        format!("alice:{hash}\n"),
    )
    .unwrap();
    let trusting = free_address();
    let untrusting = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let mut behind_proxy = ListenerRef::new("proxied", trusting.to_string());
    behind_proxy.trusted_proxies = ["127.0.0.1/32".to_owned()].into_iter().collect();
    snapshot.listeners.push(behind_proxy);
    snapshot
        .listeners
        .push(ListenerRef::new("direct", untrusting.to_string()));
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    let mut site = site(&["shop.test"]);
    site.security_policy_id = Some("site-rules".into());
    snapshot.sites.push(site);
    snapshot
        .routes
        .push(route("all", 1, prefix("/"), proxy("app")));
    let mut staff = route("staff", 1, prefix("/staff"), proxy("app"));
    staff.security_policy_id = Some("staff".into());
    snapshot.routes.push(staff);
    snapshot
        .upstream_pools
        .push(pool("silent", &[silent_upstream().await]));
    snapshot
        .routes
        .push(route("upload", 1, prefix("/upload"), proxy("silent")));
    snapshot.security_policies.push(SecurityPolicy {
        id: "site-rules".into(),
        denied_cidrs: ["203.0.113.0/24".to_owned()].into_iter().collect(),
        allowed_methods: ["GET".to_owned(), "POST".to_owned()].into_iter().collect(),
        denied_path_prefixes: vec!["/internal".into()],
        denied_user_agents: vec!["badbot".into()],
        max_header_bytes: Some(2048),
        max_body_bytes: Some(16),
        body_timeout_ms: Some(300),
        rate_limits: vec![RateLimit {
            key: RateLimitKey::ClientAddress,
            requests: 3,
            per_seconds: 60,
            burst: 2,
        }],
        ..SecurityPolicy::default()
    });
    snapshot.security_policies.push(SecurityPolicy {
        id: "staff".into(),
        basic_auth: Some(BasicAuth {
            realm: "Staff".into(),
            users_secret_id: "staff.htpasswd".into(),
        }),
        referer: Some(RefererRule {
            allowed_hosts: vec!["shop.test".into()],
            allow_empty: true,
        }),
        ..SecurityPolicy::default()
    });
    let gateway = Gateway::start(
        AdapterOptions::default().with_secrets(Arc::new(DirectorySecrets::new(secrets.path()))),
        snapshot,
    )
    .await;
    wait_for(trusting).await;
    wait_for(untrusting).await;

    assert_eq!(
        send(trusting, &forwarded("/", "203.0.113.5", ""))
            .await
            .status,
        403
    );
    let ok = send(trusting, &forwarded("/", "198.51.100.1", "")).await;
    assert_eq!(ok.status, 200);
    let echoed = String::from_utf8_lossy(&ok.body).to_lowercase();
    assert!(echoed.contains("x-real-ip: 198.51.100.1"), "{echoed}");
    assert!(
        echoed.contains("x-forwarded-for: 198.51.100.1, 127.0.0.1"),
        "{echoed}"
    );

    let deleted = send(
        trusting,
        "DELETE / HTTP/1.1\r\nhost: shop.test\r\nx-forwarded-for: 198.51.100.2\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(deleted.status, 405);
    assert_eq!(deleted.headers["allow"], "GET, HEAD, POST");
    assert_eq!(
        send(trusting, &forwarded("/internal/x", "198.51.100.3", ""))
            .await
            .status,
        403
    );
    assert_eq!(
        send(
            trusting,
            &forwarded("/", "198.51.100.4", "user-agent: BadBot/1.0\r\n")
        )
        .await
        .status,
        403
    );
    let large = format!("x-padding: {}\r\n", "a".repeat(3000));
    assert_eq!(
        send(trusting, &forwarded("/", "198.51.100.5", &large))
            .await
            .status,
        431
    );
    let posted = send(
        trusting,
        "POST / HTTP/1.1\r\nhost: shop.test\r\nx-forwarded-for: 198.51.100.6\r\ncontent-length: 32\r\nconnection: close\r\n\r\n01234567890123456789012345678901",
    )
    .await;
    assert_eq!(posted.status, 413);
    let streamed = send(
        trusting,
        "POST / HTTP/1.1\r\nhost: shop.test\r\nx-forwarded-for: 198.51.100.13\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n20\r\n01234567890123456789012345678901\r\n0\r\n\r\n",
    )
    .await;
    assert_eq!(
        streamed.status, 413,
        "bodies without a length are counted as they arrive"
    );
    let stalled = send(
        trusting,
        "POST /upload HTTP/1.1\r\nhost: shop.test\r\nx-forwarded-for: 198.51.100.14\r\ncontent-length: 10\r\n\r\n012",
    )
    .await;
    assert_eq!(stalled.status, 408, "a body that stops arriving times out");

    for _ in 0..3 {
        assert_eq!(
            send(trusting, &forwarded("/", "198.51.100.7", ""))
                .await
                .status,
            200
        );
    }
    let limited = send(trusting, &forwarded("/", "198.51.100.7", "")).await;
    assert_eq!(limited.status, 429);
    let retry: u64 = limited.headers["retry-after"].parse().unwrap();
    assert!((1..=20).contains(&retry), "{retry}");
    assert_eq!(
        send(trusting, &forwarded("/", "198.51.100.8", ""))
            .await
            .status,
        200
    );

    let challenge = send(trusting, &forwarded("/staff", "198.51.100.9", "")).await;
    assert_eq!(challenge.status, 401);
    assert_eq!(
        challenge.headers["www-authenticate"],
        "Basic realm=\"Staff\", charset=\"UTF-8\""
    );
    let basic = |user: &str, password: &str| {
        format!(
            "authorization: Basic {}\r\n",
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
        )
    };
    assert_eq!(
        send(
            trusting,
            &forwarded("/staff", "198.51.100.10", &basic("alice", "wrong"))
        )
        .await
        .status,
        401
    );
    let authorized = send(
        trusting,
        &forwarded("/staff", "198.51.100.11", &basic("alice", "s3cret")),
    )
    .await;
    assert_eq!(authorized.status, 200);
    assert!(
        !String::from_utf8_lossy(&authorized.body)
            .to_lowercase()
            .contains("authorization:"),
        "credentials stay with the gateway"
    );
    let hotlinked = format!(
        "{}referer: https://evil.example/page\r\n",
        basic("alice", "s3cret")
    );
    assert_eq!(
        send(trusting, &forwarded("/staff", "198.51.100.12", &hotlinked))
            .await
            .status,
        403
    );

    let spoofed = send(untrusting, &forwarded("/", "203.0.113.5", "")).await;
    assert_eq!(
        spoofed.status, 200,
        "untrusted peers are judged by their own address"
    );
    let echoed = String::from_utf8_lossy(&spoofed.body).to_lowercase();
    assert!(!echoed.contains("203.0.113.5"), "{echoed}");
    assert!(echoed.contains("x-real-ip: 127.0.0.1"), "{echoed}");
    gateway.stop().await;
}

/// Sends `head` a byte at a time every `pace` until the gateway answers, and
/// returns what it answered first.
async fn trickle(address: SocketAddr, head: &'static [u8], pace: Duration) -> Vec<u8> {
    let (mut reader, mut writer) = TcpStream::connect(address).await.unwrap().into_split();
    let writing = tokio::spawn(async move {
        for byte in head {
            if writer.write_all(&[*byte]).await.is_err() {
                return;
            }
            tokio::time::sleep(pace).await;
        }
        std::future::pending::<()>().await;
    });
    let mut answer = vec![0; 256];
    let read = tokio::time::timeout(Duration::from_secs(5), reader.read(&mut answer))
        .await
        .expect("the gateway answers in time")
        .unwrap_or(0);
    writing.abort();
    answer.truncate(read);
    answer
}

#[tokio::test]
async fn request_heads_must_arrive_in_time() {
    let upstream = echo_upstream().await;
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let mut listener = ListenerRef::new("http", listen.to_string());
    listener.request_head_timeout_ms = Some(400);
    snapshot.listeners.push(listener);
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    snapshot.sites.push(site(&["shop.test"]));
    snapshot
        .routes
        .push(route("all", 1, prefix("/"), proxy("app")));
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let started = std::time::Instant::now();
    let stalled = trickle(
        listen,
        b"GET / HTTP/1.1\r\nhost: shop.test\r\n",
        Duration::ZERO,
    )
    .await;
    assert!(
        stalled.starts_with(b"HTTP/1.1 408 "),
        "{}",
        String::from_utf8_lossy(&stalled)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    let trickled = trickle(
        listen,
        b"GET / HTTP/1.1\r\nhost: shop.test\r\nx-padding: aaaaaaaaaaaaaaaa\r\n\r\n",
        Duration::from_millis(40),
    )
    .await;
    assert!(
        trickled.starts_with(b"HTTP/1.1 408 "),
        "bytes arriving one by one do not extend the deadline: {}",
        String::from_utf8_lossy(&trickled)
    );

    let mut stream = TcpStream::connect(listen).await.unwrap();
    let request = b"GET / HTTP/1.1\r\nhost: shop.test\r\n\r\n";
    let mut answer = vec![0; 4096];
    for _ in 0..2 {
        stream.write_all(request).await.unwrap();
        let read = stream.read(&mut answer).await.unwrap();
        assert!(answer[..read].starts_with(b"HTTP/1.1 200 "));
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let idle = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut answer))
        .await
        .expect("an idle connection is closed after the deadline");
    assert!(matches!(idle, Ok(0) | Err(_)), "{idle:?}");

    let mut stream = TcpStream::connect(listen).await.unwrap();
    stream.write_all(request).await.unwrap();
    let read = stream.read(&mut answer).await.unwrap();
    assert!(answer[..read].starts_with(b"HTTP/1.1 200 "));
    stream
        .write_all(b"GET / HTTP/1.1\r\nhost: shop.test\r\n")
        .await
        .unwrap();
    let late = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut answer))
        .await
        .expect("a late head on a kept-alive connection closes it");
    assert!(
        matches!(late, Ok(0) | Err(_)),
        "later heads are cut without an answer: {late:?}"
    );
    gateway.stop().await;
}

fn pem(label: &str, der: &[u8]) -> String {
    ::pem::encode(&::pem::Pem::new(label, der))
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
    let mut metrics = Metrics::new();
    let gateway_metrics = GatewayMetrics::register(&mut metrics);
    let gateway = Gateway::start_with(
        AdapterOptions::default().with_secrets(Arc::new(DirectorySecrets::new(secrets.path()))),
        snapshot,
        |plane| plane.with_metrics(gateway_metrics),
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
    measured(
        &metrics,
        "pingora_panel_gateway_tls_handshakes_total{listener=\"https\",\
         tls_protocol_version=\"1.3\"} 3",
    )
    .await;
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

/// The records of `path` once it has at least `wanted` of them.
async fn logged(path: &Path, wanted: usize) -> Vec<String> {
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(path) {
            let lines: Vec<String> = text.lines().map(str::to_owned).collect();
            if lines.len() >= wanted {
                return lines;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{} never had {wanted} records", path.display());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_are_logged_by_site_route_and_format() {
    let upstream = echo_upstream().await;
    let refused = free_address();
    let listen = free_address();
    let directory = tempfile::tempdir().unwrap();
    let mut metrics = Metrics::new();
    let logs = Logs::start(directory.path(), &mut metrics).unwrap();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(7));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.sites.push(site(&["shop.example"]));
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    snapshot.upstream_pools.push(pool("gone", &[refused]));
    let mut quiet = route("quiet", 10, prefix("/health"), proxy("app"));
    quiet.access_log.enabled = Some(false);
    let mut classic = route("classic", 20, prefix("/classic"), proxy("app"));
    classic.access_log.format = Some(AccessLogFormat::Combined);
    let broken = route("broken", 30, prefix("/broken"), proxy("gone"));
    let mut main = route("main", 40, prefix("/"), proxy("app"));
    main.access_log
        .fields
        .insert("tenant".into(), "$http_x_tenant".into());
    snapshot.routes.extend([quiet, classic, broken, main]);
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new(LOGGING_CAPABILITY, "1"));
    let gateway = Gateway::start_with(AdapterOptions::default(), snapshot, |plane| {
        plane.with_logs(logs)
    })
    .await;
    wait_for(listen).await;

    let echoed = get(
        listen,
        Some("shop.example"),
        "/pay?sig=secret&order=7",
        "x-tenant: acme\r\nauthorization: Bearer secret\r\n",
    )
    .await;
    assert_eq!(echoed.status, 200);
    let forwarded = String::from_utf8_lossy(&echoed.body).to_ascii_lowercase();
    let request_id = forwarded
        .lines()
        .find_map(|line| line.strip_prefix("x-request-id: "))
        .expect("the upstream gets a request id")
        .trim()
        .to_owned();
    assert_eq!(
        get(listen, Some("shop.example"), "/health", "")
            .await
            .status,
        200
    );
    get(
        listen,
        Some("shop.example"),
        "/classic?q=1",
        "user-agent: curl/8\r\n",
    )
    .await;
    assert_eq!(
        get(listen, Some("shop.example"), "/broken", "")
            .await
            .status,
        502
    );
    assert_eq!(
        get(listen, Some("nobody.example"), "/", "").await.status,
        421
    );

    let site = logged(&directory.path().join("sites/site.access.log"), 3).await;
    assert_eq!(site.len(), 3, "{site:#?}");
    let first: serde_json::Value = serde_json::from_str(&site[0]).unwrap();
    assert_eq!(first["pingora_panel.request.id"], request_id.as_str());
    assert_eq!(first["url.query"], "sig=REDACTED&order=7");
    assert_eq!(first["tenant"], "acme");
    assert_eq!(first["http.response.status_code"], 200);
    assert_eq!(first["pingora_panel.site.id"], "site");
    assert_eq!(first["pingora_panel.route.id"], "main");
    assert_eq!(first["pingora_panel.revision.id"], 7);
    assert_eq!(first["pingora_panel.upstream.id"], "app");
    assert_eq!(first["client.address"], "127.0.0.1");
    assert!(!site[0].contains("Bearer secret"));
    assert!(
        site[1].starts_with("127.0.0.1 - - [")
            && site[1].contains("\"GET /classic?q=1 HTTP/1.1\" 200 ")
            && site[1].ends_with("\"-\" \"curl/8\""),
        "{}",
        site[1]
    );
    let failed: serde_json::Value = serde_json::from_str(&site[2]).unwrap();
    assert_eq!(failed["http.response.status_code"], 502);
    assert_eq!(failed["pingora_panel.route.id"], "broken");
    assert!(failed["error.type"].is_string(), "{failed}");

    let errors = logged(&directory.path().join("error.log"), 1).await;
    let error: serde_json::Value = serde_json::from_str(&errors[0]).unwrap();
    assert_eq!(error["event.name"], "pingora_panel.error");
    assert_eq!(error["severity_text"], "ERROR");
    assert_eq!(error["pingora_panel.route.id"], "broken");
    assert_eq!(error["error.type"], failed["error.type"]);

    let unclaimed = logged(&directory.path().join("access.log"), 1).await;
    let unclaimed: serde_json::Value = serde_json::from_str(&unclaimed[0]).unwrap();
    assert_eq!(unclaimed["http.response.status_code"], 421);
    assert!(unclaimed.get("pingora_panel.site.id").is_none());
    assert!(metrics
        .encode()
        .contains("pingora_panel_log_records_total{log=\"access\"} 4"));
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_conditions_pick_the_route_a_request_takes() {
    use panel_ir::{RouteCondition, ValueTest, ROUTE_CONDITIONS_CAPABILITY};

    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let mut listener = ListenerRef::new("http", listen.to_string());
    listener.trusted_proxies = ["127.0.0.1/32".to_owned()].into_iter().collect();
    snapshot.listeners.push(listener);
    snapshot.sites.push(site(&["shop.test"]));
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new(ROUTE_CONDITIONS_CAPABILITY, "1"));
    let equals = |value: &str| ValueTest::Equals {
        value: value.into(),
        ignore_case: false,
    };
    let mut canary = route(
        "canary",
        1,
        prefix("/"),
        RouteAction::respond(200, Some("canary".into())),
    );
    canary.conditions = vec![
        RouteCondition::Method {
            methods: vec!["GET".into()],
        },
        RouteCondition::Any {
            conditions: vec![
                RouteCondition::Header {
                    name: "x-canary".into(),
                    test: equals("1"),
                },
                RouteCondition::Cookie {
                    name: "canary".into(),
                    test: equals("1"),
                },
            ],
        },
        RouteCondition::Not {
            condition: Box::new(RouteCondition::Client {
                networks: vec!["203.0.113.0/24".into()],
            }),
        },
    ];
    let mut json = route(
        "json",
        2,
        prefix("/api"),
        RouteAction::respond(201, Some("json".into())),
    );
    json.conditions = vec![
        RouteCondition::ContentType {
            types: vec!["application/json".into()],
        },
        RouteCondition::Query {
            name: "debug".into(),
            test: ValueTest::Absent,
        },
    ];
    snapshot.routes = vec![
        canary,
        json,
        route(
            "stable",
            10,
            prefix("/"),
            RouteAction::respond(200, Some("stable".into())),
        ),
    ];
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let request = |line: &str, extra: &str| {
        format!("{line} HTTP/1.1\r\nhost: shop.test\r\n{extra}content-length: 0\r\nconnection: close\r\n\r\n")
    };
    let body = |response: Response| String::from_utf8(response.body).unwrap();
    let canary_header = send(listen, &request("GET /", "x-canary: 1\r\n")).await;
    assert_eq!(body(canary_header), "canary");
    let outside = send(
        listen,
        &request(
            "GET /",
            "cookie: canary=1\r\nx-forwarded-for: 203.0.113.5\r\n",
        ),
    )
    .await;
    assert_eq!(body(outside), "stable", "the client is outside the route");
    let posted = send(
        listen,
        &request("POST /api/items", "content-type: application/json\r\n"),
    )
    .await;
    assert_eq!(posted.status, 201);
    let debugging = send(
        listen,
        &request(
            "POST /api/items?debug=1",
            "content-type: application/json\r\n",
        ),
    )
    .await;
    assert_eq!(body(debugging), "stable");
    let plain = send(listen, &request("GET /api/items", "")).await;
    assert_eq!(body(plain), "stable");
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_policies_change_headers_answer_cors_and_compress() {
    use panel_ir::{
        CompressionAlgorithm, CompressionPolicy, CorsPolicy, HeaderField, HeaderPolicy,
        ServerHeader, HTTP_POLICIES_CAPABILITY,
    };

    let upstream = echo_upstream().await;
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    let mut shop = site(&["shop.test"]);
    shop.header_policy_id = Some("site-headers".into());
    snapshot.sites.push(shop);
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    let mut api = route("api", 1, prefix("/api"), proxy("app"));
    api.header_policy_id = Some("api".into());
    snapshot.routes = vec![api, route("all", 10, prefix("/"), proxy("app"))];
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new(HTTP_POLICIES_CAPABILITY, "1"));
    snapshot.header_policies = vec![
        HeaderPolicy {
            id: "site-headers".into(),
            request_set: [("x-from-site".to_owned(), "site".to_owned())].into(),
            response_add: vec![HeaderField {
                name: "x-site".into(),
                value: "yes".into(),
            }],
            server: ServerHeader::Replace {
                value: "shop".into(),
            },
            ..HeaderPolicy::default()
        },
        HeaderPolicy {
            id: "api".into(),
            request_add: vec![HeaderField {
                name: "x-tenant".into(),
                value: "$host".into(),
            }],
            request_remove: ["x-internal".to_owned()].into(),
            cors: Some(CorsPolicy {
                allowed_origins: vec!["https://*.shop.test".into()],
                allowed_methods: vec!["PUT".into()],
                allowed_headers: vec!["x-api-key".into()],
                exposed_headers: vec!["x-request-id".into()],
                allow_credentials: true,
                max_age_seconds: Some(600),
            }),
            compression: Some(CompressionPolicy {
                algorithms: [CompressionAlgorithm::Gzip].into(),
                types: vec!["text/*".into()],
                min_bytes: 16,
            }),
            ..HeaderPolicy::default()
        },
    ];
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let request = |line: &str, extra: &str| {
        format!("{line} HTTP/1.1\r\nhost: shop.test\r\n{extra}connection: close\r\n\r\n")
    };
    let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let forwarded = send(
        listen,
        &request(
            "GET /api/items",
            &format!(
                "x-internal: secret\r\nx-request-id: req-77\r\ntraceparent: {traceparent}\r\n"
            ),
        ),
    )
    .await;
    let head = String::from_utf8_lossy(&forwarded.body).to_ascii_lowercase();
    assert!(head.contains("x-from-site: site"), "{head}");
    assert!(head.contains("x-tenant: shop.test"), "{head}");
    assert!(!head.contains("x-internal"), "{head}");
    // The client's Host, request ID and trace context reach the upstream
    // untouched, beside the forwarding fields the gateway writes.
    for line in [
        "host: shop.test",
        "x-forwarded-host: shop.test",
        "x-request-id: req-77",
        &format!("traceparent: {traceparent}"),
    ] {
        assert!(head.contains(line), "{line}: {head}");
    }
    assert_eq!(forwarded.headers["server"], "shop");
    assert_eq!(forwarded.headers["x-site"], "yes");

    let preflight = send(
        listen,
        &request(
            "OPTIONS /api/items",
            "origin: https://app.shop.test\r\naccess-control-request-method: PUT\r\naccess-control-request-headers: X-Api-Key\r\n",
        ),
    )
    .await;
    assert_eq!(preflight.status, 204);
    assert_eq!(
        preflight.headers["access-control-allow-origin"],
        "https://app.shop.test"
    );
    assert_eq!(preflight.headers["access-control-allow-methods"], "PUT");
    assert_eq!(
        preflight.headers["access-control-allow-headers"],
        "x-api-key"
    );
    assert_eq!(
        preflight.headers["access-control-allow-credentials"],
        "true"
    );
    assert_eq!(preflight.headers["access-control-max-age"], "600");
    let foreign = send(
        listen,
        &request(
            "OPTIONS /api/items",
            "origin: https://evil.test\r\naccess-control-request-method: PUT\r\n",
        ),
    )
    .await;
    assert_eq!(foreign.status, 204);
    assert!(!foreign.headers.contains_key("access-control-allow-origin"));

    let cross = send(
        listen,
        &request("GET /api/items", "origin: https://app.shop.test\r\n"),
    )
    .await;
    assert_eq!(
        cross.headers["access-control-allow-origin"],
        "https://app.shop.test"
    );
    assert_eq!(
        cross.headers["access-control-expose-headers"],
        "x-request-id"
    );
    assert!(
        cross.headers["vary"].contains("Origin"),
        "{:?}",
        cross.headers
    );

    let compressed = send(
        listen,
        &request("GET /api/items", "accept-encoding: gzip\r\n"),
    )
    .await;
    assert_eq!(
        compressed
            .headers
            .get("content-encoding")
            .map(String::as_str),
        Some("gzip")
    );
    assert!(compressed.headers["vary"].contains("Accept-Encoding"));
    // Weights count (RFC 9110 §12.5.3): a refused coding is not used, and
    // the upstream still sees the field as the client sent it.
    let refused = send(
        listen,
        &request("GET /api/items", "accept-encoding: gzip;q=0, br\r\n"),
    )
    .await;
    assert!(!refused.headers.contains_key("content-encoding"));
    let head = String::from_utf8_lossy(&refused.body).to_ascii_lowercase();
    assert!(head.contains("accept-encoding: gzip;q=0, br"), "{head}");
    let weighed = send(
        listen,
        &request(
            "GET /api/items",
            "accept-encoding: br;q=0.5, gzip;q=0.8\r\n",
        ),
    )
    .await;
    assert_eq!(
        weighed.headers.get("content-encoding").map(String::as_str),
        Some("gzip")
    );
    let elsewhere = send(listen, &request("GET /home", "accept-encoding: gzip\r\n")).await;
    assert!(
        !elsewhere.headers.contains_key("content-encoding"),
        "only the route compresses"
    );
    assert!(!elsewhere
        .headers
        .contains_key("access-control-allow-origin"));
    assert_eq!(
        elsewhere.headers["server"], "shop",
        "the site's policy still applies"
    );
    gateway.stop().await;
}

/// An upstream that answers every request with the status `status` holds
/// after `delay`, counting the requests it reads.
async fn status_upstream(
    status: Arc<AtomicU16>,
    delay: Duration,
) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (status, hits) = (Arc::clone(&status), Arc::clone(&counted));
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                hits.fetch_add(1, SeqCst);
                tokio::time::sleep(delay).await;
                let status = status.load(SeqCst);
                let body = format!("{status} from {address}");
                let response = format!(
                    "HTTP/1.1 {status} Status\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (address, hits)
}

/// An upstream that reads each request head and closes without answering.
async fn closing_upstream() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let hits = Arc::clone(&counted);
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                hits.fetch_add(1, SeqCst);
            });
        }
    });
    (address, hits)
}

fn resilient(snapshot: &mut RuntimeSnapshot) {
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new(
            UPSTREAM_RESILIENCE_CAPABILITY,
            "1",
        ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upstreams_retry_what_their_policy_lists_within_their_budget() {
    let failing = Arc::new(AtomicU16::new(503));
    let healthy = Arc::new(AtomicU16::new(200));
    let (refusing, refused_hits) = status_upstream(Arc::clone(&failing), Duration::ZERO).await;
    let (answering, _) = status_upstream(Arc::clone(&healthy), Duration::ZERO).await;
    let (closing, closed_hits) = closing_upstream().await;
    let (answering_too, _) = status_upstream(Arc::clone(&healthy), Duration::ZERO).await;
    let (tight_refusing, _) = status_upstream(Arc::clone(&failing), Duration::ZERO).await;
    let (tight_answering, _) = status_upstream(healthy, Duration::ZERO).await;
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.sites.push(site(&["retry.test"]));
    let retry = |statuses: &[u16], on: &[RetryCondition], budget| RetryPolicy {
        attempts: 1,
        retry_statuses: statuses.iter().copied().collect(),
        retry_on: on.iter().copied().collect(),
        backoff_ms: 5,
        budget,
        ..RetryPolicy::none()
    };
    let mut statuses = pool("statuses", &[refusing, answering]);
    statuses.retry_policy = retry(&[503], &[], None);
    let mut resets = pool("resets", &[closing, answering_too]);
    resets.retry_policy = retry(&[], &[RetryCondition::Reset], None);
    let mut tight = pool("tight", &[tight_refusing, tight_answering]);
    tight.retry_policy = retry(
        &[503],
        &[],
        Some(RetryBudget {
            percent: 1,
            min_per_second: 0,
        }),
    );
    snapshot.upstream_pools.extend([statuses, resets, tight]);
    snapshot.routes.extend([
        route("statuses", 1, prefix("/statuses"), proxy("statuses")),
        route("resets", 2, prefix("/resets"), proxy("resets")),
        route("tight", 3, prefix("/tight"), proxy("tight")),
    ]);
    resilient(&mut snapshot);
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    for _ in 0..4 {
        assert_eq!(
            get(listen, Some("retry.test"), "/statuses", "")
                .await
                .status,
            200
        );
    }
    assert!(refused_hits.load(SeqCst) >= 1, "some tries met the 503");
    let mut posted = Vec::new();
    for _ in 0..4 {
        let request =
            "POST /statuses HTTP/1.1\r\nhost: retry.test\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        posted.push(send(listen, request).await.status);
    }
    assert!(
        posted.contains(&503) && posted.contains(&200),
        "POST is not idempotent, so its 503 stands: {posted:?}"
    );

    for _ in 0..4 {
        assert_eq!(
            get(listen, Some("retry.test"), "/resets", "").await.status,
            200
        );
    }
    assert!(closed_hits.load(SeqCst) >= 1, "some tries were cut off");

    let mut budgeted = Vec::new();
    for _ in 0..4 {
        budgeted.push(get(listen, Some("retry.test"), "/tight", "").await.status);
    }
    assert!(
        budgeted.contains(&503),
        "a budget of one in a hundred leaves no retry: {budgeted:?}"
    );
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn circuits_open_on_failures_and_close_after_a_trial() {
    let status = Arc::new(AtomicU16::new(502));
    let (upstream, hits) = status_upstream(Arc::clone(&status), Duration::ZERO).await;
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.sites.push(site(&["circuit.test"]));
    let mut app = pool("app", &[upstream]);
    app.circuit_breaker = Some(CircuitBreaker {
        failure_percent: 50,
        min_requests: 2,
        open_ms: 1_000,
        half_open_requests: 1,
    });
    snapshot.upstream_pools.push(app);
    snapshot
        .routes
        .push(route("app", 1, prefix("/"), proxy("app")));
    resilient(&mut snapshot);
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let call = || get(listen, Some("circuit.test"), "/", "");
    assert_eq!(call().await.status, 502);
    assert_eq!(call().await.status, 502);
    let open = call().await;
    assert_eq!(open.status, 503);
    assert_eq!(open.headers["retry-after"], "1");
    assert_eq!(
        hits.load(SeqCst),
        2,
        "an open circuit sends nothing upstream"
    );

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    assert_eq!(call().await.status, 502, "the trial still fails");
    assert_eq!(call().await.status, 503, "and the circuit opens again");
    assert_eq!(hits.load(SeqCst), 3);

    status.store(200, SeqCst);
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    assert_eq!(call().await.status, 200, "a good trial closes it");
    assert_eq!(call().await.status, 200);
    assert_eq!(hits.load(SeqCst), 5);
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upstream_limits_queue_or_refuse_requests_over_them() {
    let ok = Arc::new(AtomicU16::new(200));
    let slow = Duration::from_millis(500);
    let (queued_node, _) = status_upstream(Arc::clone(&ok), slow).await;
    let (impatient_node, _) = status_upstream(Arc::clone(&ok), slow).await;
    let (strict_node, _) = status_upstream(ok, slow).await;
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.sites.push(site(&["limit.test"]));
    let limited = |id: &str, node, queue| {
        let mut limited = pool(id, &[node]);
        limited.max_requests = Some(1);
        limited.queue = queue;
        limited
    };
    snapshot.upstream_pools.extend([
        limited(
            "queued",
            queued_node,
            Some(UpstreamQueue {
                max_waiting: 1,
                timeout_ms: 2_000,
            }),
        ),
        limited(
            "impatient",
            impatient_node,
            Some(UpstreamQueue {
                max_waiting: 4,
                timeout_ms: 100,
            }),
        ),
        limited("strict", strict_node, None),
    ]);
    snapshot.routes.extend([
        route("queued", 1, prefix("/queued"), proxy("queued")),
        route("impatient", 2, prefix("/impatient"), proxy("impatient")),
        route("strict", 3, prefix("/strict"), proxy("strict")),
    ]);
    resilient(&mut snapshot);
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    for path in ["/queued", "/impatient", "/strict"] {
        assert_eq!(get(listen, Some("limit.test"), path, "").await.status, 200);
    }
    let later = |path: &'static str, wait: u64| async move {
        tokio::time::sleep(Duration::from_millis(wait)).await;
        get(listen, Some("limit.test"), path, "").await
    };
    let outcome = |responses: &[&Response]| {
        let mut outcome: Vec<(u16, String)> = responses
            .iter()
            .map(|response| {
                let body = String::from_utf8_lossy(&response.body).into_owned();
                (
                    response.status,
                    body.split(" from ").next().unwrap().trim().to_owned(),
                )
            })
            .collect();
        outcome.sort();
        outcome
    };
    let (first, second, third) = tokio::join!(
        later("/queued", 0),
        later("/queued", 100),
        later("/queued", 200)
    );
    assert_eq!(
        outcome(&[&first, &second, &third]),
        [
            (200, "200".to_owned()),
            (200, "200".to_owned()),
            (
                503,
                "the upstream is handling all the requests it takes".to_owned()
            )
        ],
        "one waits its turn and the queue holds no more"
    );
    let (first, second) = tokio::join!(later("/impatient", 0), later("/impatient", 100));
    assert_eq!(
        outcome(&[&first, &second]),
        [
            (200, "200".to_owned()),
            (
                503,
                "the upstream did not take the request in time".to_owned()
            )
        ]
    );
    let (first, second) = tokio::join!(later("/strict", 0), later("/strict", 100));
    let refused = if first.status == 503 { &first } else { &second };
    assert_eq!(
        outcome(&[&first, &second]),
        [
            (200, "200".to_owned()),
            (
                503,
                "the upstream is handling all the requests it takes".to_owned()
            )
        ]
    );
    assert_eq!(refused.headers["retry-after"], "1");
    gateway.stop().await;
}

/// A WebSocket server that sends back every message it receives.
async fn websocket_upstream() -> SocketAddr {
    use futures_util::{SinkExt, StreamExt};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(message)) = socket.next().await {
                    if (message.is_text() || message.is_binary())
                        && socket.send(message).await.is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    address
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_upgrades_go_through_beyond_body_limits() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let upstream = websocket_upstream().await;
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    let mut chat = site(&["chat.test"]);
    chat.security_policy_id = Some("small".into());
    snapshot.sites.push(chat);
    snapshot.security_policies.push(SecurityPolicy {
        id: "small".into(),
        max_body_bytes: Some(16),
        ..SecurityPolicy::default()
    });
    let mut app = pool("chat", &[upstream]);
    app.connection.keepalive = false;
    snapshot.upstream_pools.push(app);
    snapshot
        .routes
        .push(route("chat", 1, prefix("/"), proxy("chat")));
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new(REQUEST_SECURITY_CAPABILITY, "1"));
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let stream = TcpStream::connect(listen).await.unwrap();
    let (mut socket, response) = tokio_tungstenite::client_async("ws://chat.test/ws", stream)
        .await
        .unwrap();
    assert_eq!(response.status(), 101);
    for message in [
        "a message longer than the sixteen bytes a body may have",
        "and another after it",
    ] {
        socket.send(Message::text(message)).await.unwrap();
        let echoed = socket.next().await.unwrap().unwrap();
        assert_eq!(echoed.to_text().unwrap(), message);
    }
    socket.close(None).await.unwrap();
    gateway.stop().await;
}

/// An upstream that streams one Server-Sent Event, and a second once
/// `release` is notified.
async fn event_upstream(release: Arc<tokio::sync::Notify>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let release = Arc::clone(&release);
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\ndata: one\n\n")
                    .await;
                release.notified().await;
                let _ = stream.write_all(b"data: two\n\n").await;
            });
        }
    });
    address
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_sent_events_stream_as_they_come_and_are_not_compressed() {
    use panel_ir::{
        CompressionAlgorithm, CompressionPolicy, HeaderPolicy, HTTP_POLICIES_CAPABILITY,
    };

    let release = Arc::new(tokio::sync::Notify::new());
    let upstream = event_upstream(Arc::clone(&release)).await;
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    let mut feed = site(&["feed.test"]);
    feed.header_policy_id = Some("compress".into());
    snapshot.sites.push(feed);
    snapshot.header_policies.push(HeaderPolicy {
        id: "compress".into(),
        compression: Some(CompressionPolicy {
            algorithms: [CompressionAlgorithm::Gzip].into(),
            types: vec!["text/*".into()],
            min_bytes: 0,
        }),
        ..HeaderPolicy::default()
    });
    snapshot.upstream_pools.push(pool("feed", &[upstream]));
    snapshot
        .routes
        .push(route("feed", 1, prefix("/"), proxy("feed")));
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new(HTTP_POLICIES_CAPABILITY, "1"));
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let mut stream = TcpStream::connect(listen).await.unwrap();
    stream
        .write_all(b"GET /events HTTP/1.1\r\nhost: feed.test\r\naccept: text/event-stream\r\naccept-encoding: gzip\r\nconnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut received = Vec::new();
    let mut chunk = [0; 1024];
    // The upstream holds the second event back until the first arrives, so
    // a gateway that waited for the whole response would never deliver it.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !String::from_utf8_lossy(&received).contains("data: one") {
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "the stream ended early");
            received.extend_from_slice(&chunk[..read]);
        }
    })
    .await
    .expect("the first event arrives while the stream is open");
    release.notify_one();
    let head = String::from_utf8_lossy(&received).to_ascii_lowercase();
    assert!(head.contains("content-type: text/event-stream"), "{head}");
    assert!(!head.contains("content-encoding"), "{head}");
    stream.read_to_end(&mut received).await.unwrap();
    assert!(String::from_utf8_lossy(&received).contains("data: two"));
    gateway.stop().await;
}

/// A gRPC-style server over h2c that answers each call with its own
/// message and the `grpc-status` trailer.
async fn grpc_upstream() -> SocketAddr {
    use http_body_util::{BodyExt, StreamBody};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    |request: http::Request<hyper::body::Incoming>| async move {
                        let version = format!("{:?}", request.version());
                        let message = request.into_body().collect().await?.to_bytes();
                        let mut trailers = http::HeaderMap::new();
                        trailers.insert("grpc-status", "0".parse().unwrap());
                        trailers.insert("grpc-message", "ok".parse().unwrap());
                        let frames = vec![
                            Ok::<_, hyper::Error>(hyper::body::Frame::data(message)),
                            Ok(hyper::body::Frame::trailers(trailers)),
                        ];
                        Ok::<_, hyper::Error>(
                            http::Response::builder()
                                .header("content-type", "application/grpc")
                                .header("x-upstream-version", version)
                                .body(StreamBody::new(futures_util::stream::iter(frames)))
                                .unwrap(),
                        )
                    },
                );
                let _ =
                    hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                        .await;
            });
        }
    });
    address
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_goes_over_http2_both_ways_with_its_trailers() {
    use http_body_util::{BodyExt, Full};

    let upstream = grpc_upstream().await;
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.sites.push(site(&["grpc.test"]));
    let mut app = pool("grpc", &[upstream]);
    app.connection.h2c = true;
    snapshot.upstream_pools.push(app);
    snapshot
        .routes
        .push(route("grpc", 1, prefix("/"), proxy("grpc")));
    resilient(&mut snapshot);
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let stream = TcpStream::connect(listen).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http2::handshake(
        hyper_util::rt::TokioExecutor::new(),
        hyper_util::rt::TokioIo::new(stream),
    )
    .await
    .unwrap();
    tokio::spawn(connection);
    let message = bytes::Bytes::from_static(b"\0\0\0\0\x05hello");
    let request = http::Request::post("http://grpc.test/echo.Echo/Say")
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(Full::new(message.clone()))
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-upstream-version"], "HTTP/2.0");
    let collected = response.into_body().collect().await.unwrap();
    let trailers = collected.trailers().cloned().expect("trailers");
    assert_eq!(trailers["grpc-status"], "0");
    assert_eq!(trailers["grpc-message"], "ok");
    assert_eq!(collected.to_bytes(), message);
    drop(sender);
    gateway.stop().await;
}

fn lua_script(id: &str, source: &str) -> panel_ir::LuaScript {
    panel_ir::LuaScript {
        id: id.into(),
        file: "main.conf".into(),
        line: 1,
        source: source.into(),
        sha256: panel_domain::ContentHash::from_bytes(source.as_bytes())
            .as_str()
            .into(),
        module: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lua_handlers_rewrite_answer_filter_and_choose_peers() {
    use panel_ir::{LuaFallback, LuaHandler, LuaSharedDict, LUA_SCRIPTS_CAPABILITY};

    let upstream = echo_upstream().await;
    let unreachable = free_address();
    let listen = free_address();
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new(LUA_SCRIPTS_CAPABILITY, "1"));
    let balancer = format!(
        "assert(require('ngx.balancer').set_current_peer('127.0.0.1', {}))",
        upstream.port()
    );
    snapshot.lua.scripts = vec![
        lua_script("rewrite", "if ngx.var.uri == '/old' then ngx.req.set_uri('/new') end"),
        lua_script(
            "content",
            "ngx.header['X-From'] = 'lua' ngx.say('hello ', ngx.var.arg_name or 'nobody')",
        ),
        lua_script(
            "access",
            "if ngx.req.get_headers()['x-key'] ~= 'secret' then return ngx.exit(403) end ngx.req.set_header('X-Checked', 'yes')",
        ),
        lua_script("header", "ngx.header['X-Filtered'] = tostring(ngx.status)"),
        lua_script("seen", "ngx.header['X-Seen'] = ngx.var.uri"),
        lua_script("body", "ngx.arg[1] = string.upper(ngx.arg[1])"),
        lua_script("balancer", &balancer),
        lua_script("broken", "error('nope')"),
        lua_script("loop", "while true do end"),
        lua_script(
            "count",
            "local hits = ngx.shared.hits hits:safe_add('n', 0) ngx.say(hits:incr('n', 1))",
        ),
    ];
    snapshot.lua.shared_dicts.push(LuaSharedDict {
        name: "hits".into(),
        capacity_bytes: 1 << 20,
    });
    let mut shop = site(&["shop.test"]);
    shop.lua.server_rewrite = Some(LuaHandler::new("rewrite"));
    snapshot.sites.push(shop);
    let mut checked = route("checked", 3, prefix("/checked"), proxy("app"));
    checked.lua.access = Some(LuaHandler::new("access"));
    checked.lua.header_filter = Some(LuaHandler::new("header"));
    let mut upper = LuaHandler::new("body");
    upper.allow.body = true;
    checked.lua.body_filter = Some(upper);
    let mut broken = LuaHandler::new("broken");
    broken.on_error = LuaFallback::Status { status: 503 };
    let mut looping = LuaHandler::new("loop");
    looping.time_limit_ms = 50;
    let mut generated = route(
        "new",
        1,
        prefix("/new"),
        RouteAction::respond(200, Some("new".into())),
    );
    generated.lua.header_filter = Some(LuaHandler::new("seen"));
    snapshot.routes = vec![
        generated,
        route(
            "content",
            2,
            prefix("/lua"),
            RouteAction::Lua {
                handler: LuaHandler::new("content"),
            },
        ),
        checked,
        route("balanced", 4, prefix("/balanced"), proxy("lb")),
        route(
            "broken",
            5,
            prefix("/broken"),
            RouteAction::Lua { handler: broken },
        ),
        route(
            "loop",
            6,
            prefix("/loop"),
            RouteAction::Lua { handler: looping },
        ),
        route(
            "count",
            7,
            prefix("/count"),
            RouteAction::Lua {
                handler: LuaHandler::new("count"),
            },
        ),
    ];
    snapshot.upstream_pools.push(pool("app", &[upstream]));
    let mut lb = pool("lb", &[unreachable]);
    let mut choose = LuaHandler::new("balancer");
    choose.allow.upstream = true;
    lb.balancer = Some(choose);
    snapshot.upstream_pools.push(lb);
    let gateway = Gateway::start(AdapterOptions::default(), snapshot).await;
    wait_for(listen).await;

    let body = |response: &Response| String::from_utf8_lossy(&response.body).into_owned();
    let rewritten = get(listen, Some("shop.test"), "/old", "").await;
    assert_eq!(body(&rewritten), "new", "server_rewrite chose the route");
    assert_eq!(
        rewritten.headers["x-seen"], "/new",
        "the header filter runs on responses the gateway makes"
    );

    let answered = get(listen, Some("shop.test"), "/lua?name=ann", "").await;
    assert_eq!(answered.status, 200);
    assert_eq!(answered.headers["x-from"], "lua");
    assert_eq!(body(&answered), "hello ann\n");

    let refused = get(listen, Some("shop.test"), "/checked", "").await;
    assert_eq!(refused.status, 403);
    let admitted = get(listen, Some("shop.test"), "/checked", "x-key: secret\r\n").await;
    assert_eq!(admitted.status, 200);
    assert_eq!(admitted.headers["x-filtered"], "200");
    assert_eq!(admitted.headers["transfer-encoding"], "chunked");
    let echoed = body(&admitted);
    assert!(echoed.contains("X-CHECKED: YES"), "{echoed}");
    assert!(echoed.contains("\r\nGET /CHECKED"), "{echoed}");

    let balanced = get(listen, Some("shop.test"), "/balanced", "").await;
    assert_eq!(balanced.status, 200, "{}", body(&balanced));
    assert!(body(&balanced).starts_with("GET /balanced"));

    assert_eq!(
        get(listen, Some("shop.test"), "/broken", "").await.status,
        503
    );
    let started = std::time::Instant::now();
    assert_eq!(
        get(listen, Some("shop.test"), "/loop", "").await.status,
        500
    );
    assert!(started.elapsed() < Duration::from_secs(2));

    for expected in ["1\n", "2\n", "3\n"] {
        assert_eq!(
            body(&get(listen, Some("shop.test"), "/count", "").await),
            expected
        );
    }
    gateway.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snapshots_whose_scripts_do_not_compile_are_refused() {
    use panel_ir::{LuaHandler, LUA_SCRIPTS_CAPABILITY};

    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", free_address().to_string()));
    snapshot
        .required_capabilities
        .push(CapabilityRequirement::new(LUA_SCRIPTS_CAPABILITY, "1"));
    snapshot.lua.scripts = vec![lua_script("broken", "local x = \nif x then")];
    snapshot.sites.push(site(&["shop.test"]));
    snapshot.routes.push(route(
        "content",
        1,
        prefix("/"),
        RouteAction::Lua {
            handler: LuaHandler::new("broken"),
        },
    ));
    snapshot.refresh_content_hash();
    let adapter = PingoraGatewayAdapter::with_options(AdapterOptions::default());
    let error = adapter.prepare(snapshot).await.err().expect("refused");
    assert!(
        error
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.starts_with("main.conf:2:")),
        "{error:?}"
    );
}
