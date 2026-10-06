#![forbid(unsafe_code)]

//! `resty.http` against a server that answers in each of HTTP/1.1's body
//! framings, takes requests as a forward proxy, and opens CONNECT tunnels
//! to itself over TLS.

use http::HeaderMap;
use panel_lua::{
    Connection, Exchange, Handler, NoHost, Outcome, Phase, Program, Request, Runtime, Settings,
    SharedStore, Source,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
};

async fn line<S: AsyncRead + Unpin>(stream: &mut S) -> Option<String> {
    let mut bytes = Vec::new();
    let mut byte = [0; 1];
    while !bytes.ends_with(b"\r\n") {
        if stream.read(&mut byte).await.ok()? == 0 {
            return None;
        }
        bytes.push(byte[0]);
    }
    bytes.truncate(bytes.len() - 2);
    String::from_utf8(bytes).ok()
}

struct Head {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

async fn head<S: AsyncRead + Unpin>(stream: &mut S) -> Option<Head> {
    let request = line(stream).await?;
    let mut parts = request.split(' ');
    let (method, target) = (parts.next()?.to_owned(), parts.next()?.to_owned());
    let mut headers = Vec::new();
    loop {
        let field = line(stream).await?;
        if field.is_empty() {
            break;
        }
        let (name, value) = field.split_once(':')?;
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    Some(Head {
        method,
        target,
        headers,
    })
}

async fn body<S: AsyncRead + Unpin>(stream: &mut S, head: &Head) -> Option<Vec<u8>> {
    if head
        .header("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        let mut data = Vec::new();
        loop {
            let size = usize::from_str_radix(line(stream).await?.trim(), 16).ok()?;
            if size == 0 {
                line(stream).await?;
                return Some(data);
            }
            let mut chunk = vec![0; size + 2];
            stream.read_exact(&mut chunk).await.ok()?;
            data.extend_from_slice(&chunk[..size]);
        }
    }
    let length: usize = head
        .header("content-length")
        .map_or(Some(0), |value| value.parse().ok())?;
    let mut data = vec![0; length];
    stream.read_exact(&mut data).await.ok()?;
    Some(data)
}

fn answer(status: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut bytes = response.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// Answers requests on `stream` until it closes; whether it was asked for
/// a tunnel and opened one.
async fn serve<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S) -> bool {
    while let Some(head) = head(stream).await {
        let path = head.target.split('?').next().unwrap_or_default().to_owned();
        let query = head
            .target
            .split_once('?')
            .map(|(_, query)| query.to_owned())
            .unwrap_or_default();
        let reply = match (head.method.as_str(), path.as_str()) {
            ("CONNECT", _) => {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await;
                return true;
            }
            (_, target) if target.starts_with("http://") => {
                let authorization = head.header("proxy-authorization").unwrap_or("none");
                answer(
                    "200 OK",
                    &[],
                    format!("via proxy {} {authorization}", head.target).as_bytes(),
                )
            }
            ("HEAD", "/hello") => {
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n".to_vec()
            }
            (_, "/hello") => answer(
                "200 OK",
                &[("X-Multi", "a".into()), ("X-Multi", "b".into())],
                b"hello",
            ),
            (_, "/nobody") => b"HTTP/1.1 204 No Content\r\n\r\n".to_vec(),
            (_, "/chunked") => b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTrailer: X-Trailer\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\nX-Trailer: done\r\n\r\n".to_vec(),
            (_, "/close") => {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nbye")
                    .await;
                return false;
            }
            (_, "/refuse-continue") => {
                let _ = stream
                    .write_all(b"HTTP/1.1 417 Expectation Failed\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                    .await;
                return false;
            }
            (method, "/echo" | "/continue") => {
                if head
                    .header("expect")
                    .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
                {
                    let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await;
                }
                let Some(data) = body(stream, &head).await else {
                    return false;
                };
                answer(
                    "200 OK",
                    &[("X-Method", method.into()), ("X-Query", query)],
                    &data,
                )
            }
            _ => answer("404 Not Found", &[], b""),
        };
        if stream.write_all(&reply).await.is_err() {
            return false;
        }
    }
    false
}

/// The server's port, and how many connections it accepted.
async fn server() -> (u16, Arc<AtomicUsize>) {
    let certified = rcgen::generate_simple_self_signed(vec!["tls.test".into()]).unwrap();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![certified.cert.der().clone()], key.into())
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&accepted);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if serve(&mut stream).await {
                    if let Ok(mut tls) = acceptor.accept(stream).await {
                        serve(&mut tls).await;
                    }
                }
            });
        }
    });
    (port, accepted)
}

#[tokio::test]
async fn http_clients_speak_as_lua_resty_http_does() {
    let (port, accepted) = server().await;
    let script = format!(
        r#"
        local http = require "resty.http"
        local http_headers = require "resty.http_headers"
        local headers = http_headers.new()
        headers["Content-Type"] = "text/plain"
        assert(headers["content-type"] == "text/plain" and headers.CONTENT_TYPE == nil)
        headers["CONTENT-TYPE"] = nil
        assert(headers["Content-Type"] == nil and next(headers) == nil)

        local parsed = http:parse_uri("https://user:pw@[::1]:8443/a/b?x=1#frag")
        assert(parsed[1] == "https" and parsed[2] == "[::1]" and parsed[3] == 8443)
        assert(parsed[4] == "/a/b?x=1" and parsed[5] == "")
        parsed = http:parse_uri("http://example.com", false)
        assert(parsed[3] == 80 and parsed[4] == "/" and parsed[5] == "")
        parsed = http:parse_uri("http://example.com/p?q=2", false)
        assert(parsed[4] == "/p" and parsed[5] == "q=2")
        assert(select(2, http:parse_uri("ftp://example.com/")) == "bad uri: ftp://example.com/")

        local httpc = assert(http.new())
        httpc:set_timeouts(2000, 2000, 2000)
        local res, err = httpc:request_uri("http://127.0.0.1:{port}/hello")
        assert(res and res.status == 200 and res.reason == "OK" and res.body == "hello", err)
        assert(res.headers["x-multi"][1] == "a" and res.headers["X-Multi"][2] == "b")
        res = assert(httpc:request_uri("http://127.0.0.1:{port}/echo", {{
            method = "POST", body = "a=1", query = {{ b = "2" }},
            headers = {{ ["Content-Type"] = "text/plain" }},
        }}))
        assert(res.body == "a=1" and res.headers["X-Method"] == "POST" and res.headers["X-Query"] == "b=2")

        assert(httpc:connect({{ scheme = "http", host = "127.0.0.1", port = {port} }}))
        assert(httpc:get_reused_times() >= 1)
        res = assert(httpc:request({{ path = "/chunked" }}))
        assert(res.status == 200 and res.has_body)
        local parts = {{}}
        while true do
            local chunk, failed = res.body_reader(3)
            assert(not failed, failed)
            if not chunk then
                break
            end
            assert(#chunk <= 3)
            parts[#parts + 1] = chunk
        end
        assert(table.concat(parts) == "Wikipedia")
        res:read_trailers()
        assert(res.headers["X-Trailer"] == "done")
        res = assert(httpc:request({{
            method = "PUT", path = "/echo", headers = {{ ["Transfer-Encoding"] = "chunked" }},
            body = coroutine.wrap(function()
                coroutine.yield("3\r\nabc\r\n")
                coroutine.yield("0\r\n\r\n")
            end),
        }}))
        assert(res:read_body() == "abc")
        res = assert(httpc:request({{ method = "HEAD", path = "/hello" }}))
        assert(not res.has_body and res:read_body() == "")
        res = assert(httpc:request({{ path = "/nobody" }}))
        assert(res.status == 204 and not res.has_body)
        res = assert(httpc:request({{
            method = "POST", path = "/continue", body = "late", headers = {{ Expect = "100-continue" }},
        }}))
        assert(res.status == 200 and res:read_body() == "late")
        local responses = assert(httpc:request_pipeline({{
            {{ path = "/hello" }},
            {{ path = "/echo", method = "POST", body = "x" }},
        }}))
        assert(responses[1].status == 200 and responses[1]:read_body() == "hello")
        assert(responses[2].status == 200 and responses[2]:read_body() == "x")
        assert(httpc:set_keepalive() == 1)

        res = assert(http.new():request_uri("http://127.0.0.1:{port}/close"))
        assert(res.body == "bye")
        local refusing = http.new()
        assert(refusing:connect({{ host = "127.0.0.1", port = {port} }}))
        res = assert(refusing:request({{
            method = "POST", path = "/refuse-continue", body = "never", headers = {{ Expect = "100-continue" }},
        }}))
        assert(res.status == 417)
        local code, why = refusing:set_keepalive()
        assert(code == 2 and why == "connection must be closed", why)

        local proxied = http.new()
        proxied:set_proxy_options({{
            http_proxy = "http://127.0.0.1:{port}",
            http_proxy_authorization = "Basic dGVzdA==",
            no_proxy = "localhost,.internal",
        }})
        res = assert(proxied:request_uri("http://upstream.test/proxied"))
        assert(res.body == "via proxy http://upstream.test/proxied Basic dGVzdA==", res.body)
        local tunneled = http.new()
        tunneled:set_proxy_options({{ https_proxy = "http://127.0.0.1:{port}" }})
        res, err = tunneled:request_uri("https://tls.test/hello", {{ ssl_verify = false }})
        assert(res and res.body == "hello", err)
        local bypassed = http.new()
        bypassed:set_proxy_options({{ http_proxy = "http://127.0.0.1:1", no_proxy = "127.0.0.1" }})
        assert(bypassed:request_uri("http://127.0.0.1:{port}/hello").body == "hello")
        ngx.say("ok")
        "#
    );
    let mut builder = Program::builder();
    let id = builder.handler(&Source::new("main.conf", &script, 1));
    let program = builder.build().expect("scripts compile");
    let (runtime, _) = Runtime::start(
        &program,
        &Settings {
            vms: 1,
            memory: 16 << 20,
        },
        &SharedStore::default(),
    )
    .expect("starts");
    let exchange = Exchange::new(
        Request {
            method: "GET".into(),
            uri: "/".into(),
            request_uri: "/".into(),
            headers: HeaderMap::new(),
            ..Request::default()
        },
        Connection::default(),
    );
    let mut handler = Handler::new(id, Phase::Content);
    handler.permissions.network = true;
    handler.limits.time = Duration::from_secs(10);
    let mut scripts = runtime.scripts(exchange);
    let outcome = scripts.run(handler, &mut NoHost).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"ok\n");
    assert!(
        accepted.load(Ordering::SeqCst) <= 7,
        "kept connections were not reused: {}",
        accepted.load(Ordering::SeqCst)
    );
}
