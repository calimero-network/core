use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use libp2p::identity::Keypair;
use multiaddr::Multiaddr;
#[cfg(feature = "http-server")]
use multiaddr::Protocol;

#[cfg(feature = "http-server")]
use crate::admin::service::AdminConfig;
#[cfg(feature = "http-server")]
use crate::jsonrpc::JsonRpcConfig;
#[cfg(feature = "http-server")]
use crate::sse::SseConfig;
#[cfg(feature = "http-server")]
use crate::ws::WsConfig;

pub const DEFAULT_PORT: u16 = 2528; // (CHAT in T9) + 100
pub const DEFAULT_ADDRS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ServerConfig {
    pub listen: Vec<Multiaddr>,

    pub identity: Keypair,

    #[cfg(feature = "http-server")]
    pub admin: Option<AdminConfig>,

    #[cfg(feature = "http-server")]
    pub jsonrpc: Option<JsonRpcConfig>,

    #[cfg(feature = "http-server")]
    pub websocket: Option<WsConfig>,

    #[cfg(feature = "http-server")]
    pub sse: Option<SseConfig>,
}

impl ServerConfig {
    // Server mode: Full constructor with HTTP config
    #[cfg(feature = "http-server")]
    #[must_use]
    pub const fn new(
        listen: Vec<Multiaddr>,
        identity: Keypair,
        admin: Option<AdminConfig>,
        jsonrpc: Option<JsonRpcConfig>,
        websocket: Option<WsConfig>,
        sse: Option<SseConfig>,
    ) -> Self {
        Self {
            listen,
            identity,
            admin,
            jsonrpc,
            websocket,
            sse,
        }
    }

    // Desktop mode: Minimal constructor
    #[cfg(not(feature = "http-server"))]
    #[must_use]
    pub const fn new(listen: Vec<Multiaddr>, identity: Keypair) -> Self {
        Self { listen, identity }
    }
}

#[cfg(feature = "http-server")]
#[must_use]
pub fn default_addrs() -> Vec<Multiaddr> {
    DEFAULT_ADDRS
        .into_iter()
        .map(|addr| Multiaddr::from(addr).with(Protocol::Tcp(DEFAULT_PORT)))
        .collect()
}
