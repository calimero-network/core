use core::fmt::{self, Formatter};
use core::time::Duration;

use libp2p::identity::Keypair;
use libp2p::rendezvous::Namespace;
use multiaddr::{Multiaddr, Protocol};
use serde::de::{Error as SerdeError, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const DEFAULT_PORT: u16 = 2428; // CHAT in T9

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
    "/ip4/18.156.18.6/udp/4001/quic-v1/p2p/12D3KooWMgoF9xzyeKJHtRvrYwdomheRbHPELagWZwTLmXb6bCVC",
    "/ip4/18.156.18.6/tcp/4001/p2p/12D3KooWMgoF9xzyeKJHtRvrYwdomheRbHPELagWZwTLmXb6bCVC",
];

#[derive(Debug)]
#[non_exhaustive]
pub struct NetworkConfig {
    pub identity: Keypair,

    pub swarm: SwarmConfig,
    pub bootstrap: BootstrapConfig,
    pub discovery: DiscoveryConfig,
    pub gossipsub: GossipsubConfig,
}

impl NetworkConfig {
    #[must_use]
    pub const fn new(
        identity: Keypair,
        swarm: SwarmConfig,
        bootstrap: BootstrapConfig,
        discovery: DiscoveryConfig,
        gossipsub: GossipsubConfig,
    ) -> Self {
        Self {
            identity,
            swarm,
            bootstrap,
            discovery,
            gossipsub,
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

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct DiscoveryConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub mdns: bool,

    pub advertise_address: bool,

    pub rendezvous: RendezvousConfig,

    pub relay: RelayConfig,

    pub autonat: AutonatConfig,
}

impl DiscoveryConfig {
    #[must_use]
    pub const fn new(
        mdns: bool,
        advertise_address: bool,
        rendezvous: RendezvousConfig,
        relay: RelayConfig,
        autonat: AutonatConfig,
    ) -> Self {
        Self {
            mdns,
            advertise_address,
            rendezvous,
            relay,
            autonat,
        }
    }
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            mdns: true,
            advertise_address: false,
            rendezvous: RendezvousConfig::default(),
            relay: RelayConfig::default(),
            autonat: AutonatConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GossipsubConfig {
    /// Minimum number of peers in mesh (D_low in spec)
    /// For 2-node networks, this should be 1
    pub mesh_n_low: usize,

    /// Target number of peers in mesh (D in spec)
    /// For small networks (2-5 nodes), 2 is reasonable
    pub mesh_n: usize,

    /// Maximum number of peers in mesh (D_high in spec)
    /// For larger networks, can go higher
    pub mesh_n_high: usize,

    /// Number of outbound-only peers to keep (D_out in spec)
    pub mesh_outbound_min: usize,

    /// Target for heartbeat interval in seconds
    pub heartbeat_interval_secs: u64,
}

impl GossipsubConfig {
    #[must_use]
    pub const fn new(
        mesh_n_low: usize,
        mesh_n: usize,
        mesh_n_high: usize,
        mesh_outbound_min: usize,
        heartbeat_interval_secs: u64,
    ) -> Self {
        Self {
            mesh_n_low,
            mesh_n,
            mesh_n_high,
            mesh_outbound_min,
            heartbeat_interval_secs,
        }
    }
}

impl Default for GossipsubConfig {
    fn default() -> Self {
        // These values are optimized for small networks (2-20 nodes)
        // while still providing good performance for larger networks
        Self {
            mesh_n_low: 1,              // Allow mesh with just 1 peer (for 2-node networks)
            mesh_n: 2,                  // Target 2 peers (works for 2-5 node networks)
            mesh_n_high: 4,             // Max 4 peers in mesh (good for up to 20 nodes)
            mesh_outbound_min: 1,       // Keep at least 1 outbound connection
            heartbeat_interval_secs: 1, // Default heartbeat
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
    pub confidence_threshold: usize,
}

impl AutonatConfig {
    #[must_use]
    pub const fn new(confidence_threshold: usize) -> Self {
        Self {
            confidence_threshold,
        }
    }
}

impl Default for AutonatConfig {
    fn default() -> Self {
        Self {
            confidence_threshold: 2,
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
            discovery_interval: Duration::from_secs(90),
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
