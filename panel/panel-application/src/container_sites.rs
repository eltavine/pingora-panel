//! Where the gateway can reach containers, and the sites their labels
//! declare (ADR 0033).

use crate::{ContainerState, ContainerSummary};
use panel_domain::NormalizedHost;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr},
};

/// The label naming the hosts of the site a container backs, separated by
/// commas.
pub const SITE_DOMAINS_LABEL: &str = "pingora-panel.site.domains";
/// The label naming the container's port the site proxies to.
pub const SITE_PORT_LABEL: &str = "pingora-panel.site.port";
/// The label naming the site.
pub const SITE_NAME_LABEL: &str = "pingora-panel.site.name";

/// How the gateway reaches an endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EndpointRoute {
    /// A port the engine publishes on the host.
    Published,
    /// The container's address on this network.
    Network(String),
}

/// Where the gateway can reach one of a container's TCP ports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerEndpoint {
    pub address: IpAddr,
    pub port: u16,
    /// The container's own port behind it.
    pub container_port: u16,
    pub route: EndpointRoute,
}

/// A running container's endpoints: its published ports, a wildcard address
/// read as `127.0.0.1`, then each of its ports on each of its addresses.
pub fn endpoints(container: &ContainerSummary) -> Vec<ContainerEndpoint> {
    if container.state != ContainerState::Running {
        return Vec::new();
    }
    let tcp = || {
        container
            .ports
            .iter()
            .filter(|port| port.protocol.eq_ignore_ascii_case("tcp"))
    };
    let mut found: Vec<ContainerEndpoint> = Vec::new();
    for port in tcp() {
        let Some(public) = port.public_port else {
            continue;
        };
        let address = match port.host_ip.parse::<IpAddr>() {
            Ok(address) if !address.is_unspecified() => address,
            _ => IpAddr::V4(Ipv4Addr::LOCALHOST),
        };
        if !found
            .iter()
            .any(|known| known.address == address && known.port == public)
        {
            found.push(ContainerEndpoint {
                address,
                port: public,
                container_port: port.private_port,
                route: EndpointRoute::Published,
            });
        }
    }
    let own: BTreeSet<u16> = tcp().map(|port| port.private_port).collect();
    for on in &container.addresses {
        let addresses = [on.ipv4.map(IpAddr::V4), on.ipv6.map(IpAddr::V6)];
        for address in addresses.into_iter().flatten() {
            found.extend(own.iter().map(|port| ContainerEndpoint {
                address,
                port: *port,
                container_port: *port,
                route: EndpointRoute::Network(on.network.clone()),
            }));
        }
    }
    found
}

/// The endpoint a site for the container's `port` proxies to: published
/// first, since network addresses change when a container is recreated.
pub fn endpoint_for(container: &ContainerSummary, port: u16) -> Option<ContainerEndpoint> {
    endpoints(container)
        .into_iter()
        .find(|endpoint| endpoint.container_port == port)
}

/// What a container's labels say about the site it backs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeclaredSite {
    pub name: Option<String>,
    /// Normalized; hosts that are not valid are left out.
    pub domains: Vec<NormalizedHost>,
    /// The container's port, when the labels name one.
    pub port: Option<u16>,
}

/// The site a container's labels declare: none unless they name a valid host.
pub fn declared_site(labels: &BTreeMap<String, String>) -> Option<DeclaredSite> {
    let mut domains: Vec<NormalizedHost> = Vec::new();
    for host in labels.get(SITE_DOMAINS_LABEL)?.split(',') {
        if let Ok(host) = NormalizedHost::new(host.trim()) {
            if !domains.contains(&host) {
                domains.push(host);
            }
        }
    }
    if domains.is_empty() {
        return None;
    }
    Some(DeclaredSite {
        name: labels
            .get(SITE_NAME_LABEL)
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty()),
        domains,
        port: labels
            .get(SITE_PORT_LABEL)
            .and_then(|port| port.trim().parse().ok())
            .filter(|port| *port != 0),
    })
}

/// Whether `host` and `address` name the same host for a link: equal
/// addresses, or both loopback, `localhost` included.
pub fn same_host(host: &str, address: IpAddr) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(parsed) => parsed == address || (parsed.is_loopback() && address.is_loopback()),
        Err(_) => host.eq_ignore_ascii_case("localhost") && address.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContainerAddress, PortMapping};

    fn container(state: ContainerState) -> ContainerSummary {
        let port =
            |private_port, public_port: Option<u16>, host_ip: &str, protocol: &str| PortMapping {
                private_port,
                public_port,
                host_ip: host_ip.into(),
                protocol: protocol.into(),
            };
        ContainerSummary {
            id: "b2".into(),
            names: vec!["shop-web-1".into()],
            image: "nginx:1.27".into(),
            image_id: String::new(),
            created: None,
            state,
            status: String::new(),
            ports: vec![
                port(80, Some(8081), "0.0.0.0", "tcp"),
                port(80, Some(8081), "::", "tcp"),
                port(443, Some(8443), "127.0.0.1", "tcp"),
                port(9000, None, "", "tcp"),
                port(53, Some(5353), "0.0.0.0", "udp"),
            ],
            labels: BTreeMap::new(),
            compose_project: None,
            addresses: vec![ContainerAddress {
                network: "shop_default".into(),
                ipv4: Some([172, 18, 0, 2].into()),
                ipv6: None,
            }],
        }
    }

    #[test]
    fn published_ports_come_before_network_addresses() {
        let found: Vec<(String, u16, u16, bool)> = endpoints(&container(ContainerState::Running))
            .into_iter()
            .map(|endpoint| {
                (
                    endpoint.address.to_string(),
                    endpoint.port,
                    endpoint.container_port,
                    endpoint.route == EndpointRoute::Published,
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("127.0.0.1".to_owned(), 8081, 80, true),
                ("127.0.0.1".to_owned(), 8443, 443, true),
                ("172.18.0.2".to_owned(), 80, 80, false),
                ("172.18.0.2".to_owned(), 443, 443, false),
                ("172.18.0.2".to_owned(), 9000, 9000, false),
            ],
            "a wildcard is loopback once, and UDP is left out"
        );
        assert!(endpoints(&container(ContainerState::Exited)).is_empty());
        let web = container(ContainerState::Running);
        assert_eq!(endpoint_for(&web, 80).unwrap().port, 8081);
        assert_eq!(
            endpoint_for(&web, 9000).unwrap().address.to_string(),
            "172.18.0.2"
        );
        assert!(endpoint_for(&web, 22).is_none());
    }

    #[test]
    fn labels_declare_a_site_with_valid_hosts() {
        let labels = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect()
        };
        let declared = declared_site(&labels(&[
            (
                SITE_DOMAINS_LABEL,
                " Shop.Example , www.shop.example,shop.example, bad host",
            ),
            (SITE_PORT_LABEL, "80"),
            (SITE_NAME_LABEL, " shop "),
        ]))
        .unwrap();
        assert_eq!(
            declared
                .domains
                .iter()
                .map(NormalizedHost::as_str)
                .collect::<Vec<_>>(),
            ["shop.example", "www.shop.example"]
        );
        assert_eq!(
            (declared.name.as_deref(), declared.port),
            (Some("shop"), Some(80))
        );
        let unported = declared_site(&labels(&[
            (SITE_DOMAINS_LABEL, "shop.example"),
            (SITE_PORT_LABEL, "http"),
        ]))
        .unwrap();
        assert_eq!(unported.port, None);
        assert!(declared_site(&labels(&[(SITE_DOMAINS_LABEL, " , bad host")])).is_none());
        assert!(declared_site(&labels(&[(SITE_PORT_LABEL, "80")])).is_none());
    }

    #[test]
    fn loopback_names_one_host() {
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for host in ["127.0.0.1", "localhost", "LOCALHOST", "::1", "[::1]"] {
            assert!(same_host(host, loopback), "{host}");
        }
        let network: IpAddr = [172, 18, 0, 2].into();
        assert!(same_host("172.18.0.2", network));
        assert!(!same_host("172.18.0.3", network));
        assert!(!same_host("localhost", network));
        assert!(!same_host("shop.internal", loopback));
    }
}
