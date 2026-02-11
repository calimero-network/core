//! Constants for the Calimero relayer

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Default port for the relayer service (Mero-rELAY = MELAY)
pub const DEFAULT_PORT: u16 = 63529;

/// Default listen address
pub const DEFAULT_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_PORT);

/// Default relayer URL for client configuration
pub const DEFAULT_RELAYER_URL: &str = "http://localhost:63529";

// Protocol configuration constants
pub mod protocols {
    /// Near Protocol configuration
    pub mod near {
        pub const NAME: &str = "near";
        pub const DEFAULT_NETWORK: &str = "testnet";
        pub const DEFAULT_RPC_URL: &str = "https://rpc.testnet.near.org";
        pub const DEFAULT_CONTRACT_ID: &str = "v0-6.config.calimero-context.testnet";
        // Note: All credentials must be provided via environment variables
    }
}
