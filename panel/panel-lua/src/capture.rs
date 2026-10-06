//! The subrequests of `ngx.location.capture` and `capture_multi`, as the
//! host is asked to make them and as their responses come back.

use bytes::Bytes;
use http::{HeaderMap, Method};
use std::collections::HashMap;

/// The most subrequests one `capture_multi` makes at once.
pub const MOST_CAPTURES: usize = 200;

/// How deep subrequests may make subrequests of their own, as nginx allows
/// fifty.
pub const MOST_DEPTH: usize = 50;

/// A subrequest a script makes, to a route of the request's site.
#[derive(Debug)]
#[non_exhaustive]
pub struct Capture {
    pub method: Method,
    /// The path, without the query.
    pub path: String,
    pub args: Option<String>,
    /// What the subrequest sends: the script's `body`, or the request's own
    /// body when it is forwarded.
    pub body: Option<Bytes>,
    /// What its variables start as (`vars`, `copy_all_vars`).
    pub variables: HashMap<String, String>,
    /// Its variables come back to the request when it ends
    /// (`share_all_vars`).
    pub share_variables: bool,
    /// The VM and `ngx.ctx` its scripts run with (`ctx`).
    pub share: Option<Share>,
}

/// The VM and `ngx.ctx` table a subrequest's scripts share with the script
/// that made it.
#[derive(Clone, Debug)]
pub struct Share {
    pub(crate) vm: usize,
    pub(crate) ctx: mlua::Table,
}

/// A subrequest's response.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Captured {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
    /// The body stopped early: the subrequest failed or ran past what a
    /// script may hold.
    pub truncated: bool,
    /// Its variables when it ended, for `share_all_vars`.
    pub variables: Option<HashMap<String, String>>,
}

impl Captured {
    pub fn new(status: u16, headers: HeaderMap, body: Bytes) -> Self {
        Self {
            status,
            headers,
            body,
            truncated: false,
            variables: None,
        }
    }
}
