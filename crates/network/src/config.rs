use std::{fmt, time};

use libp2p::{identity, rendezvous};
use multiaddr::Multiaddr;
use serde::{Deserialize, Serialize};

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
pub struct NetworkConfig {
    pub identity: identity::Keypair,
    pub node_type: calimero_node_primitives::NodeType,

    pub swarm: SwarmConfig,
    pub bootstrap: BootstrapConfig,
    pub discovery: DiscoveryConfig,
    pub catchup: CatchupConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SwarmConfig {
    pub listen: Vec<Multiaddr>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BootstrapConfig {
    #[serde(default)]
    pub nodes: BootstrapNodes,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BootstrapNodes {
    #[serde(deserialize_with = "deserialize_bootstrap")]
    pub list: Vec<Multiaddr>,
}

impl BootstrapNodes {
    pub fn ipfs() -> Self {
        Self {
            list: IPFS_BOOT_NODES
                .iter()
                .map(|s| s.parse().expect("invalid multiaddr"))
                .collect(),
        }
    }

    pub fn calimero_dev() -> Self {
        Self {
            list: CALIMERO_DEV_BOOT_NODES
                .iter()
                .map(|s| s.parse().expect("invalid multiaddr"))
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub mdns: bool,

    pub rendezvous: RendezvousConfig,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            mdns: true,
            rendezvous: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RendezvousConfig {
    #[serde(
        serialize_with = "serialize_rendezvous_namespace",
        deserialize_with = "deserialize_rendezvous_namespace"
    )]
    pub namespace: rendezvous::Namespace,

    pub discovery_rpm: f32,

    pub discovery_interval: time::Duration,
}

impl Default for RendezvousConfig {
    fn default() -> Self {
        Self {
            namespace: rendezvous::Namespace::from_static("/calimero/devnet/global"),
            discovery_rpm: 0.5,
            discovery_interval: time::Duration::from_secs(90),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatchupConfig {
    pub batch_size: u8,
    pub receive_timeout: time::Duration,
}

fn serialize_rendezvous_namespace<S>(
    namespace: &rendezvous::Namespace,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let namespace_str = namespace.to_string();
    serializer.serialize_str(&namespace_str)
}

fn deserialize_rendezvous_namespace<'de, D>(
    deserializer: D,
) -> Result<rendezvous::Namespace, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let namespace_str = String::deserialize(deserializer)?;
    rendezvous::Namespace::new(namespace_str).map_err(serde::de::Error::custom)
}

fn deserialize_bootstrap<'de, D>(deserializer: D) -> Result<Vec<Multiaddr>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct BootstrapVisitor;

    impl<'de> de::Visitor<'de> for BootstrapVisitor {
        type Value = Vec<Multiaddr>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a list of multiaddresses")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut addrs = Vec::new();

            while let Some(addr) = seq.next_element::<Multiaddr>()? {
                let Some(multiaddr::Protocol::P2p(_)) = addr.iter().last() else {
                    return Err(serde::de::Error::custom("peer ID not allowed"));
                };

                addrs.push(addr);
            }

            Ok(addrs)
        }
    }

    deserializer.deserialize_seq(BootstrapVisitor)
}
