use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use multiaddr::Multiaddr;

pub const DEFAULT_PORT: u16 = 2528; // (CHAT in T9) + 100
pub const DEFAULT_ADDRS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

#[derive(Debug)]
#[non_exhaustive]
pub struct ServerConfig {
    pub listen: Vec<Multiaddr>,

    pub identity: libp2p::identity::Keypair,

    #[cfg(feature = "admin")]
    pub admin: Option<crate::admin::service::AdminConfig>,

    #[cfg(feature = "jsonrpc")]
    pub jsonrpc: Option<crate::jsonrpc::JsonRpcConfig>,

    #[cfg(feature = "websocket")]
    pub websocket: Option<crate::ws::WsConfig>,
}

impl ServerConfig {
    #[must_use]
    pub const fn new(
        listen: Vec<Multiaddr>,
        identity: libp2p::identity::Keypair,
        admin: Option<crate::admin::service::AdminConfig>,
        jsonrpc: Option<crate::jsonrpc::JsonRpcConfig>,
        websocket: Option<crate::ws::WsConfig>,
    ) -> Self {
        Self {
            listen,
            identity,
            admin,
            jsonrpc,
            websocket,
        }
    }
}

#[must_use]
pub fn default_addrs() -> Vec<Multiaddr> {
    DEFAULT_ADDRS
        .into_iter()
        .map(|addr| Multiaddr::from(addr).with(multiaddr::Protocol::Tcp(DEFAULT_PORT)))
        .collect()
}
