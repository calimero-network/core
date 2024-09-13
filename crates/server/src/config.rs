use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use libp2p::identity::Keypair;
use multiaddr::{Multiaddr, Protocol};

use crate::admin::service::AdminConfig;
use crate::jsonrpc::JsonRpcConfig;
use crate::ws::WsConfig;

pub const DEFAULT_PORT: u16 = 2528; // (CHAT in T9) + 100
pub const DEFAULT_ADDRS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

#[derive(Debug)]
pub struct ServerConfig {
    pub listen: Vec<Multiaddr>,

    pub identity: Keypair,

    #[cfg(feature = "admin")]
    pub admin: Option<AdminConfig>,

    #[cfg(feature = "jsonrpc")]
    pub jsonrpc: Option<JsonRpcConfig>,

    #[cfg(feature = "websocket")]
    pub websocket: Option<WsConfig>,
}

#[must_use]
pub fn default_addrs() -> Vec<Multiaddr> {
    DEFAULT_ADDRS
        .into_iter()
        .map(|addr| Multiaddr::from(addr).with(Protocol::Tcp(DEFAULT_PORT)))
        .collect()
}
