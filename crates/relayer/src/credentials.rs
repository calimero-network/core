//! Credential management for the relayer

use calimero_context_config::client::config::Credentials;
use calimero_context_config::client::protocol::{
    ethereum::Credentials as EthereumCredentials, icp::Credentials as IcpCredentials,
    near::Credentials as NearCredentials, starknet::Credentials as StarknetCredentials,
};
use eyre::Result as EyreResult;

use crate::config::ProtocolCredentials;
use crate::constants::{dummy, protocols};

/// Trait for creating protocol-specific credentials
pub trait CredentialBuilder {
    /// Create credentials from environment variables
    fn from_env(protocol: &str) -> Option<ProtocolCredentials>;

    /// Create default credentials (for testing only)
    fn default_credentials(protocol: &str) -> Option<ProtocolCredentials>;

    /// Create dummy/fallback credentials
    fn dummy_credentials(protocol: &str) -> EyreResult<Credentials>;
}

pub struct RelayerCredentials;

impl CredentialBuilder for RelayerCredentials {
    fn from_env(protocol: &str) -> Option<ProtocolCredentials> {
        let prefix = protocol.to_uppercase();

        // Get environment variables
        let account_id = std::env::var(format!("{}_ACCOUNT_ID", prefix)).ok();
        let public_key = std::env::var(format!("{}_PUBLIC_KEY", prefix)).ok();
        let secret_key = std::env::var(format!("{}_SECRET_KEY", prefix)).ok();

        // Only create credentials if all required variables are set and non-empty
        match protocol {
            protocols::near::NAME => {
                if let (Some(account_id), Some(public_key), Some(secret_key)) =
                    (&account_id, &public_key, &secret_key)
                {
                    if !account_id.is_empty() && !public_key.is_empty() && !secret_key.is_empty() {
                        return Some(ProtocolCredentials::Near {
                            account_id: account_id.clone(),
                            public_key: public_key.clone(),
                            secret_key: secret_key.clone(),
                        });
                    }
                }
            }
            protocols::starknet::NAME => {
                if let (Some(account_id), Some(public_key), Some(secret_key)) =
                    (&account_id, &public_key, &secret_key)
                {
                    if !account_id.is_empty() && !public_key.is_empty() && !secret_key.is_empty() {
                        return Some(ProtocolCredentials::Starknet {
                            account_id: account_id.clone(),
                            public_key: public_key.clone(),
                            secret_key: secret_key.clone(),
                        });
                    }
                }
            }
            protocols::icp::NAME => {
                if let (Some(account_id), Some(public_key), Some(secret_key)) =
                    (&account_id, &public_key, &secret_key)
                {
                    if !account_id.is_empty() && !public_key.is_empty() && !secret_key.is_empty() {
                        return Some(ProtocolCredentials::Icp {
                            account_id: account_id.clone(),
                            public_key: public_key.clone(),
                            secret_key: secret_key.clone(),
                        });
                    }
                }
            }
            protocols::ethereum::NAME => {
                if let (Some(account_id), Some(secret_key)) = (&account_id, &secret_key) {
                    if !account_id.is_empty() && !secret_key.is_empty() {
                        return Some(ProtocolCredentials::Ethereum {
                            account_id: account_id.clone(),
                            secret_key: secret_key.clone(),
                        });
                    }
                }
            }
            _ => {}
        }

        None
    }

    fn default_credentials(protocol: &str) -> Option<ProtocolCredentials> {
        match protocol {
            protocols::near::NAME => {
                // Only provide default credentials if secret key is available via env
                if let Ok(secret_key) = std::env::var("NEAR_DEFAULT_SECRET_KEY") {
                    if !secret_key.is_empty() {
                        return Some(ProtocolCredentials::Near {
                            account_id: protocols::near::DEFAULT_ACCOUNT_ID.to_owned(),
                            public_key: protocols::near::DEFAULT_PUBLIC_KEY.to_owned(),
                            secret_key,
                        });
                    }
                }
            }
            protocols::starknet::NAME => {
                if let Ok(secret_key) = std::env::var("STARKNET_DEFAULT_SECRET_KEY") {
                    if !secret_key.is_empty() {
                        return Some(ProtocolCredentials::Starknet {
                            account_id: protocols::starknet::DEFAULT_ACCOUNT_ID.to_owned(),
                            public_key: protocols::starknet::DEFAULT_PUBLIC_KEY.to_owned(),
                            secret_key,
                        });
                    }
                }
            }
            protocols::icp::NAME => {
                if let Ok(secret_key) = std::env::var("ICP_DEFAULT_SECRET_KEY") {
                    if !secret_key.is_empty() {
                        return Some(ProtocolCredentials::Icp {
                            account_id: protocols::icp::DEFAULT_ACCOUNT_ID.to_owned(),
                            public_key: protocols::icp::DEFAULT_PUBLIC_KEY.to_owned(),
                            secret_key,
                        });
                    }
                }
            }
            protocols::ethereum::NAME => {
                if let Ok(secret_key) = std::env::var("ETHEREUM_DEFAULT_SECRET_KEY") {
                    if !secret_key.is_empty() {
                        return Some(ProtocolCredentials::Ethereum {
                            account_id: protocols::ethereum::DEFAULT_ACCOUNT_ID.to_owned(),
                            secret_key,
                        });
                    }
                }
            }
            _ => {}
        }

        None
    }

    fn dummy_credentials(protocol: &str) -> EyreResult<Credentials> {
        match protocol {
            protocols::near::NAME => Ok(Credentials::Near(NearCredentials {
                account_id: dummy::near::ACCOUNT_ID.parse()?,
                public_key: dummy::near::PUBLIC_KEY.parse()?,
                secret_key: dummy::near::SECRET_KEY.parse()?,
            })),
            protocols::starknet::NAME => Ok(Credentials::Starknet(StarknetCredentials {
                account_id: dummy::starknet::ACCOUNT_ID.parse()?,
                public_key: dummy::starknet::PUBLIC_KEY.parse()?,
                secret_key: dummy::starknet::SECRET_KEY.parse()?,
            })),
            protocols::icp::NAME => Ok(Credentials::Icp(IcpCredentials {
                account_id: dummy::icp::ACCOUNT_ID.parse()?,
                public_key: dummy::icp::PUBLIC_KEY.to_owned(),
                secret_key: dummy::icp::SECRET_KEY.to_owned(),
            })),
            protocols::ethereum::NAME => Ok(Credentials::Ethereum(EthereumCredentials {
                account_id: dummy::ethereum::ACCOUNT_ID.to_owned(),
                secret_key: dummy::ethereum::SECRET_KEY.to_owned(),
            })),
            _ => eyre::bail!("Unknown protocol: {}", protocol),
        }
    }
}

/// Convert relayer credentials to client credentials
pub fn convert_to_client_credentials(creds: &ProtocolCredentials) -> EyreResult<Credentials> {
    match creds {
        ProtocolCredentials::Near {
            account_id,
            public_key,
            secret_key,
        } => Ok(Credentials::Near(NearCredentials {
            account_id: account_id.parse()?,
            public_key: public_key.parse()?,
            secret_key: secret_key.parse()?,
        })),
        ProtocolCredentials::Starknet {
            account_id,
            public_key,
            secret_key,
        } => Ok(Credentials::Starknet(StarknetCredentials {
            account_id: account_id.parse()?,
            public_key: public_key.parse()?,
            secret_key: secret_key.parse()?,
        })),
        ProtocolCredentials::Icp {
            account_id,
            public_key,
            secret_key,
        } => Ok(Credentials::Icp(IcpCredentials {
            account_id: account_id.parse()?,
            public_key: public_key.clone(),
            secret_key: secret_key.clone(),
        })),
        ProtocolCredentials::Ethereum {
            account_id,
            secret_key,
        } => Ok(Credentials::Ethereum(EthereumCredentials {
            account_id: account_id.clone(),
            secret_key: secret_key.clone(),
        })),
    }
}
