use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use soroban_client::{
    contract::{ContractBehavior, Contracts},
    keypair::{Keypair, KeypairBehavior},
    network::{NetworkPassphrase, Networks},
    soroban_rpc::TransactionStatus,
    transaction::{TransactionBuilder, TransactionBuilderBehavior, TransactionBehavior, ReadXdr},
    xdr::{ScVal, Limits, WriteXdr},
    Options, Server,
};
use thiserror::Error;
use tokio::time;
use url::Url;

use super::Protocol;
use crate::client::transport::{
    AssociatedTransport, Operation, ProtocolTransport, TransportRequest,
};

#[derive(Copy, Clone, Debug)]
pub enum Stellar {}

impl Protocol for Stellar {
    const PROTOCOL: &'static str = "stellar";
}

impl AssociatedTransport for StellarTransport<'_> {
    type Protocol = Stellar;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Credentials {
    pub public_key: String,
    pub secret_key: String,
}

#[derive(Debug)]
pub struct NetworkConfig {
    pub rpc_url: Url,
    pub network: String,
    pub public_key: String,
    pub secret_key: String,
}

#[derive(Debug)]
pub struct StellarConfig<'a> {
    pub networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

#[derive(Clone, Debug)]
struct Network {
    client: Arc<Server>,
    network: &'static str,
    keypair: Keypair,
}

#[derive(Clone, Debug)]
pub struct StellarTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, Network>,
}

impl<'a> StellarTransport<'a> {
    #[must_use]
    pub fn new(config: &StellarConfig<'a>) -> Self {
        let mut networks: BTreeMap<Cow<'a, str>, Network> = BTreeMap::new();

        for (network_id, network_config) in &config.networks {
            let keypair: Keypair = Keypair::from_secret(&network_config.secret_key).unwrap();

            let options = Options {
                allow_http: true,
                timeout: 10,
                headers: Default::default(),
                friendbot_url: None,
            };
            
            let server = Server::new(&network_config.rpc_url.to_string(), options)
                .expect("Failed to create server");

            let network = match network_config.network.as_str() {
                "mainnet" => Networks::public(),
                "testnet" => Networks::testnet(),
                "local" => Networks::standalone(),
                _ => Networks::standalone(),
            };

            let _ignored = networks.insert(
                network_id.clone(),
                Network {
                    client: Arc::new(server),
                    network,
                    keypair,
                },
            );
        }

        Self { networks }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StellarError {
    #[error("unknown network `{0}`")]
    UnknownNetwork(String),
    #[error("invalid contract id `{0}`")]
    InvalidContractId(String),
    #[error("error while {operation}: {reason}")]
    Custom {
        operation: ErrorOperation,
        reason: String,
    },
}

#[derive(Copy, Clone, Debug, Error)]
#[non_exhaustive]
pub enum ErrorOperation {
    #[error("querying contract")]
    Query,
    #[error("updating contract")]
    Mutate,
    #[error("transport")]
    Transport,
}

impl ProtocolTransport for StellarTransport<'_> {
    type Error = StellarError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, StellarError> {
        let Some(network) = self.networks.get(&request.network_id) else {
            return Err(StellarError::UnknownNetwork(
                request.network_id.into_owned(),
            ));
        };

        let contract: Contracts = Contracts::new(&request.contract_id)
            .map_err(|_| StellarError::InvalidContractId(request.contract_id.into_owned()))?;

        match request.operation {
            Operation::Read { method } => network.query(&contract, &method, payload).await,
            Operation::Write { method } => network.mutate(&contract, &method, payload).await,
        }
    }
}

impl Network {
    async fn query(
        &self,
        contract: &Contracts,
        method: &str,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, StellarError> {
        // Parse arguments from payload if provided
        let args = if !args.is_empty() {
            self.parse_args_from_payload(&args)?
        } else {
            vec![]
        };

        // Get account for transaction building
        let account = self
            .client
            .get_account(self.keypair.public_key().as_str())
            .await
            .map_err(|e| StellarError::Custom {
                operation: ErrorOperation::Query,
                reason: e.to_string(),
            })?;

        // Build and simulate transaction
        let source_account = Rc::new(RefCell::new(account));
        let transaction = TransactionBuilder::new(source_account, self.network, None)
            .fee(10000u32)
            .add_operation(contract.call(method, Some(args)))
            .set_timeout(15)
            .expect("Transaction timeout")
            .build();

        let simulation_result = self
            .client
            .simulate_transaction(&transaction, None)
            .await
            .map_err(|e| StellarError::Custom {
                operation: ErrorOperation::Query,
                reason: format!("Simulation failed: {}", e),
            })?;

        // Extract result from simulation
        let result = simulation_result
            .to_result()
            .map(|(sc_val, _)| sc_val)
            .ok_or_else(|| StellarError::Custom {
                operation: ErrorOperation::Query,
                reason: "No result from simulation".to_string(),
            })?;

        // Convert ScVal to bytes
        Ok(result.to_xdr(Limits::none()).map_err(|_| StellarError::Custom {
            operation: ErrorOperation::Query,
            reason: "Failed to convert result to XDR".to_string(),
        })?)
    }

    async fn mutate(
        &self,
        contract: &Contracts,
        method: &str,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, StellarError> {
        // Parse arguments from payload if provided
        let args = if !args.is_empty() {
            self.parse_args_from_payload(&args)?
        } else {
            vec![]
        };

        // Get account for transaction building
        let account = self
            .client
            .get_account(self.keypair.public_key().as_str())
            .await
            .map_err(|e| StellarError::Custom {
                operation: ErrorOperation::Mutate,
                reason: e.to_string(),
            })?;

        // Build transaction
        let source_account = Rc::new(RefCell::new(account.clone()));
        let transaction = TransactionBuilder::new(source_account, self.network, None)
            .fee(10000u32)
            .add_operation(contract.call(method, Some(args.clone())))
            .set_timeout(15)
            .expect("Transaction timeout")
            .build();

        // Simulate first to catch errors early
        let simulation_result = self
            .client
            .simulate_transaction(&transaction, None)
            .await
            .map_err(|e| StellarError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Simulation failed: {}", e),
            })?;

        // Check if simulation was successful
        if simulation_result.error.is_some() {
            return Err(StellarError::Custom {
                operation: ErrorOperation::Mutate,
                reason: "Simulation failed".to_string(),
            });
        }

        // Build the final transaction
        let final_transaction = TransactionBuilder::new(
            Rc::new(RefCell::new(account.clone())),
            self.network,
            None,
        )
        .fee(10000u32)
        .add_operation(contract.call(method, Some(args)))
        .set_timeout(15)
        .expect("Transaction timeout")
        .build();

        // Sign the transaction
        let mut signed_tx = final_transaction;
        signed_tx.sign(&[self.keypair.clone()]);

        // Send transaction
        let response = self
            .client
            .send_transaction(signed_tx)
            .await
            .map_err(|e| StellarError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Failed to send transaction: {}", e),
            })?;

        // Wait for transaction to be confirmed
        let hash = response.hash;
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 35;

        while attempts < MAX_ATTEMPTS {
            match self.client.get_transaction(hash.as_str()).await {
                Ok(response) => {
                    match response.status {
                        TransactionStatus::Success => {
                            // Get the return value from the transaction result
                            if let Some((_, return_value)) = response.to_result_meta() {
                                if let Some(sc_val) = return_value {
                                    return Ok(sc_val.to_xdr(Limits::none()).map_err(|_| StellarError::Custom {
                                        operation: ErrorOperation::Mutate,
                                        reason: "Failed to convert return value to XDR".to_string(),
                                    })?);
                                } else {
                                    return Ok(vec![]); // No return value
                                }
                            } else {
                                return Ok(vec![]); // No result meta
                            }
                        }
                        TransactionStatus::Failed => {
                            return Err(StellarError::Custom {
                                operation: ErrorOperation::Mutate,
                                reason: "Transaction failed".to_string(),
                            });
                        }
                        TransactionStatus::NotFound => {
                            // Transaction still pending, wait and retry
                            attempts += 1;
                            time::sleep(time::Duration::from_secs(1)).await;
                            continue;
                        }
                    }
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= MAX_ATTEMPTS {
                        return Err(StellarError::Custom {
                            operation: ErrorOperation::Mutate,
                            reason: format!("Failed to get transaction status after {} attempts: {}", attempts, e),
                        });
                    }
                    time::sleep(time::Duration::from_secs(1)).await;
                }
            }
        }

        Err(StellarError::Custom {
            operation: ErrorOperation::Mutate,
            reason: "Transaction confirmation timeout".to_string(),
        })
    }

    /// Parse arguments from the payload bytes
    fn parse_args_from_payload(&self, payload: &[u8]) -> Result<Vec<ScVal>, StellarError> {
        // Try to parse as a single ScVal first (for backward compatibility)
        if let Ok(sc_val) = ScVal::from_xdr(payload, Limits::none()) {
            return Ok(vec![sc_val]);
        }

        // Try to parse as a vector of ScVals
        if let Ok(mut sc_vals) = soroban_client::xdr::VecM::<ScVal>::from_xdr(payload, Limits::none()) {
            return Ok(sc_vals.into_iter().map(|v| v.clone()).collect());
        }

        // If all parsing fails, return empty args
        Ok(vec![])
    }
}
