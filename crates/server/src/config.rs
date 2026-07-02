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

const fn default_allow_private_network() -> bool {
    // Preserve historical behavior when unset (see `CorsConfig`). Deployments
    // that don't need public-page → private-node access should set this to
    // `false` and configure `allowed_origins`.
    true
}

/// Cross-origin policy for the HTTP layer.
///
/// Defaults preserve the historical permissive behavior (any origin, private
/// network allowed) so existing browser apps / Tauri webviews keep working.
/// Production deployments should set an explicit `allowed_origins` list and set
/// `allow_private_network = false` — a wildcard origin combined with private
/// network access lets any visited website drive authenticated requests against
/// a local/private node once a token leaks into a URL (`?token=`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CorsConfig {
    /// Exact origins permitted to make cross-origin requests. `None` (the
    /// default) allows **any** origin. `Some(list)` restricts to that list.
    #[serde(default)]
    pub allowed_origins: Option<Vec<String>>,

    /// Whether to advertise `Access-Control-Allow-Private-Network`, which lets a
    /// more-public page reach this (private) node. Defaults to `true` to
    /// preserve the historical behavior; set to `false` (together with an
    /// `allowed_origins` list) to remove the wildcard-origin + private-network
    /// combination that lets any website drive authenticated requests.
    #[serde(default = "default_allow_private_network")]
    pub allow_private_network: bool,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: None,
            allow_private_network: default_allow_private_network(),
        }
    }
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

    pub cors: CorsConfig,
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
            cors: CorsConfig {
                allowed_origins: None,
                allow_private_network: true,
            },
        }
    }

    #[must_use]
    pub const fn with_auth(
        listen: Vec<Multiaddr>,
        identity: Keypair,
        services: ServiceConfigs,
        auth_mode: AuthMode,
        embedded_auth: Option<AuthConfig>,
    ) -> Self {
        let ServiceConfigs {
            admin,
            jsonrpc,
            websocket,
            sse,
        } = services;
        Self {
            listen,
            identity,
            admin,
            jsonrpc,
            websocket,
            sse,
            auth_mode,
            embedded_auth,
            cors: CorsConfig {
                allowed_origins: None,
                allow_private_network: true,
            },
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

/// The optional per-service endpoint configs passed to [`ServerConfig::with_auth`].
pub struct ServiceConfigs {
    pub admin: Option<AdminConfig>,
    pub jsonrpc: Option<JsonRpcConfig>,
    pub websocket: Option<WsConfig>,
    pub sse: Option<SseConfig>,
}
