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

    /// Starknet Protocol configuration  
    pub mod starknet {
        pub const NAME: &str = "starknet";
        pub const DEFAULT_NETWORK: &str = "sepolia";
        pub const DEFAULT_RPC_URL: &str = "https://free-rpc.nethermind.io/sepolia-juno/";
        pub const DEFAULT_CONTRACT_ID: &str =
            "0x1b991ee006e2d1e372ab96d0a957401fa200358f317b681df2948f30e17c29c";
        // Note: All credentials must be provided via environment variables
    }

    /// ICP Protocol configuration
    pub mod icp {
        pub const NAME: &str = "icp";
        pub const DEFAULT_NETWORK: &str = "local";
        pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:4943";
        pub const DEFAULT_CONTRACT_ID: &str = "bkyz2-fmaaa-aaaaa-qaaaq-cai";
        // Note: All credentials must be provided via environment variables
    }

    /// Ethereum Protocol configuration
    pub mod ethereum {
        pub const NAME: &str = "ethereum";
        pub const DEFAULT_NETWORK: &str = "sepolia";
        pub const DEFAULT_RPC_URL: &str = "https://sepolia.drpc.org";
        pub const DEFAULT_CONTRACT_ID: &str = "0x83365DE41E1247511F4C5D10Fb1AFe59b96aD4dB";
        // Note: All credentials must be provided via environment variables
    }

    /// Mock Relayer Protocol configuration
    pub mod mock_relayer {
        pub const NAME: &str = "mock-relayer";
        pub const DEFAULT_NETWORK: &str = "local";
        pub const DEFAULT_RPC_URL: &str = "http://localhost:9812";
        pub const DEFAULT_CONTRACT_ID: &str = "mock-context-config";
        // Note: No credentials needed for mock protocol
    }
}
