//! The subrequests of `ngx.location.capture` (ADR 0039), made with
//! Pingora's: each goes through this gateway as a request of the same site
//! does, with the method, URI, body and variables the script gave it, and
//! its response comes back to the script.

use bytes::{Bytes, BytesMut};
use http::{header, HeaderMap, Uri};
use panel_lua::{Capture, Captured, Share};
use parking_lot::Mutex;
use pingora_core::protocols::http::{subrequest::server::SubrequestHandle, HttpTask};
use pingora_proxy::{
    subrequest::{BodyMode, Ctx},
    PreparedSubrequest, Session, SubrequestSpawner,
};
use std::collections::HashMap;
use tokio::sync::oneshot;

/// The most of a subrequest's response a script gets; what follows is cut
/// off and the response marked truncated.
const MOST_BODY: usize = 16 << 20;

/// What a subrequest is made with, handed to it through Pingora's
/// subrequest context.
struct Spec {
    capture: Capture,
    depth: usize,
    variables: oneshot::Sender<HashMap<String, String>>,
}

/// What a subrequest keeps of the script that made it.
pub(crate) struct Subrequest {
    /// The VM and `ngx.ctx` its scripts share with that script.
    pub share: Option<Share>,
    /// How deep it is: one for a request's subrequest.
    pub depth: usize,
    /// Where its variables go when it ends, for `share_all_vars`.
    variables: Option<oneshot::Sender<HashMap<String, String>>>,
}

impl Subrequest {
    /// Gives the request that made it the variables it ended with.
    pub(crate) fn finish(&mut self, variables: HashMap<String, String>) {
        if let Some(back) = self.variables.take() {
            let _ = back.send(variables);
        }
    }
}

/// Makes the subrequest `capture` describes, at `depth`, from `session`.
fn prepare(
    spawner: &SubrequestSpawner,
    session: &Session,
    capture: Capture,
    depth: usize,
) -> (
    PreparedSubrequest,
    SubrequestHandle,
    Option<Bytes>,
    oneshot::Receiver<HashMap<String, String>>,
) {
    let body = capture.body.clone();
    let (variables, back) = oneshot::channel();
    let spec = Spec {
        capture,
        depth,
        variables,
    };
    let context = Ctx::builder()
        .user_ctx(Box::new(Mutex::new(Some(spec))))
        .body_mode(if body.is_some() {
            BodyMode::ExpectBody
        } else {
            BodyMode::NoBody
        })
        .build();
    let (prepared, handle) = spawner.create_subrequest(&session.downstream_session, context);
    (prepared, handle, body, back)
}

/// Runs a prepared subrequest and gathers its response.
async fn run(
    prepared: PreparedSubrequest,
    handle: SubrequestHandle,
    body: Option<Bytes>,
    variables: oneshot::Receiver<HashMap<String, String>>,
) -> Captured {
    let SubrequestHandle {
        tx,
        mut rx,
        subreq_wants_body,
        ..
    } = handle;
    let feed = async move {
        if let Some(body) = body {
            if subreq_wants_body.await.is_ok() {
                let _ = tx.send(HttpTask::Body(Some(body), true)).await;
            }
        }
    };
    // The subrequest ends once it has written everything; reading until its
    // channel closes keeps it from waiting on a full one.
    let gather = async move {
        let mut status = 0;
        let mut headers = HeaderMap::new();
        let mut body = BytesMut::new();
        let mut truncated = false;
        while let Some(task) = rx.recv().await {
            match task {
                HttpTask::Header(header, _) => {
                    status = header.status.as_u16();
                    headers = header.headers.clone();
                }
                HttpTask::Body(Some(chunk), _) | HttpTask::UpgradedBody(Some(chunk), _) => {
                    if body.len() + chunk.len() > MOST_BODY {
                        truncated = true;
                    } else {
                        body.extend_from_slice(&chunk);
                    }
                }
                HttpTask::Failed(_) => truncated = true,
                _ => {}
            }
        }
        let mut captured = Captured::new(
            if status == 0 { 500 } else { status },
            headers,
            body.freeze(),
        );
        captured.truncated = truncated || status == 0;
        captured
    };
    let ((), (), mut captured) = tokio::join!(prepared.run(), feed, gather);
    captured.variables = variables.await.ok();
    captured
}

/// Makes `requests` from `session` at once and gives their responses in
/// order, `depth` being how deep `session` is.
pub(crate) async fn capture(
    session: &Session,
    requests: Vec<Capture>,
    depth: usize,
) -> Result<Vec<Captured>, String> {
    if depth >= panel_lua::MOST_DEPTH {
        return Err("subrequests cycle while processing the request".into());
    }
    let spawner = session
        .subrequest_spawner
        .as_ref()
        .ok_or("subrequests cannot be made from this request")?;
    let mut runs = tokio::task::JoinSet::new();
    let count = requests.len();
    for (index, capture) in requests.into_iter().enumerate() {
        let (prepared, handle, body, variables) = prepare(spawner, session, capture, depth + 1);
        runs.spawn(async move { (index, run(prepared, handle, body, variables).await) });
    }
    let mut captured: Vec<Option<Captured>> = (0..count).map(|_| None).collect();
    while let Some(done) = runs.join_next().await {
        let (index, response) = done.map_err(|error| format!("a subrequest failed: {error}"))?;
        captured[index] = Some(response);
    }
    Ok(captured.into_iter().flatten().collect())
}

/// Turns a subrequest's session into the request its script described,
/// and gives what the request keeps of it. `None` for the requests of
/// clients.
pub(crate) fn begin(session: &mut Session) -> Option<(Subrequest, HashMap<String, String>)> {
    let spec = session
        .subrequest_ctx
        .as_mut()?
        .user_ctx_mut()?
        .downcast_mut::<Mutex<Option<Spec>>>()?
        .lock()
        .take()?;
    let Spec {
        capture,
        depth,
        variables,
    } = spec;
    let target = match &capture.args {
        Some(args) if !args.is_empty() => format!("{}?{args}", capture.path),
        _ => capture.path.clone(),
    };
    let request = session.req_header_mut();
    request.set_method(capture.method.clone());
    if let Ok(uri) = target.parse::<Uri>() {
        request.set_uri(uri);
    }
    request.remove_header(&header::TRANSFER_ENCODING);
    match &capture.body {
        Some(body) => {
            let _ = request.insert_header(header::CONTENT_LENGTH, body.len().to_string());
        }
        None => {
            request.remove_header(&header::CONTENT_LENGTH);
        }
    }
    Some((
        Subrequest {
            share: capture.share,
            depth,
            variables: capture.share_variables.then_some(variables),
        },
        capture.variables,
    ))
}
