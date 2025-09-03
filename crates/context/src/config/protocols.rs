//! Protocol-specific implementations - flattened from deeply nested structure

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use url::Url;

/// Protocol trait for context operations
pub trait Protocol {
    const PROTOCOL: &'static str;
    type Error: std::error::Error + Send + Sync + 'static;
}

/// Protocol-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProtocolConfig {
    Near(NearConfig),
    Starknet(StarknetConfig),
    Icp(IcpConfig),
    Stellar(StellarConfig),
    Ethereum(EthereumConfig),
}

/// Near Protocol implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearConfig {
    pub networks: BTreeMap<String, NearNetworkConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearNetworkConfig {
    pub rpc_url: Url,
    pub account_id: String,
    pub access_key: String,
}

impl Protocol for NearConfig {
    const PROTOCOL: &'static str = "near";
    type Error = NearError;
}

#[derive(Debug, thiserror::Error)]
pub enum NearError {
    #[error("Near protocol error: {0}")]
    Protocol(String),
}

/// Starknet Protocol implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StarknetConfig {
    pub networks: BTreeMap<String, StarknetNetworkConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StarknetNetworkConfig {
    pub rpc_url: Url,
    pub account_id: String,
    pub access_key: String,
}

impl Protocol for StarknetConfig {
    const PROTOCOL: &'static str = "starknet";
    type Error = StarknetError;
}

#[derive(Debug, thiserror::Error)]
pub enum StarknetError {
    #[error("Starknet protocol error: {0}")]
    Protocol(String),
}

/// ICP Protocol implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcpConfig {
    pub networks: BTreeMap<String, IcpNetworkConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcpNetworkConfig {
    pub rpc_url: Url,
    pub account_id: String,
    pub secret_key: String,
}

impl Protocol for IcpConfig {
    const PROTOCOL: &'static str = "icp";
    type Error = IcpError;
}

#[derive(Debug, thiserror::Error)]
pub enum IcpError {
    #[error("ICP protocol error: {0}")]
    Protocol(String),
}

/// Stellar Protocol implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StellarConfig {
    pub networks: BTreeMap<String, StellarNetworkConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StellarNetworkConfig {
    pub network: String,
    pub rpc_url: Url,
    pub public_key: String,
    pub secret_key: String,
}

impl Protocol for StellarConfig {
    const PROTOCOL: &'static str = "stellar";
    type Error = StellarError;
}

#[derive(Debug, thiserror::Error)]
pub enum StellarError {
    #[error("Stellar protocol error: {0}")]
    Protocol(String),
}

/// Ethereum Protocol implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EthereumConfig {
    pub networks: BTreeMap<String, EthereumNetworkConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EthereumNetworkConfig {
    pub rpc_url: Url,
    pub account_id: String,
    pub access_key: String, // Private key
}

impl Protocol for EthereumConfig {
    const PROTOCOL: &'static str = "ethereum";
    type Error = EthereumError;
}

#[derive(Debug, thiserror::Error)]
pub enum EthereumError {
    #[error("Ethereum protocol error: {0}")]
    Protocol(String),
}

/// Unified protocol error type
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("Near error: {0}")]
    Near(#[from] NearError),
    #[error("Starknet error: {0}")]
    Starknet(#[from] StarknetError),
    #[error("ICP error: {0}")]
    Icp(#[from] IcpError),
    #[error("Stellar error: {0}")]
    Stellar(#[from] StellarError),
    #[error("Ethereum error: {0}")]
    Ethereum(#[from] EthereumError),
    #[error("Unsupported protocol: {0}")]
    UnsupportedProtocol(String),
}
