//! What a handler sends the client before it ends (`ngx.flush`, `ngx.eof`,
//! `ngx.send_headers` and output past what is kept): the response header
//! the first time, then the body as it comes.

use crate::{capture::Share, exchange::Exchange};
use bytes::Bytes;

/// Output a running handler sends on.
#[derive(Debug)]
#[non_exhaustive]
pub struct Output {
    /// The exchange as the handler has it, the first time: its response
    /// header goes out, through the header filter.
    pub header: Option<Box<Exchange>>,
    pub body: Bytes,
    /// `ngx.eof`: the response ends with this.
    pub last: bool,
    /// The VM and `ngx.ctx` the filters run with.
    pub share: Share,
}
