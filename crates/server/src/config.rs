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

/// Which browser pages and host names may reach the node (`[server.cors]`).
///
/// With nothing set, a request is served only under this node's own host
/// names, and from a browser only if the page is this node's own or on loopback.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CorsConfig {
    /// Exact origins, besides this node's own and loopback ones, whose pages
    /// may call the node.
    #[serde(default)]
    pub allowed_origins: Option<Vec<String>>,

    /// Host names, without a port, the node answers to besides its listen
    /// addresses and loopback: the names a proxy in front of it forwards.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,

    /// Whether to advertise `Access-Control-Allow-Private-Network`, which lets a
    /// more-public page among the allowed origins reach this (private) node.
    #[serde(default)]
    pub allow_private_network: bool,
}

/// How the node treats traffic that is not sealed to its attested transport key
/// (`[server.sealed]`). See [`crate::sealed`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SealedConfig {
    /// Refuse every request that is not sealed, except the few a client needs
    /// before it can seal anything: health and readiness probes,
    /// `GET /admin-api/tee/info`, which names the release to verify the quote
    /// against, and `POST /admin-api/tee/attest`, which is how it learns the
    /// transport key.
    ///
    /// Off by default, so sealing stays opt-in per client and an unsealed client
    /// still works. On a TEE node behind a proxy it does not trust, turn it on:
    /// then a client that forgets to seal gets a `403 sealed_required` instead
    /// of sending its bearer token through the proxy in the clear.
    #[serde(default)]
    pub required: bool,
}

impl SealedConfig {
    #[must_use]
    pub const fn new(required: bool) -> Self {
        Self { required }
    }

    #[must_use]
    pub const fn is_default(&self) -> bool {
        !self.required
    }
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl CorsConfig {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            allowed_origins: None,
            allowed_hosts: Vec::new(),
            allow_private_network: false,
        }
    }

    /// Fail fast on a misconfigured allowlist: every entry in `allowed_origins`
    /// must be a concrete origin (`scheme://host[:port]`). Called at startup so a
    /// typo in a security-sensitive origin list is an immediate, actionable error
    /// rather than a silently-narrowed (or empty) allowlist discovered at runtime.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(origins) = &self.allowed_origins {
            for origin in origins {
                if axum::http::HeaderValue::from_str(origin).is_err() {
                    return Err(format!(
                        "invalid entry in cors.allowed_origins (not a valid header value): \
                         {origin:?}"
                    ));
                }
                // Matched against the browser's `Origin` as a whole, so `*`, `null`, a
                // path or a missing scheme would silently match nothing.
                if !is_valid_origin(origin) {
                    return Err(format!(
                        "invalid entry in cors.allowed_origins (expected scheme://host[:port]): \
                         {origin:?}"
                    ));
                }
            }
        }
        // Compared with the request's host name alone, so a port or scheme here
        // would silently match nothing.
        if let Some(host) = self.allowed_hosts.iter().find(|host| {
            host.is_empty()
                || host.contains(['/', '*'])
                || crate::browser_origins::host_name(host) != host.as_str()
        }) {
            return Err(format!(
                "invalid entry in cors.allowed_hosts (expected a host name without a port): \
                 {host:?}"
            ));
        }
        Ok(())
    }
}

/// Whether `s` is a concrete CORS origin: `http`/`https` scheme, a non-empty
/// host[:port], and no path/query/fragment (browsers send `Origin` without a
/// trailing slash). Rejects `*`, `null`, and malformed values.
fn is_valid_origin(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once("://") else {
        return false;
    };
    matches!(scheme, "http" | "https")
        && !rest.is_empty()
        && !rest.contains('/')
        && !rest.contains('?')
        && !rest.contains('#')
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

    pub sealed: SealedConfig,

    /// Take an account-anchored caller's identity from the proxy in front of
    /// this node (`server.proxy_identity`). Read only under
    /// [`AuthMode::Proxy`]; see [`crate::proxy_identity`] for what it trusts.
    pub proxy_identity: bool,

    /// The mero-tee node release this node runs, when it is a fleet TEE node
    /// told so (`MERO_TEE_VERSION`). Not read from the config file.
    pub tee_release_version: Option<String>,
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
            cors: CorsConfig::new(),
            sealed: SealedConfig::new(false),
            proxy_identity: false,
            tee_release_version: None,
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
            cors: CorsConfig::new(),
            sealed: SealedConfig::new(false),
            proxy_identity: false,
            tee_release_version: None,
        }
    }

    #[must_use]
    pub fn use_embedded_auth(&self) -> bool {
        matches!(self.auth_mode, AuthMode::Embedded)
    }

    /// Whether the proxy's identity headers are read: only when asked for, and
    /// only in proxy mode, where no guard of this process names the caller.
    #[must_use]
    pub fn use_proxy_identity(&self) -> bool {
        self.proxy_identity && matches!(self.auth_mode, AuthMode::Proxy)
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

#[cfg(test)]
mod proxy_identity_tests {
    use libp2p::identity::Keypair;

    use super::{AuthMode, ServerConfig, ServiceConfigs};

    fn config(auth_mode: AuthMode, proxy_identity: bool) -> ServerConfig {
        let mut config = ServerConfig::with_auth(
            vec![],
            Keypair::generate_ed25519(),
            ServiceConfigs {
                admin: None,
                jsonrpc: None,
                websocket: None,
                sse: None,
            },
            auth_mode,
            None,
        );
        config.proxy_identity = proxy_identity;
        config
    }

    /// Off by default, and never read in embedded mode: there this process
    /// authenticates the caller itself, and a header would be a second,
    /// weaker answer to a question already settled.
    #[test]
    fn proxy_identity_applies_only_when_asked_for_in_proxy_mode() {
        assert!(!config(AuthMode::Proxy, false).use_proxy_identity());
        assert!(config(AuthMode::Proxy, true).use_proxy_identity());
        assert!(!config(AuthMode::Embedded, true).use_proxy_identity());
    }
}

#[cfg(test)]
mod cors_config_tests {
    use super::CorsConfig;

    #[test]
    fn allowed_hosts_must_be_bare_host_names() {
        let with_hosts = |host: &str| {
            CorsConfig {
                allowed_hosts: vec![host.to_owned()],
                ..CorsConfig::new()
            }
            .validate()
        };
        assert!(with_hosts("node.example").is_ok());
        assert!(with_hosts("[2001:db8::1]").is_ok());
        for bad in [
            "",
            "node.example:2528",
            "https://node.example",
            "[::1]:2528",
            "*.example",
        ] {
            assert!(with_hosts(bad).is_err(), "{bad:?} accepted");
        }
    }
}
