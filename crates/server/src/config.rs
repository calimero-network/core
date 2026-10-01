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
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CorsConfig {
    /// Exact origins permitted to make cross-origin requests. `None` (the
    /// default) allows **any** origin. `Some(list)` restricts to that list.
    /// In [`AuthMode::Proxy`] the node's own origin and loopback pages are
    /// admitted too, and every other origin is refused whether or not this
    /// is set.
    #[serde(default)]
    pub allowed_origins: Option<Vec<String>>,

    /// Whether to advertise `Access-Control-Allow-Private-Network`, which lets a
    /// more-public page reach this (private) node. Defaults to `true` to
    /// preserve the historical behavior; set to `false` (together with an
    /// `allowed_origins` list) to remove the wildcard-origin + private-network
    /// combination that lets any website drive authenticated requests.
    /// In [`AuthMode::Proxy`] it is advertised only to origins listed in
    /// `allowed_origins`.
    #[serde(default = "default_allow_private_network")]
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
        Self {
            allowed_origins: None,
            allow_private_network: default_allow_private_network(),
        }
    }
}

impl CorsConfig {
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
                // tower-http compares allowlist entries against the browser's
                // `Origin` header by exact string, so entries must be concrete
                // origins. Reject `*`, `null`, path-bearing, or scheme-less
                // values — these are almost always operator mistakes and would
                // silently match nothing.
                if !is_valid_origin(origin) {
                    return Err(format!(
                        "invalid entry in cors.allowed_origins (expected scheme://host[:port]): \
                         {origin:?}"
                    ));
                }
            }
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
            cors: CorsConfig {
                allowed_origins: None,
                allow_private_network: true,
            },
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
            cors: CorsConfig {
                allowed_origins: None,
                allow_private_network: true,
            },
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
