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

    /// Acknowledges that an external authenticating proxy fronts this node.
    /// In `Proxy` auth mode the node performs no authentication itself, so
    /// serving the admin API on a network-reachable address without this set
    /// would expose unauthenticated admin/governance operations — the node
    /// refuses to start in that case. Loopback-only binds are unaffected.
    pub allow_unauthenticated_admin: bool,
}

/// Whether a listen address is reachable beyond the local host.
///
/// Loopback (`127.0.0.0/8`, `::1`) is local-only. The unspecified address
/// (`0.0.0.0`, `::`) binds every interface and any routable IP is reachable,
/// so both count as exposed. An address with no IP component (e.g. `/dns/...`)
/// can't be proven local, so it is treated as exposed (fail-closed).
fn addr_is_network_exposed(addr: &Multiaddr) -> bool {
    for proto in addr.iter() {
        match proto {
            Protocol::Ip4(ip) => return !ip.is_loopback(),
            Protocol::Ip6(ip) => return !ip.is_loopback(),
            _ => {}
        }
    }
    true
}

/// Result of [`ServerConfig::admin_exposure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminExposure {
    /// Safe to serve: admin disabled, node-side auth enabled, or loopback-only.
    Safe,
    /// The mutating admin API would be served unauthenticated on the listed
    /// network-reachable address(es) with no explicit opt-in. Refuse to start.
    Refuse(Vec<Multiaddr>),
    /// Same exposure, but the operator set `allow_unauthenticated_admin` —
    /// proceed, but warn loudly.
    AllowedWithWarning(Vec<Multiaddr>),
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
            allow_unauthenticated_admin: false,
        }
    }

    #[must_use]
    #[allow(clippy::too_many_arguments, reason = "full server config constructor")]
    pub const fn with_auth(
        listen: Vec<Multiaddr>,
        identity: Keypair,
        admin: Option<AdminConfig>,
        jsonrpc: Option<JsonRpcConfig>,
        websocket: Option<WsConfig>,
        sse: Option<SseConfig>,
        auth_mode: AuthMode,
        embedded_auth: Option<AuthConfig>,
        allow_unauthenticated_admin: bool,
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
            allow_unauthenticated_admin,
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

    /// Classify whether starting would serve the mutating admin API
    /// unauthenticated on a network-reachable address.
    ///
    /// Only relevant when the admin API is enabled AND the node does not
    /// authenticate requests itself (`Proxy` mode, no embedded guard). A
    /// loopback-only bind is always [`AdminExposure::Safe`].
    #[must_use]
    pub fn admin_exposure(&self) -> AdminExposure {
        if self.admin.is_none() || self.use_embedded_auth() {
            return AdminExposure::Safe;
        }
        let exposed: Vec<Multiaddr> = self
            .listen
            .iter()
            .filter(|addr| addr_is_network_exposed(addr))
            .cloned()
            .collect();
        if exposed.is_empty() {
            AdminExposure::Safe
        } else if self.allow_unauthenticated_admin {
            AdminExposure::AllowedWithWarning(exposed)
        } else {
            AdminExposure::Refuse(exposed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ma(s: &str) -> Multiaddr {
        s.parse().expect("valid multiaddr")
    }

    fn cfg(listen: Vec<Multiaddr>, auth_mode: AuthMode, admin: bool, allow: bool) -> ServerConfig {
        ServerConfig::with_auth(
            listen,
            Keypair::generate_ed25519(),
            admin.then(|| AdminConfig::new(true)),
            None,
            None,
            None,
            auth_mode,
            None,
            allow,
        )
    }

    #[test]
    fn loopback_is_not_exposed() {
        assert!(!addr_is_network_exposed(&ma("/ip4/127.0.0.1/tcp/2528")));
        assert!(!addr_is_network_exposed(&ma("/ip6/::1/tcp/2528")));
    }

    #[test]
    fn unspecified_and_routable_are_exposed() {
        assert!(addr_is_network_exposed(&ma("/ip4/0.0.0.0/tcp/2528")));
        assert!(addr_is_network_exposed(&ma("/ip6/::/tcp/2528")));
        assert!(addr_is_network_exposed(&ma("/ip4/10.0.0.5/tcp/2528")));
    }

    #[test]
    fn address_without_ip_is_treated_as_exposed() {
        assert!(addr_is_network_exposed(&ma("/dns4/example.com/tcp/2528")));
    }

    #[test]
    fn loopback_only_is_safe_in_proxy_mode() {
        let c = cfg(
            vec![ma("/ip4/127.0.0.1/tcp/2528"), ma("/ip6/::1/tcp/2528")],
            AuthMode::Proxy,
            true,
            false,
        );
        assert_eq!(c.admin_exposure(), AdminExposure::Safe);
    }

    #[test]
    fn exposed_proxy_admin_without_optin_is_refused() {
        let c = cfg(
            vec![ma("/ip4/0.0.0.0/tcp/2528")],
            AuthMode::Proxy,
            true,
            false,
        );
        assert_eq!(
            c.admin_exposure(),
            AdminExposure::Refuse(vec![ma("/ip4/0.0.0.0/tcp/2528")])
        );
    }

    #[test]
    fn exposed_proxy_admin_with_optin_warns() {
        let c = cfg(
            vec![ma("/ip4/0.0.0.0/tcp/2528")],
            AuthMode::Proxy,
            true,
            true,
        );
        assert_eq!(
            c.admin_exposure(),
            AdminExposure::AllowedWithWarning(vec![ma("/ip4/0.0.0.0/tcp/2528")])
        );
    }

    #[test]
    fn embedded_auth_is_safe_even_when_exposed() {
        let c = cfg(
            vec![ma("/ip4/0.0.0.0/tcp/2528")],
            AuthMode::Embedded,
            true,
            false,
        );
        assert_eq!(c.admin_exposure(), AdminExposure::Safe);
    }

    #[test]
    fn admin_disabled_is_safe_even_when_exposed() {
        let c = cfg(
            vec![ma("/ip4/0.0.0.0/tcp/2528")],
            AuthMode::Proxy,
            false,
            false,
        );
        assert_eq!(c.admin_exposure(), AdminExposure::Safe);
    }

    #[test]
    fn only_exposed_addrs_are_reported() {
        let c = cfg(
            vec![ma("/ip4/127.0.0.1/tcp/2528"), ma("/ip4/0.0.0.0/tcp/2529")],
            AuthMode::Proxy,
            true,
            false,
        );
        assert_eq!(
            c.admin_exposure(),
            AdminExposure::Refuse(vec![ma("/ip4/0.0.0.0/tcp/2529")])
        );
    }
}

#[must_use]
pub fn default_addrs() -> Vec<Multiaddr> {
    DEFAULT_ADDRS
        .into_iter()
        .map(|addr| Multiaddr::from(addr).with(Protocol::Tcp(DEFAULT_PORT)))
        .collect()
}
