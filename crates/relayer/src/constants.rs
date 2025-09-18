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
        pub const DEFAULT_CONTRACT_ID: &str = "calimero-context-config.testnet";

        /// Default testnet credentials (for testing only - production should use env vars)
        pub const DEFAULT_ACCOUNT_ID: &str = "test.testnet";
        pub const DEFAULT_PUBLIC_KEY: &str = "ed25519:HyFiHQkpBZ1PnWKz2DWZBGEGVcVHxuGBDMEi9EUceDxK";
        // Note: Secret key removed - must be provided via environment variables
    }

    /// Starknet Protocol configuration  
    pub mod starknet {

        pub const NAME: &str = "starknet";
        pub const DEFAULT_NETWORK: &str = "sepolia";
        pub const DEFAULT_RPC_URL: &str = "https://free-rpc.nethermind.io/sepolia-juno/";
        pub const DEFAULT_CONTRACT_ID: &str =
            "0x1b991ee006e2d1e372ab96d0a957401fa200358f317b681df2948f30e17c29c";

        /// Default testnet credentials (for testing only - production should use env vars)
        pub const DEFAULT_ACCOUNT_ID: &str =
            "0x01cf4d57ba01109f018dec3ea079a38fc08b0f8a78eed0d4c5e5fb22928dbc8c";
        pub const DEFAULT_PUBLIC_KEY: &str =
            "0x02c5dbad71c92a45cc4b40573ae661f8147869a91d57b8d9b8f48c8af7f83159";
        // Note: Secret key removed - must be provided via environment variables
    }

    /// ICP Protocol configuration
    pub mod icp {

        pub const NAME: &str = "icp";
        pub const DEFAULT_NETWORK: &str = "local";
        pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:4943";
        pub const DEFAULT_CONTRACT_ID: &str = "bkyz2-fmaaa-aaaaa-qaaaq-cai";

        /// Default local credentials (for testing only - production should use env vars)
        pub const DEFAULT_ACCOUNT_ID: &str = "rdmx6-jaaaa-aaaaa-aaadq-cai";
        pub const DEFAULT_PUBLIC_KEY: &str =
            "MCowBQYDK2VwAyEAL8XDEY1gGOWvv/0h01tW/ZV14qYY7GrHJF3pZoNxmHE=";
        // Note: Secret key removed - must be provided via environment variables
    }

    /// Ethereum Protocol configuration
    pub mod ethereum {

        pub const NAME: &str = "ethereum";
        pub const DEFAULT_NETWORK: &str = "sepolia";
        pub const DEFAULT_RPC_URL: &str = "https://sepolia.drpc.org";
        pub const DEFAULT_CONTRACT_ID: &str = "0x83365DE41E1247511F4C5D10Fb1AFe59b96aD4dB";

        /// Default testnet credentials (for testing only - production should use env vars)
        pub const DEFAULT_ACCOUNT_ID: &str = "0x8ba1f109551bD432803012645Hac136c22C177ec";
        // Note: Secret key removed - must be provided via environment variables
    }
}

// Dummy credentials for fallback when no real credentials are provided
pub mod dummy {
    /// Dummy credentials for protocols without explicit credentials
    pub mod near {
        pub const ACCOUNT_ID: &str = "dummy.testnet";
        pub const PUBLIC_KEY: &str = "ed25519:dummy";
        pub const SECRET_KEY: &str = "ed25519:dummy";
    }

    pub mod starknet {
        pub const ACCOUNT_ID: &str = "0x0";
        pub const PUBLIC_KEY: &str = "0x0";
        pub const SECRET_KEY: &str = "0x0";
    }

    pub mod icp {
        pub const ACCOUNT_ID: &str = "rdmx6-jaaaa-aaaaa-aaadq-cai";
        pub const PUBLIC_KEY: &str = "dummy";
        pub const SECRET_KEY: &str = "dummy";
    }

    pub mod ethereum {
        pub const ACCOUNT_ID: &str = "0x0000000000000000000000000000000000000000";
        pub const SECRET_KEY: &str =
            "0000000000000000000000000000000000000000000000000000000000000001";
    }
}
