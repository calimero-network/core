use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use libp2p::identity::Keypair;
use multiaddr::{Multiaddr, Protocol};

use crate::admin::service::AdminConfig;
use crate::jsonrpc::JsonRpcConfig;
use crate::sse::SseConfig;
use crate::ws::WsConfig;

use mero_auth::config::AuthConfig;
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 2528; // (CHAT in T9) + 100
pub const DEFAULT_ADDRS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    #[default]
    Proxy,
    Embedded,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ServerConfig {
    pub listen: Vec<Multiaddr>,

    pub identity: Keypair,

    pub admin: Option<AdminConfig>,

    pub jsonrpc: Option<JsonRpcConfig>,

    pub websocket: Option<WsConfig>,

    pub sse: Option<SseConfig>,

    pub auth_mode: AuthMode,

    pub embedded_auth: Option<AuthConfig>,
}

impl ServerConfig {
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
            auth_mode: AuthMode::Proxy,
            embedded_auth: None,
        }
    }

    #[must_use]
    pub const fn with_auth(
        listen: Vec<Multiaddr>,
        identity: Keypair,
        admin: Option<AdminConfig>,
        jsonrpc: Option<JsonRpcConfig>,
        websocket: Option<WsConfig>,
        sse: Option<SseConfig>,
        auth_mode: AuthMode,
        embedded_auth: Option<AuthConfig>,
    ) -> Self {
        Self {
            listen,
            identity,
            admin,
            jsonrpc,
            websocket,
            sse,
            auth_mode,
            embedded_auth,
        }
    }

    #[must_use]
    pub fn use_embedded_auth(&self) -> bool {
        matches!(self.auth_mode, AuthMode::Embedded)
    }

    #[must_use]
    pub fn embedded_auth_config(&self) -> Option<&AuthConfig> {
        self.embedded_auth.as_ref()
    }
}

#[must_use]
pub fn default_addrs() -> Vec<Multiaddr> {
    DEFAULT_ADDRS
        .into_iter()
        .map(|addr| Multiaddr::from(addr).with(Protocol::Tcp(DEFAULT_PORT)))
        .collect()
}
