use calimero_primitives::context::ContextId;
use clap::Parser;
use color_eyre::owo_colors::OwoColorize;
use const_format::concatcp;
use eyre::Result as EyreResult;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

pub const EXAMPLES: &str = r"
  # Fetch the node's DID
  $ meroctl -- --node-name node1 did
";

#[derive(Debug, Parser)]
#[command(about = "Fetch the node's Decentralized Identifier (DID)")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct DidCommand;

// Define the expected response structure from the Admin API
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum NearNetworkId {
    Mainnet,
    Testnet,
    #[serde(untagged)]
    Custom(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum WalletType {
    NEAR {
        #[serde(rename = "networkId")]
        network_id: NearNetworkId,
    },
    ETH {
        #[serde(rename = "chainId")]
        chain_id: u64,
    },
    STARKNET {
        #[serde(rename = "walletName")]
        wallet_name: String,
    },
    ICP {
        #[serde(rename = "canisterId")]
        canister_id: String,
        #[serde(rename = "walletName")]
        wallet_name: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RootKey {
    pub signing_key: String,
    #[serde(rename = "wallet")]
    pub wallet_type: WalletType,
    pub wallet_address: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ClientKey {
    #[serde(rename = "wallet")]
    pub wallet_type: WalletType,
    pub signing_key: String,
    pub created_at: u64,
    pub context_id: Option<ContextId>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DidData {
    pub id: String,
    pub root_keys: Vec<RootKey>,
    pub client_keys: Vec<ClientKey>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetDidResponse {
    pub data: DidData,
}

impl Report for GetDidResponse {
    fn report(&self) {
        // Print the main DID ID
        println!("{}: {}", "DID".bold(), self.data.id.cyan());

        // Print Root Keys
        if !self.data.root_keys.is_empty() {
            println!("\n{}", "Root Keys:".bold());
            for key in &self.data.root_keys {
                println!(
                    "  - {}: {}, {}: {:?}, {}: {}, {}: {}",
                    "Signing Key".italic(),
                    key.signing_key.yellow(),
                    "Wallet Type".italic(),
                    key.wallet_type,
                    "Wallet Addr".italic(),
                    key.wallet_address,
                    "Created At".italic(),
                    key.created_at
                );
            }
        }

        // Print Client Keys
        if !self.data.client_keys.is_empty() {
            println!("\n{}", "Client Keys:".bold());
            for key in &self.data.client_keys {
                println!(
                    "  - {}: {}, {}: {:?}, {}: {:?}, {}: {}",
                    "Signing Key".italic(),
                    key.signing_key.yellow(),
                    "Wallet Type".italic(),
                    key.wallet_type,
                    "Context ID".italic(),
                    key.context_id,
                    "Created At".italic(),
                    key.created_at
                );
            }
        }
    }
}

impl DidCommand {
    pub async fn run(&self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let response: GetDidResponse = do_request(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/did")?,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
