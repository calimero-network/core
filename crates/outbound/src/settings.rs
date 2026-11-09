//! The operator's outbound settings (`[outbound]` in merod's configuration, or its `--settings` file).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The operator's knobs for outbound HTTPS requests (`[outbound]`).
///
/// Off by default. When on, a run may make one request, only under a URL
/// prefix its app's signed manifest declares, only to a host allowed here,
/// and only with
/// secrets this node can supply for that app. See [`crate`].
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct OutboundSettings {
    /// Make outbound requests at all. `false` (the default): `http_request`
    /// traps in every run.
    pub enabled: bool,
    /// The hosts requests may go to, as DNS names; `"*"` allows any host an
    /// installed app's signed manifest declares. A request to any other host
    /// is refused, whatever the app declares. Every host is still public-only,
    /// and a secret still goes only to the hosts it is bound to.
    pub hosts: Vec<String>,
    /// Secrets from environment variables, by the name a request uses
    /// (`{{secret:NAME}}`). Exclusive with `sidecar`.
    pub env_secrets: BTreeMap<String, EnvSecretSettings>,
    /// Secrets from a loopback service next to the node. Exclusive with
    /// `env_secrets`.
    pub sidecar: Option<SidecarSettings>,
    /// Test builds only: answer `.test` hosts in-process, take literal secrets.
    #[cfg(feature = "outbound-testing")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub testing: Option<OutboundTestSettings>,
}

/// `[outbound.testing]`, compiled only with `outbound-testing`. A
/// release node refuses the table as an unknown field.
#[cfg(feature = "outbound-testing")]
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct OutboundTestSettings {
    /// Answer a request to a host ending in `.test` (RFC 2606: never a public
    /// name) in-process, with a JSON echo of what would have been sent, once
    /// every gate has passed and the secrets are filled in.
    pub echo: bool,
    /// Answer Cloudflare Turnstile's siteverify URL ([`crate::TURNSTILE_VERIFY_URL`])
    /// in-process, as Cloudflare answers its test secrets: `1x…AA` passes, `2x…AA` fails,
    /// `3x…AA` answers "timeout-or-duplicate". A token is spent by its first check, unless
    /// a later one repeats its idempotency key.
    pub turnstile: bool,
    /// Secrets given as values, scoped as `env_secrets` are. Exclusive with
    /// `env_secrets` and a sidecar.
    pub secrets: BTreeMap<String, TestSecretSettings>,
}

/// One literal test secret.
#[cfg(feature = "outbound-testing")]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TestSecretSettings {
    pub value: String,
    pub packages: Vec<String>,
    pub signers: Vec<String>,
    pub hosts: Vec<String>,
}

/// One secret from an environment variable.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvSecretSettings {
    /// The variable holding the value; read once at start.
    pub env: String,
    /// The app packages that may use it, e.g. `com.calimero.mero-forms`.
    pub packages: Vec<String>,
    /// The bundle signers (`did:key:…`) whose builds of those packages may use
    /// it. Both must match: a package name alone is not proof of who built it.
    pub signers: Vec<String>,
    /// The hosts it may be sent to.
    pub hosts: Vec<String>,
}

/// The loopback secrets service.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SidecarSettings {
    /// `http://127.0.0.1:<port>/…` or another loopback address.
    pub url: String,
    /// The variable holding the bearer token the node sends it.
    pub token_env: String,
}
