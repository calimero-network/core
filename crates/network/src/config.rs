use std::fmt;

use libp2p::identity;
use libp2p::multiaddr::{self, Multiaddr};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 2428;
pub const DEFAULT_RPC_HOST: &str = "127.0.0.1";
pub const DEFAULT_RPC_PORT: u16 = 3030;

// https://github.com/ipfs/kubo/blob/efdef7fdcfeeb30e2f1ce3dbf65b6460b58afaaf/config/bootstrap_peers.go#L17-L24
pub const IPFS_BOOT_NODES: &[&str] = &[
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
    "/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
    "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
];

#[derive(Debug)]

pub struct NetworkConfig {
    pub identity: identity::Keypair,
    pub node_type: calimero_primitives::types::NodeType,

    pub swarm: SwarmConfig,
    pub bootstrap: BootstrapConfig,
    pub discovery: DiscoveryConfig,
    pub endpoint: EndpointConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SwarmConfig {
    pub listen: Vec<Multiaddr>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BootstrapConfig {
    #[serde(default)]
    pub nodes: BootstrapNodes,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            nodes: Default::default(),
        }
    }
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
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    #[serde(default = "bool_true")]
    pub mdns: bool,
}

const fn bool_true() -> bool {
    true
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self { mdns: true }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EndpointConfig {
    pub host: String,
    pub port: u16,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_RPC_HOST.to_string(),
            port: DEFAULT_RPC_PORT,
        }
    }
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
