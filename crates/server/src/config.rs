use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use multiaddr::Multiaddr;

pub const DEFAULT_PORT: u16 = 2528; // (CHAT in T9) + 100
pub const DEFAULT_ADDRS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

#[derive(Debug)]
pub struct ServerConfig {
    pub listen: Vec<Multiaddr>,

    pub identity: libp2p::identity::Keypair,

    #[cfg(feature = "admin")]
    pub admin: Option<crate::admin::AdminConfig>,

    #[cfg(feature = "graphql")]
    pub graphql: Option<crate::graphql::GraphQLConfig>,

    #[cfg(feature = "jsonrpc")]
    pub jsonrpc: Option<crate::jsonrpc::JsonRpcConfig>,

    #[cfg(feature = "websocket")]
    pub websocket: Option<crate::websocket::WsConfig>,
}

pub fn default_addrs() -> Vec<Multiaddr> {
    DEFAULT_ADDRS
        .into_iter()
        .map(|addr| Multiaddr::from(addr).with(multiaddr::Protocol::Tcp(DEFAULT_PORT)))
        .collect()
}
