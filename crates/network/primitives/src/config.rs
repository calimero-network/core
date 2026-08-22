use core::fmt::{self, Formatter};
use core::time::Duration;

use libp2p::identity::Keypair;
use libp2p::rendezvous::Namespace;
use multiaddr::{Multiaddr, Protocol};
use serde::de::{Error as SerdeError, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const DEFAULT_PORT: u16 = 2428; // CHAT in T9

/// Gossipsub mesh sizing for Calimero's typical 2–20 peer clusters.
///
/// libp2p's defaults (`mesh_n_low=5`, `mesh_n=6`, `mesh_n_high=12`,
/// `mesh_outbound_min=2`) assume larger swarms. In a 3-node cluster, the
/// default `mesh_n_low=5` is permanently unreachable — every heartbeat
/// logs `Mesh low. Topic contains: 2 needs: 6` and re-runs
/// `get_random_peers` for no candidates. Matching the water marks to the
/// expected cluster size makes the mesh sit at steady state and the
/// `Mesh low` path quiet.
///
/// These are also read by the governance Phase-1 readiness gate
/// (`assert_transport_ready` in `crates/context/src/governance_broadcast`)
/// as the upper bound for `required = min(GOSSIPSUB_MESH_N_LOW,
/// known_subscribers)`. A mismatch between this constant and the value
/// passed to `gossipsub::ConfigBuilder::mesh_n_low` in
/// `crates/network/src/behaviour.rs` would either reject healthy
/// publishes (gate too high — the mesh never reaches the required size)
/// or admit publishes on an unhealthy mesh (gate too low). Keep them
/// in sync via this single source of truth.
pub const GOSSIPSUB_MESH_N_LOW: usize = 2;

/// Target mesh size per topic (gossipsub `mesh_n`). Heartbeat backfill
/// adds peers until the mesh reaches this size.
pub const GOSSIPSUB_MESH_N: usize = 4;

/// Upper bound before gossipsub prunes mesh peers (gossipsub `mesh_n_high`).
/// At 8 we leave headroom for clusters up to 9 (8 peers + self) without
/// pruning churn; the 8-node soak in `apps/scaffolding-e2e/workflows/
/// mesh-soak-8node.yml` empirically reaches max 7 per topic (7 other
/// peers in an 8-node cluster), staying below this cap.
pub const GOSSIPSUB_MESH_N_HIGH: usize = 8;

/// Minimum outbound mesh peers per topic. libp2p enforces the invariant
/// `mesh_outbound_min ≤ mesh_n_low / 2`, so with `mesh_n_low = 2` this
/// must be 1.
///
/// Security note: in larger public swarms, `mesh_outbound_min = 2` is
/// the conventional defence against an inbound-only Sybil cluster
/// monopolising a node's mesh. Calimero topics are namespace-gated by
/// signed governance membership (a non-member's gossipsub subscription
/// is accepted at the transport but their messages are rejected at the
/// governance/cryptographic layer in `state_delta` handling), so the
/// Sybil-via-subscription vector that motivates the default isn't load-
/// bearing here. The trade-off is explicit and bounded by the
/// `mesh_n_low/2` invariant rather than a free choice.
pub const GOSSIPSUB_MESH_OUTBOUND_MIN: usize = 1;

// https://github.com/ipfs/kubo/blob/efdef7fdcfeeb30e2f1ce3dbf65b6460b58afaaf/config/bootstrap_peers.go#L17-L24
pub const IPFS_BOOT_NODES: &[&str] = &[
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
    "/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
    "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
];

pub const CALIMERO_DEV_BOOT_NODES: &[&str] = &[
    "/ip4/63.181.86.34/udp/4001/quic-v1/p2p/12D3KooWR5V4zmisVtVdGE6i8jfFwtgRNq5t8eDGxfckKuhXu7Eh",
    "/ip4/63.181.86.34/tcp/4001/p2p/12D3KooWR5V4zmisVtVdGE6i8jfFwtgRNq5t8eDGxfckKuhXu7Eh",
];

#[derive(Debug)]
#[non_exhaustive]
pub struct NetworkConfig {
    pub identity: Keypair,
    pub swarm: SwarmConfig,
    pub bootstrap: BootstrapConfig,
    pub discovery: DiscoveryConfig,
}

impl NetworkConfig {
    #[must_use]
    pub const fn new(
        identity: Keypair,
        swarm: SwarmConfig,
        bootstrap: BootstrapConfig,
        discovery: DiscoveryConfig,
    ) -> Self {
        Self {
            identity,
            swarm,
            bootstrap,
            discovery,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct SwarmConfig {
    pub listen: Vec<Multiaddr>,
}

impl SwarmConfig {
    #[must_use]
    pub const fn new(listen: Vec<Multiaddr>) -> Self {
        Self { listen }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct BootstrapConfig {
    #[serde(default)]
    pub nodes: BootstrapNodes,
}

impl BootstrapConfig {
    #[must_use]
    pub const fn new(nodes: BootstrapNodes) -> Self {
        Self { nodes }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(transparent)]
#[non_exhaustive]
pub struct BootstrapNodes {
    #[serde(deserialize_with = "deserialize_bootstrap")]
    pub list: Vec<Multiaddr>,
}

impl BootstrapNodes {
    #[must_use]
    pub const fn new(list: Vec<Multiaddr>) -> Self {
        Self { list }
    }

    #[must_use]
    pub fn ipfs() -> Self {
        Self {
            list: IPFS_BOOT_NODES
                .iter()
                .map(|s| s.parse().expect("invalid multiaddr"))
                .collect(),
        }
    }

    #[must_use]
    pub fn calimero_dev() -> Self {
        Self {
            list: CALIMERO_DEV_BOOT_NODES
                .iter()
                .map(|s| s.parse().expect("invalid multiaddr"))
                .collect(),
        }
    }
}

/// Discovery mechanisms available to a node.
#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct DiscoveryConfig {
    /// Multicast (mDNS) peer discovery.
    ///
    /// One rule decides the defaults here: **a config that does not mention
    /// mDNS keeps it on.** So this `serde` default is `true`, and so is
    /// [`DiscoveryConfig::default`] — the latter matters because
    /// `calimero_config::NetworkConfig` marks its `discovery` field
    /// `#[serde(default)]`, so a file omitting the whole `[discovery]` table
    /// goes through `Default` and never reaches this attribute. Both paths have
    /// to agree or an existing node loses multicast to a version bump, with no
    /// config change and nothing in the release notes it reads.
    ///
    /// A NEW node gets the safe default instead, because `merod init` writes
    /// `mdns = false` explicitly — announcing yourself on the local network and
    /// dialling whoever answers is a local-development convenience and a
    /// tenancy question anywhere else. `merod init --mdns` opts in.
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub mdns: bool,

    pub advertise_address: bool,

    /// Operator-configured external addresses, seeded directly into the
    /// swarm's confirmed external-address set at init (gated on
    /// `advertise_address`). This is the deterministic alternative to
    /// AutoNAT-based discovery for known-static-IP / hosted deployments,
    /// and supports both IPv4 and IPv6. When empty, external addresses
    /// are discovered solely via identify + AutoNAT v2 confirmation.
    #[serde(default)]
    pub external_address: Vec<Multiaddr>,

    pub rendezvous: RendezvousConfig,

    pub relay: RelayConfig,

    pub autonat: AutonatConfig,
}

impl DiscoveryConfig {
    #[must_use]
    pub const fn new(
        mdns: bool,
        advertise_address: bool,
        external_address: Vec<Multiaddr>,
        rendezvous: RendezvousConfig,
        relay: RelayConfig,
        autonat: AutonatConfig,
    ) -> Self {
        Self {
            mdns,
            advertise_address,
            external_address,
            rendezvous,
            relay,
            autonat,
        }
    }
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            // `true`, matching the serde default on the field rather than
            // `merod init`. This is the value a config file omitting the whole
            // `[discovery]` table resolves to, so it decides what an EXISTING
            // node does after an upgrade — not what a new one is given.
            mdns: true,
            advertise_address: false,
            external_address: Vec::new(),
            rendezvous: RendezvousConfig::default(),
            relay: RelayConfig::default(),
            autonat: AutonatConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RelayConfig {
    pub registrations_limit: usize,
}

impl RelayConfig {
    #[must_use]
    pub const fn new(registrations_limit: usize) -> Self {
        Self {
            registrations_limit,
        }
    }
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            registrations_limit: 3,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct AutonatConfig {
    pub max_candidates: usize,
    pub probe_interval: Duration,
}

impl AutonatConfig {
    #[must_use]
    pub const fn new(max_candidates: usize, probe_interval: Duration) -> Self {
        Self {
            max_candidates,
            probe_interval,
        }
    }
}

impl Default for AutonatConfig {
    fn default() -> Self {
        Self {
            max_candidates: 5,
            probe_interval: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RendezvousConfig {
    #[serde(
        serialize_with = "serialize_rendezvous_namespace",
        deserialize_with = "deserialize_rendezvous_namespace"
    )]
    pub namespace: Namespace,

    pub discovery_rpm: f32,

    pub discovery_interval: Duration,

    pub registrations_limit: usize,
}

impl RendezvousConfig {
    #[must_use]
    pub fn new(registrations_limit: usize) -> Self {
        let default = Self::default();
        Self {
            namespace: default.namespace,
            discovery_rpm: default.discovery_rpm,
            discovery_interval: default.discovery_interval,
            registrations_limit,
        }
    }
}

impl Default for RendezvousConfig {
    fn default() -> Self {
        Self {
            namespace: Namespace::from_static("/calimero/devnet/global"),
            discovery_rpm: 0.5,
            // Was 90s before #2469's NAT-recovery investigation.
            // Cut to 15s so the periodic-tick recovery path picks
            // up post-restart peer registrations within ~15s of
            // the throttle window expiring, instead of waiting up
            // to 90s for the next aligned tick. The throttle
            // (`discovery_rpm = 0.5` = 120s floor per peer) still
            // gates against hammering the rendezvous server in
            // steady state — only one query goes out per 120s in
            // the no-event case. The faster tick only matters
            // when a recent event (peer disconnect, fresh
            // registration) makes a new query worth doing; the
            // throttle naturally suppresses the rest.
            discovery_interval: Duration::from_secs(15),
            registrations_limit: 3,
        }
    }
}

fn serialize_rendezvous_namespace<S>(
    namespace: &Namespace,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let namespace_str = namespace.to_string();
    serializer.serialize_str(&namespace_str)
}

fn deserialize_rendezvous_namespace<'de, D>(deserializer: D) -> Result<Namespace, D::Error>
where
    D: Deserializer<'de>,
{
    let namespace_str = String::deserialize(deserializer)?;
    Namespace::new(namespace_str).map_err(SerdeError::custom)
}

fn deserialize_bootstrap<'de, D>(deserializer: D) -> Result<Vec<Multiaddr>, D::Error>
where
    D: Deserializer<'de>,
{
    struct BootstrapVisitor;

    impl<'de> Visitor<'de> for BootstrapVisitor {
        type Value = Vec<Multiaddr>;

        fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
            formatter.write_str("a list of multiaddresses")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut addrs = Vec::new();

            while let Some(addr) = seq.next_element::<Multiaddr>()? {
                let Some(Protocol::P2p(_)) = addr.iter().last() else {
                    return Err(SerdeError::custom("peer ID not allowed"));
                };

                addrs.push(addr);
            }

            Ok(addrs)
        }
    }

    deserializer.deserialize_seq(BootstrapVisitor)
}

#[cfg(test)]
mod tests {
    use super::DiscoveryConfig;

    /// The smallest `[discovery]` table that parses, minus the `mdns` line the
    /// caller supplies. Every sub-table has required fields, so a fixture that
    /// omits them fails on those instead of exercising the mdns default.
    fn discovery_toml(mdns_line: &str) -> String {
        format!(
            r#"
            {mdns_line}
            advertise_address = false

            [rendezvous]
            namespace = "calimero/test"
            discovery_rpm = 0.5
            discovery_interval = {{ secs = 90, nanos = 0 }}
            registrations_limit = 3

            [relay]
            registrations_limit = 3

            [autonat]
            max_candidates = 5
            probe_interval = {{ secs = 90, nanos = 0 }}
            "#
        )
    }

    #[test]
    fn discovery_default_matches_the_serde_default() {
        // Both must say "on", because `calimero_config::NetworkConfig` marks
        // its `discovery` field `#[serde(default)]`: a config omitting the whole
        // `[discovery]` table resolves through `Default`, bypassing the
        // field-level attribute. If these two disagree, an existing node with no
        // `[discovery]` section loses multicast on upgrade. What gives a NEW
        // node the safe default is `merod init`, which writes the key.
        assert!(
            DiscoveryConfig::default().mdns,
            "Default must agree with the serde default, or an omitted \
             [discovery] table silently changes an existing node's behaviour"
        );
    }

    #[test]
    fn a_config_omitting_mdns_still_deserialises_to_on() {
        // Deliberately NOT the same as the default above. This is the upgrade
        // path for a node whose config predates the field: it keeps the
        // behaviour it has been running with rather than losing multicast to a
        // version bump. Changing this to `false` is a behaviour change to
        // existing deployments, not a default change — if that is ever wanted
        // it needs its own release note, and this test should fail loudly first.
        let cfg: DiscoveryConfig =
            toml::from_str(&discovery_toml("")).expect("a config omitting mdns must still parse");

        assert!(
            cfg.mdns,
            "an existing config without the key must keep multicast on"
        );
    }

    #[test]
    fn an_explicit_mdns_value_wins_over_both_defaults() {
        for (toml_value, expected) in [("true", true), ("false", false)] {
            let cfg: DiscoveryConfig =
                toml::from_str(&discovery_toml(&format!("mdns = {toml_value}")))
                    .expect("explicit mdns must parse");
            assert_eq!(cfg.mdns, expected, "mdns = {toml_value} must be honoured");
        }
    }
}
