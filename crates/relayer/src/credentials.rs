//! Credential management for the relayer

use calimero_context_config::client::config::Credentials;
use calimero_context_config::client::protocol::{
    ethereum::Credentials as ClientEthereumCredentials, icp::Credentials as ClientIcpCredentials,
    near::Credentials as ClientNearCredentials, starknet::Credentials as ClientStarknetCredentials,
};

use crate::constants::protocols;

/// Protocol-specific signing credentials
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProtocolCredentials {
    Near {
        account_id: String,
        public_key: String,
        secret_key: String,
    },
    Starknet {
        account_id: String,
        public_key: String,
        secret_key: String,
    },
    Icp {
        account_id: String,
        public_key: String,
        secret_key: String,
    },
    Ethereum {
        account_id: String,
        secret_key: String,
    },
}

impl From<ProtocolCredentials> for Credentials {
    fn from(creds: ProtocolCredentials) -> Self {
        match creds {
            ProtocolCredentials::Near {
                account_id,
                public_key,
                secret_key,
            } => Credentials::Near(ClientNearCredentials {
                account_id: account_id.parse().expect("Invalid NEAR account ID"),
                public_key: public_key.parse().expect("Invalid NEAR public key"),
                secret_key: secret_key.parse().expect("Invalid NEAR secret key"),
            }),
            ProtocolCredentials::Starknet {
                account_id,
                public_key,
                secret_key,
            } => Credentials::Starknet(ClientStarknetCredentials {
                account_id: account_id.parse().expect("Invalid Starknet account ID"),
                public_key: public_key.parse().expect("Invalid Starknet public key"),
                secret_key: secret_key.parse().expect("Invalid Starknet secret key"),
            }),
            ProtocolCredentials::Icp {
                account_id,
                public_key,
                secret_key,
            } => Credentials::Icp(ClientIcpCredentials {
                account_id: account_id.parse().expect("Invalid ICP account ID"),
                public_key: public_key.clone(),
                secret_key: secret_key.clone(),
            }),
            ProtocolCredentials::Ethereum {
                account_id,
                secret_key,
            } => Credentials::Ethereum(ClientEthereumCredentials {
                account_id: account_id.clone(),
                secret_key: secret_key.clone(),
            }),
        }
    }
}

/// Create credentials from environment variables
pub fn from_env(protocol: &str) -> Option<ProtocolCredentials> {
    let prefix = protocol.to_uppercase();

    // Get environment variables
    let account_id = std::env::var(format!("{}_ACCOUNT_ID", prefix)).ok();
    let public_key = std::env::var(format!("{}_PUBLIC_KEY", prefix)).ok();
    let secret_key = std::env::var(format!("{}_SECRET_KEY", prefix)).ok();

    // Helper function to check if all required credentials are present and non-empty
    let has_required_creds =
        |account_id: &Option<String>, public_key: &Option<String>, secret_key: &Option<String>| {
            account_id.as_ref().map_or(false, |s| !s.is_empty())
                && public_key.as_ref().map_or(false, |s| !s.is_empty())
                && secret_key.as_ref().map_or(false, |s| !s.is_empty())
        };

    let has_eth_creds = |account_id: &Option<String>, secret_key: &Option<String>| {
        account_id.as_ref().map_or(false, |s| !s.is_empty())
            && secret_key.as_ref().map_or(false, |s| !s.is_empty())
    };

    // Create credentials based on protocol
    match protocol {
        protocols::near::NAME if has_required_creds(&account_id, &public_key, &secret_key) => {
            Some(ProtocolCredentials::Near {
                account_id: account_id.unwrap(),
                public_key: public_key.unwrap(),
                secret_key: secret_key.unwrap(),
            })
        }
        protocols::starknet::NAME if has_required_creds(&account_id, &public_key, &secret_key) => {
            Some(ProtocolCredentials::Starknet {
                account_id: account_id.unwrap(),
                public_key: public_key.unwrap(),
                secret_key: secret_key.unwrap(),
            })
        }
        protocols::icp::NAME if has_required_creds(&account_id, &public_key, &secret_key) => {
            Some(ProtocolCredentials::Icp {
                account_id: account_id.unwrap(),
                public_key: public_key.unwrap(),
                secret_key: secret_key.unwrap(),
            })
        }
        protocols::ethereum::NAME if has_eth_creds(&account_id, &secret_key) => {
            Some(ProtocolCredentials::Ethereum {
                account_id: account_id.unwrap(),
                secret_key: secret_key.unwrap(),
            })
        }
        _ => None,
    }
}
