//! Calls of ports through the host (ADR 0044). A caller names the plugin
//! in `x-pingora-panel-plugin`; the host forwards the gRPC call to it when
//! it is enabled, provides and is granted the port, and has room for one
//! more call. A unary call gets the caller's deadline or the plugin's call
//! timeout, whichever is sooner; each message of a streaming call has the
//! call timeout to arrive, and the whole call the caller's deadline.

use crate::runtime::{Refusal, Runtime};
use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Request, Response};
use http_body::{Frame, SizeHint};
use std::{
    convert::Infallible,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{sync::OwnedSemaphorePermit, time::Sleep};
use tonic::{body::Body, server::NamedService, Status};
use tower::ServiceExt;

/// The metadata that names the plugin a call is for.
pub const PLUGIN_HEADER: &str = plugin_contracts::PLUGIN_METADATA;
const TIMEOUT_HEADER: &str = "grpc-timeout";
/// How close to its deadline a failed call counts as one that ran out of
/// time.
const DEADLINE_SLACK: Duration = Duration::from_millis(50);

/// A port's gRPC service.
pub trait Port: Send + Sync + 'static {
    /// The port, which is also the capability a plugin needs to be called.
    const PORT: &'static str;
    const SERVICE: &'static str;
    /// The paths of the service's streaming methods.
    const STREAMING: &'static [&'static str];
}

macro_rules! ports {
    ($($name:ident => $port:literal, $service:literal, [$($streaming:literal),*];)*) => {$(
        pub struct $name;

        impl Port for $name {
            const PORT: &'static str = $port;
            const SERVICE: &'static str = $service;
            const STREAMING: &'static [&'static str] = &[$($streaming),*];
        }
    )*};
}

ports! {
    Dns01 => "dns01", "pingora.panel.plugin.v1.Dns01Provider", [];
    Secrets => "secrets", "pingora.panel.plugin.v1.SecretProvider", [];
    Notifications => "notifications", "pingora.panel.plugin.v1.NotificationProvider", [];
    Backups => "backups", "pingora.panel.plugin.v1.BackupTarget", [
        "/pingora.panel.plugin.v1.BackupTarget/Put",
        "/pingora.panel.plugin.v1.BackupTarget/Get"
    ];
    Containers => "containers", "pingora.panel.ops.v1.Containers", [
        "/pingora.panel.ops.v1.Containers/FollowLogs"
    ];
    GatewayEngine => "gateway", "pingora.panel.gateway.v1.GatewayEngine", [];
    GatewayRuntime => "gateway", "pingora.panel.gateway.v1.GatewayRuntime", [];
}

/// The gRPC service of port `P` the host serves to the control plane.
pub struct PortProxy<P> {
    runtime: Arc<Runtime>,
    port: PhantomData<fn() -> P>,
}

impl<P> PortProxy<P> {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            port: PhantomData,
        }
    }
}

impl<P> Clone for PortProxy<P> {
    fn clone(&self) -> Self {
        Self::new(Arc::clone(&self.runtime))
    }
}

impl<P: Port> NamedService for PortProxy<P> {
    const NAME: &'static str = P::SERVICE;
}

impl<P: Port> tower::Service<Request<Body>> for PortProxy<P> {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let runtime = Arc::clone(&self.runtime);
        Box::pin(async move {
            Ok(forward::<P>(&runtime, request)
                .await
                .unwrap_or_else(Status::into_http))
        })
    }
}

async fn forward<P: Port>(
    runtime: &Runtime,
    mut request: Request<Body>,
) -> Result<Response<Body>, Status> {
    let name = request
        .headers()
        .get(PLUGIN_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| Status::invalid_argument(format!("name the plugin in {PLUGIN_HEADER}")))?
        .to_owned();
    let instance = runtime
        .get(&name)
        .ok_or_else(|| Status::failed_precondition(format!("plugin {name} is not enabled")))?;
    if !instance.provides(P::PORT) {
        return Err(Status::failed_precondition(format!(
            "plugin {name} does not provide {}",
            P::PORT
        )));
    }
    if !instance.is_granted(P::PORT) {
        return Err(Status::permission_denied(format!(
            "plugin {name} is not granted {}",
            P::PORT
        )));
    }
    let (permit, channel) = instance.admit().map_err(|refusal| match refusal {
        Refusal::Degraded(reason) => {
            Status::unavailable(format!("plugin {name} is degraded: {reason}"))
        }
        Refusal::Busy(most) => {
            Status::resource_exhausted(format!("plugin {name} already has {most} calls in flight"))
        }
    })?;
    let timeout = instance.call_timeout();
    let caller = deadline(request.headers());
    let streaming = P::STREAMING.contains(&request.uri().path());
    let wait = if streaming {
        caller
    } else {
        Some(caller.map_or(timeout, |caller| caller.min(timeout)))
    };
    let headers = request.headers_mut();
    headers.remove(PLUGIN_HEADER);
    match wait {
        Some(wait) => headers.insert(TIMEOUT_HEADER, encode(wait)),
        None => headers.remove(TIMEOUT_HEADER),
    };
    let idle = streaming.then_some(timeout);
    let request = request.map(|body| Body::new(Watched::new(body, idle, None)));
    let late = |wait: Duration| {
        Status::deadline_exceeded(format!(
            "plugin {name} did not answer within {} ms",
            wait.as_millis()
        ))
    };
    let started = tokio::time::Instant::now();
    let call = channel.oneshot(request);
    let response = match wait {
        Some(wait) => tokio::time::timeout(wait, call)
            .await
            .map_err(|_| late(wait))?,
        None => call.await,
    }
    .map_err(|error| match wait {
        // The plugin gives up at the deadline it was sent, as the host does.
        Some(wait) if started.elapsed() + DEADLINE_SLACK >= wait => late(wait),
        _ => Status::unavailable(format!("plugin {name} cannot be reached: {error}")),
    })?;
    Ok(response.map(|body| Body::new(Watched::new(body, idle, Some(permit)))))
}

/// The caller's `grpc-timeout`, when it sets one the host can read.
fn deadline(headers: &HeaderMap) -> Option<Duration> {
    let text = headers.get(TIMEOUT_HEADER)?.to_str().ok()?;
    let split = text.len().checked_sub(1)?;
    let (digits, unit) = text.split_at(split);
    if digits.is_empty() || digits.len() > 8 {
        return None;
    }
    let value: u64 = digits.parse().ok()?;
    Some(match unit {
        "H" => Duration::from_secs(value.saturating_mul(3600)),
        "M" => Duration::from_secs(value.saturating_mul(60)),
        "S" => Duration::from_secs(value),
        "m" => Duration::from_millis(value),
        "u" => Duration::from_micros(value),
        "n" => Duration::from_nanos(value),
        _ => return None,
    })
}

/// `duration` as a `grpc-timeout` value, in milliseconds.
fn encode(duration: Duration) -> HeaderValue {
    let milliseconds = duration.as_millis().clamp(1, 99_999_999);
    HeaderValue::from_str(&format!("{milliseconds}m")).expect("digits and a unit are a valid value")
}

/// A body whose frames each have `idle` to arrive, when set, holding a
/// call's slot until it ends.
struct Watched {
    body: Body,
    idle: Option<(Duration, Pin<Box<Sleep>>)>,
    _permit: Option<OwnedSemaphorePermit>,
}

impl Watched {
    fn new(body: Body, idle: Option<Duration>, permit: Option<OwnedSemaphorePermit>) -> Self {
        Self {
            body,
            idle: idle.map(|idle| (idle, Box::pin(tokio::time::sleep(idle)))),
            _permit: permit,
        }
    }
}

impl http_body::Body for Watched {
    type Data = Bytes;
    type Error = Status;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Status>>> {
        let this = self.get_mut();
        match Pin::new(&mut this.body).poll_frame(context) {
            Poll::Ready(frame) => {
                if let Some((idle, sleep)) = &mut this.idle {
                    sleep.as_mut().reset(tokio::time::Instant::now() + *idle);
                }
                Poll::Ready(frame)
            }
            Poll::Pending => {
                if let Some((idle, sleep)) = &mut this.idle {
                    if sleep.as_mut().poll(context).is_ready() {
                        return Poll::Ready(Some(Err(Status::deadline_exceeded(format!(
                            "no message of the call came within {} ms",
                            idle.as_millis()
                        )))));
                    }
                }
                Poll::Pending
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_read_in_every_unit_and_write_in_milliseconds() {
        let read = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(TIMEOUT_HEADER, HeaderValue::from_str(value).unwrap());
            deadline(&headers)
        };
        assert_eq!(read("2H"), Some(Duration::from_secs(7200)));
        assert_eq!(read("1500m"), Some(Duration::from_millis(1500)));
        assert_eq!(read("30S"), Some(Duration::from_secs(30)));
        assert_eq!(read("100u"), Some(Duration::from_micros(100)));
        assert_eq!(read("123456789m"), None);
        assert_eq!(read("10x"), None);
        assert_eq!(read("m"), None);
        assert_eq!(encode(Duration::from_secs(60)), "60000m");
        assert_eq!(encode(Duration::from_nanos(1)), "1m");
    }
}
