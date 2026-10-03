//! Listening sockets and the fixed listener set of a data plane generation.

use panel_errors::{PanelError, Result};
use panel_ir::ListenerRef;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io,
    net::{SocketAddr, TcpListener},
};

const BACKLOG: i32 = 65_535;

/// Socket options fixed at bind time; changing any of them needs a new socket.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct SocketKey {
    pub address: SocketAddr,
    pub reuse_port: bool,
    pub ipv6_only: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ListenerPlan {
    pub id: String,
    pub socket: SocketKey,
    pub tls: bool,
    pub http1: bool,
    pub http2: bool,
}

impl ListenerPlan {
    pub(crate) fn from_ir(listener: &ListenerRef) -> Result<Self> {
        let address = listener.address.parse().map_err(|_| {
            PanelError::validation_failed(format!(
                "listener {} address {:?} is not an IP socket address",
                listener.id, listener.address
            ))
        })?;
        Ok(Self {
            id: listener.id.clone(),
            socket: SocketKey {
                address,
                reuse_port: listener.reuse_port,
                ipv6_only: listener.ipv6_only,
            },
            tls: listener.tls_profile_id.is_some(),
            http1: listener.protocols.http1,
            http2: listener.protocols.http2,
        })
    }
}

pub(crate) fn bind(key: &SocketKey) -> io::Result<TcpListener> {
    let socket = Socket::new(
        Domain::for_address(key.address),
        Type::STREAM,
        Some(Protocol::TCP),
    )?;
    #[cfg(unix)]
    {
        socket.set_reuse_address(true)?;
        if key.reuse_port {
            socket.set_reuse_port(true)?;
        }
    }
    if let (Some(only), true) = (key.ipv6_only, key.address.is_ipv6()) {
        socket.set_only_v6(only)?;
    }
    socket.bind(&key.address.into())?;
    socket.listen(BACKLOG)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_carry_socket_options_and_protocols() {
        let mut listener = ListenerRef::new("https", "[::]:0");
        listener.tls_profile_id = Some("tls".into());
        listener.reuse_port = true;
        listener.ipv6_only = Some(true);
        listener.protocols.http1 = false;
        let plan = ListenerPlan::from_ir(&listener).unwrap();
        assert!(plan.tls && !plan.http1 && plan.http2);
        assert!(plan.socket.reuse_port);
        assert_eq!(plan.socket.ipv6_only, Some(true));
        assert!(ListenerPlan::from_ir(&ListenerRef::new("bad", "localhost:80")).is_err());
    }

    #[test]
    fn occupied_addresses_fail_to_bind() {
        let key = SocketKey {
            address: "127.0.0.1:0".parse().unwrap(),
            reuse_port: false,
            ipv6_only: None,
        };
        let first = bind(&key).unwrap();
        let taken = SocketKey {
            address: first.local_addr().unwrap(),
            ..key
        };
        assert_eq!(bind(&taken).unwrap_err().kind(), io::ErrorKind::AddrInUse);
    }
}
