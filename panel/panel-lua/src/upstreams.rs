//! The upstreams `ngx.upstream` reads and the peers scripts take down, as
//! the host that runs the scripts has them.

/// A server of an upstream, as its configuration gives it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct UpstreamServer {
    /// The address connections go to.
    pub addr: String,
    /// The address as the configuration names it.
    pub name: String,
    pub weight: u32,
    /// Failures within `fail_timeout` that take the server out of rotation.
    pub max_fails: u32,
    /// Seconds.
    pub fail_timeout: u32,
    pub backup: bool,
    /// The configuration keeps the server out of rotation.
    pub down: bool,
}

/// A peer of an upstream as connections find it now.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct UpstreamPeer {
    /// Its index among the upstream's primary or backup peers.
    pub id: usize,
    pub name: String,
    pub weight: u32,
    /// Failures since the last success.
    pub fails: u32,
    pub max_fails: u32,
    /// Seconds.
    pub fail_timeout: u32,
    /// A script or the configuration took the peer out of rotation.
    pub down: bool,
    /// Requests under way on it.
    pub conns: u32,
}

/// The upstreams of the configuration the scripts belong to.
pub trait Upstreams: Send + Sync {
    /// The names of the upstreams.
    fn names(&self) -> Vec<String>;

    /// The servers of `upstream`, primary and backup, or `None` when there
    /// is no such upstream.
    fn servers(&self, upstream: &str) -> Option<Vec<UpstreamServer>>;

    /// The primary or backup peers of `upstream`, or `None` when there is
    /// no such upstream.
    fn peers(&self, upstream: &str, backup: bool) -> Option<Vec<UpstreamPeer>>;

    /// Takes peer `id` of `upstream` out of rotation, or puts it back.
    fn set_peer_down(
        &self,
        upstream: &str,
        backup: bool,
        id: usize,
        down: bool,
    ) -> Result<(), String>;
}

/// No upstreams, for runtimes that proxy nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoUpstreams;

impl Upstreams for NoUpstreams {
    fn names(&self) -> Vec<String> {
        Vec::new()
    }

    fn servers(&self, _: &str) -> Option<Vec<UpstreamServer>> {
        None
    }

    fn peers(&self, _: &str, _: bool) -> Option<Vec<UpstreamPeer>> {
        None
    }

    fn set_peer_down(&self, _: &str, _: bool, _: usize, _: bool) -> Result<(), String> {
        Err("upstream not found".into())
    }
}
