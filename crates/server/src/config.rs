use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use multiaddr::Multiaddr;
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 2528; // (CHAT in T9) + 100
pub const DEFAULT_ADDRS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub listen: Vec<Multiaddr>,

    #[serde(default)]
    #[cfg(feature = "graphql")]
    pub graphql: Option<crate::graphql::GraphQLConfig>,
}

pub fn default_addrs() -> Vec<Multiaddr> {
    DEFAULT_ADDRS
        .into_iter()
        .map(|addr| Multiaddr::from(addr).with(multiaddr::Protocol::Tcp(DEFAULT_PORT)))
        .collect()
}
