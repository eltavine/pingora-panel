//! Which processes listen on TCP ports, from `/proc` on Linux.

// Only Linux serves the capability; elsewhere the matching is built for
// its tests alone.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use panel_contracts::ops::v1::{
    AgentCapability, Capability, CapabilityState, Listener, ListeningProcess,
};
use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
};
use tonic::Status;

/// What is looked at when the caller names no port.
const DEFAULT_PORTS: [u16; 2] = [80, 443];
const MAX_PORTS: usize = 16;

/// The drop-in that enables the capability with its privileges.
const ENABLE: &str = "install listeners.conf, which grants CAP_DAC_READ_SEARCH and CAP_SYS_PTRACE";

/// A listening socket as the kernel lists it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Socket {
    pub(crate) local: SocketAddr,
    pub(crate) uid: u32,
    pub(crate) inode: u64,
}

/// A process and the listening sockets it holds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Holder {
    pub(crate) pid: i32,
    pub(crate) name: String,
    pub(crate) executable: Option<String>,
    pub(crate) uid: u32,
    pub(crate) sockets: Vec<u64>,
}

/// The ports asked for, or 80 and 443 when none is.
pub(crate) fn ports(requested: &[u32]) -> Result<BTreeSet<u16>, Status> {
    if requested.len() > MAX_PORTS {
        return Err(Status::invalid_argument(format!(
            "ask for at most {MAX_PORTS} ports"
        )));
    }
    if requested.is_empty() {
        return Ok(DEFAULT_PORTS.into_iter().collect());
    }
    requested
        .iter()
        .map(|port| {
            u16::try_from(*port)
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| Status::invalid_argument(format!("{port} is not a TCP port")))
        })
        .collect()
}

/// The sockets listening on `ports`, each with the processes that hold it,
/// by port and then address.
pub(crate) fn listeners(
    sockets: Vec<Socket>,
    holders: Vec<Holder>,
    ports: &BTreeSet<u16>,
) -> Vec<Listener> {
    let mut held: HashMap<u64, Vec<ListeningProcess>> = HashMap::new();
    for holder in holders {
        for inode in &holder.sockets {
            held.entry(*inode).or_default().push(ListeningProcess {
                pid: holder.pid,
                name: holder.name.clone(),
                executable: holder.executable.clone().unwrap_or_default(),
                uid: holder.uid,
            });
        }
    }
    let mut found: Vec<Listener> = sockets
        .into_iter()
        .filter(|socket| ports.contains(&socket.local.port()))
        .map(|socket| {
            let mut processes = held.remove(&socket.inode).unwrap_or_default();
            processes.sort_by_key(|process| process.pid);
            Listener {
                address: socket.local.ip().to_string(),
                port: socket.local.port().into(),
                uid: socket.uid,
                processes,
            }
        })
        .collect();
    found.sort_by(|left, right| (left.port, &left.address).cmp(&(right.port, &right.address)));
    found
}

/// Whether the capability is enabled, possible here and permitted.
pub(crate) fn capability(enabled: bool) -> AgentCapability {
    let (state, detail) = if !cfg!(target_os = "linux") {
        (
            CapabilityState::Unsupported,
            "port diagnostics read /proc, which only Linux has".to_owned(),
        )
    } else if !enabled {
        (CapabilityState::NotEnabled, ENABLE.to_owned())
    } else if !source::permitted() {
        (CapabilityState::Denied, ENABLE.to_owned())
    } else {
        (CapabilityState::Available, String::new())
    };
    AgentCapability {
        capability: Capability::Listeners.into(),
        state: state.into(),
        detail,
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{source, ListenerService};

#[cfg(not(target_os = "linux"))]
mod source {
    pub(crate) fn permitted() -> bool {
        false
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{listeners, ports, Holder, Socket};
    use panel_contracts::ops::v1::{self as wire, listeners_server::Listeners};
    use std::{collections::BTreeSet, time::SystemTime};
    use tonic::{Request, Response, Status};

    pub(crate) mod source {
        use super::{Holder, Socket};
        use procfs::{
            net::TcpState,
            process::{all_processes, FDTarget, Process},
            ProcError, ProcResult,
        };
        use std::collections::{BTreeSet, HashSet};

        /// Whether the agent may read the descriptors of processes not its
        /// own, the first process's included.
        pub(crate) fn permitted() -> bool {
            Process::new(1)
                .and_then(|process| process.fd().map(drop))
                .is_ok()
        }

        pub(crate) fn read(ports: &BTreeSet<u16>) -> ProcResult<(Vec<Socket>, Vec<Holder>)> {
            let sockets: Vec<Socket> = procfs::net::tcp()?
                .into_iter()
                .chain(procfs::net::tcp6()?)
                .filter(|entry| {
                    entry.state == TcpState::Listen && ports.contains(&entry.local_address.port())
                })
                .map(|entry| Socket {
                    local: entry.local_address,
                    uid: entry.uid,
                    inode: entry.inode,
                })
                .collect();
            let wanted: HashSet<u64> = sockets.iter().map(|socket| socket.inode).collect();
            let mut holders = Vec::new();
            if wanted.is_empty() {
                return Ok((sockets, holders));
            }
            for process in all_processes()? {
                let Ok(process) = process else { continue };
                let held: Vec<u64> = match process.fd() {
                    Ok(descriptors) => descriptors
                        .filter_map(Result::ok)
                        .filter_map(|descriptor| match descriptor.target {
                            FDTarget::Socket(inode) if wanted.contains(&inode) => Some(inode),
                            _ => None,
                        })
                        .collect(),
                    Err(ProcError::NotFound(_)) => continue,
                    Err(error) => {
                        tracing::debug!(pid = process.pid(), %error, "descriptors unreadable");
                        continue;
                    }
                };
                if held.is_empty() {
                    continue;
                }
                holders.push(Holder {
                    pid: process.pid(),
                    name: process.stat().map(|stat| stat.comm).unwrap_or_default(),
                    executable: process.exe().ok().map(|path| path.display().to_string()),
                    uid: process.uid().unwrap_or_default(),
                    sockets: held,
                });
            }
            Ok((sockets, holders))
        }
    }

    /// The listening sockets on the ports a caller names.
    pub(crate) struct ListenerService;

    #[tonic::async_trait]
    impl Listeners for ListenerService {
        async fn list(
            &self,
            request: Request<wire::ListenersListRequest>,
        ) -> Result<Response<wire::ListenersListResponse>, Status> {
            let ports: BTreeSet<u16> = ports(&request.into_inner().ports)?;
            let found = tokio::task::spawn_blocking(move || {
                source::read(&ports).map(|(sockets, holders)| listeners(sockets, holders, &ports))
            })
            .await
            .map_err(|_| Status::internal("reading /proc stopped"))?
            .map_err(|error| Status::unavailable(format!("cannot read /proc: {error}")))?;
            Ok(Response::new(wire::ListenersListResponse {
                observed_at: Some(SystemTime::now().into()),
                listeners: found,
                error: None,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_default_to_web_ports_and_refuse_nonsense() {
        assert_eq!(ports(&[]).unwrap(), BTreeSet::from([80, 443]));
        assert_eq!(ports(&[8443, 80, 80]).unwrap(), BTreeSet::from([80, 8443]));
        assert!(ports(&[0]).is_err());
        assert!(ports(&[65_536]).is_err());
        assert!(ports(&[1; 17]).is_err());
    }

    #[test]
    fn sockets_are_matched_to_the_processes_that_hold_them() {
        let socket = |local: &str, inode| Socket {
            local: local.parse().unwrap(),
            uid: 0,
            inode,
        };
        let found = listeners(
            vec![
                socket("[::]:443", 2),
                socket("0.0.0.0:443", 1),
                socket("127.0.0.1:80", 3),
                socket("0.0.0.0:22", 4),
            ],
            vec![
                Holder {
                    pid: 9,
                    name: "nginx".into(),
                    executable: Some("/usr/sbin/nginx".into()),
                    uid: 33,
                    sockets: vec![1, 2],
                },
                Holder {
                    pid: 7,
                    name: "nginx".into(),
                    executable: None,
                    uid: 0,
                    sockets: vec![1],
                },
            ],
            &BTreeSet::from([80, 443]),
        );
        let summary: Vec<_> = found
            .iter()
            .map(|listener| {
                (
                    listener.port,
                    listener.address.as_str(),
                    listener
                        .processes
                        .iter()
                        .map(|process| process.pid)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (80, "127.0.0.1", vec![]),
                (443, "0.0.0.0", vec![7, 9]),
                (443, "::", vec![9]),
            ]
        );
        assert_eq!(found[1].processes[1].executable, "/usr/sbin/nginx");
        assert_eq!(found[1].processes[0].executable, "");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_is_found_listening_on_its_own_port() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ports = BTreeSet::from([socket.local_addr().unwrap().port()]);
        let (sockets, holders) = source::read(&ports).unwrap();
        let found = listeners(sockets, holders, &ports);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].address, "127.0.0.1");
        let me = i32::try_from(std::process::id()).unwrap();
        assert!(found[0].processes.iter().any(|process| process.pid == me));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn other_platforms_say_the_capability_is_unsupported() {
        assert_eq!(capability(true).state(), CapabilityState::Unsupported);
    }
}
