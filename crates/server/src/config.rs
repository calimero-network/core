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

    #[serde(
        with = "calimero_identity::config::serde_identity",
        default = "libp2p::identity::Keypair::generate_ed25519"
    )]
    pub identity: libp2p::identity::Keypair,

    #[serde(default)]
    #[cfg(feature = "graphql")]
    pub graphql: Option<crate::graphql::GraphQLConfig>,

    #[serde(default)]
    #[cfg(feature = "websocket")]
    pub websocket: Option<crate::websocket::WsConfig>,
}

pub fn default_addrs() -> Vec<Multiaddr> {
    DEFAULT_ADDRS
        .into_iter()
        .map(|addr| Multiaddr::from(addr).with(multiaddr::Protocol::Tcp(DEFAULT_PORT)))
        .collect()
}
