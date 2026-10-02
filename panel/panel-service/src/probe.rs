use axum::body::Body;
use hyper::{client::conn::http1, Request};
use hyper_util::rt::TokioIo;
use std::{net::SocketAddr, time::Duration};
use tokio::net::TcpStream;

/// Requests `path` from a local HTTP/1.1 endpoint and reports whether it
/// answered 2xx within `timeout`.
///
/// Container health checks run the service binary itself with a probe
/// argument, because minimal images ship no HTTP client.
pub async fn probe_http(address: SocketAddr, path: &str, timeout: Duration) -> bool {
    tokio::time::timeout(timeout, request(address, path))
        .await
        .ok()
        .flatten()
        .is_some_and(|status| status.is_success())
}

async fn request(address: SocketAddr, path: &str) -> Option<hyper::StatusCode> {
    let stream = TcpStream::connect(address).await.ok()?;
    let (mut sender, connection) = http1::handshake(TokioIo::new(stream)).await.ok()?;
    let connection = tokio::spawn(connection);
    let request = Request::get(path)
        .header(hyper::header::HOST, address.to_string())
        .body(Body::empty())
        .ok()?;
    let status = sender.send_request(request).await.ok()?.status();
    connection.abort();
    Some(status)
}
