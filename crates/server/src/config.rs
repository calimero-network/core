use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use multiaddr::Multiaddr;
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 2528; // (CHAT in T9) + 100
pub const DEFAULT_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_PORT);

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub listen: Vec<Multiaddr>,

    #[serde(default)]
    #[cfg(feature = "graphql")]
    pub graphql: Option<crate::graphql::GraphQLConfig>,
}
