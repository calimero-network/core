use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use serde::{Deserialize, Serialize};
use soroban_client::contract::{ContractBehavior, Contracts};
use soroban_client::error::Error;
use soroban_client::keypair::{Keypair, KeypairBehavior};
use soroban_client::network::{NetworkPassphrase, Networks};
use soroban_client::soroban_rpc::{
    SendTransactionStatus, SimulateTransactionResponse, TransactionStatus,
};
use soroban_client::transaction::{TransactionBehavior, TransactionBuilder};
use soroban_client::transaction_builder::TransactionBuilderBehavior;
use soroban_client::xdr::{Limits, ReadXdr, ScVal, WriteXdr};
use soroban_client::{Options, Server};

use thiserror::Error;
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
    network: String,
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
                timeout: 1000,
                headers: std::collections::HashMap::new(),
                friendbot_url: None,
            };
            let server = Server::new(network_config.rpc_url.as_str(), options)
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
                    keypair,
                    network: network.to_owned(),
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
    #[error("quering contract")]
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
    ) -> Result<Vec<u8>, Self::Error> {
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
        let account = self
            .client
            .get_account(self.keypair.public_key().as_str())
            .await
            .map_err(|e| StellarError::Custom {
                operation: ErrorOperation::Query,
                reason: e.to_string(),
            })?;

        let source_account = Rc::new(RefCell::new(account));
        let mut encoded_args = None;

        // First convert the XDR bytes back to a Vec<Val>
        if !args.is_empty() {
            // Convert the raw bytes to base64 and then to ScVal
            let base64_args = base64::engine::general_purpose::STANDARD.encode(&args);
            let sc_val: ScVal =
                ScVal::from_xdr_base64(&base64_args, Limits::none()).map_err(|_| {
                    StellarError::Custom {
                        operation: ErrorOperation::Query,
                        reason: "Failed to convert to ScVal".to_owned(),
                    }
                })?;
            encoded_args = Some(vec![sc_val]);
        }

        let transaction = TransactionBuilder::new(source_account, self.network.as_str(), None)
            .fee(10000u32)
            .add_operation(contract.call(method, encoded_args))
            .set_timeout(15)
            .expect("Transaction timeout")
            .build();

        let result: Result<SimulateTransactionResponse, Error> =
            self.client.simulate_transaction(&transaction, None).await;

        let response = result.map_err(|e| StellarError::Custom {
            operation: ErrorOperation::Query,
            reason: format!("Simulation failed: {}", e),
        })?;

        match response.to_result() {
            Some((sc_val, _auth)) => {
                // Convert the result to XDR bytes
                let xdr_bytes =
                    sc_val
                        .to_xdr_base64(Limits::none())
                        .map_err(|_| StellarError::Custom {
                            operation: ErrorOperation::Query,
                            reason: "Failed to encode XDR response".to_owned(),
                        })?;
                base64::engine::general_purpose::STANDARD
                    .decode(xdr_bytes)
                    .map_err(|_| StellarError::Custom {
                        operation: ErrorOperation::Query,
                        reason: "Failed to decode XDR response".to_owned(),
                    })
            }
            None => Err(StellarError::Custom {
                operation: ErrorOperation::Query,
                reason: "No XDR results found".to_owned(),
            }),
        }
    }

    async fn mutate(
        &self,
        contract: &Contracts,
        method: &str,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, StellarError> {
        let account = self
            .client
            .get_account(self.keypair.public_key().as_str())
            .await
            .map_err(|e| StellarError::Custom {
                operation: ErrorOperation::Mutate,
                reason: e.to_string(),
            })?;

        let source_account = Rc::new(RefCell::new(account));

        let mut encoded_args = None;

        if !args.is_empty() {
            // Convert the raw bytes to base64 and then to ScVal
            let base64_args = base64::engine::general_purpose::STANDARD.encode(&args);
            let sc_val: ScVal =
                ScVal::from_xdr_base64(&base64_args, Limits::none()).map_err(|_| {
                    StellarError::Custom {
                        operation: ErrorOperation::Query,
                        reason: "Failed to convert to ScVal".to_owned(),
                    }
                })?;
            encoded_args = Some(vec![sc_val]);
        }

        let transaction = TransactionBuilder::new(source_account, self.network.as_str(), None)
            .fee(10000u32)
            .add_operation(contract.call(method, encoded_args))
            .set_timeout(15)
            .expect("Transaction timeout")
            .build();

        let simulation_result = self.client.simulate_transaction(&transaction, None).await;

        if let Err(err) = simulation_result {
            return Err(StellarError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Simulation failed: {}", err),
            });
        }

        let signed_tx = {
            let prepared_tx = self.client.prepare_transaction(&transaction).await;
            if let Ok(mut tx) = prepared_tx {
                tx.sign(&[self.keypair.clone()]);
                Some(tx.clone())
            } else {
                return Err(StellarError::Custom {
                    operation: ErrorOperation::Mutate,
                    reason: format!("Failed to create transaction: {:?}", prepared_tx),
                });
            }
        };

        let result = match signed_tx {
            Some(tx) => match self.client.send_transaction(tx).await {
                Ok(response) => {
                    let hash = response.hash;
                    let status = response.status;
                    let start = Instant::now();

                    if matches!(status, SendTransactionStatus::Pending) {
                        loop {
                            match self.client.get_transaction(hash.as_str()).await {
                                Ok(response) => {
                                    match response.status {
                                        TransactionStatus::Success => {
                                            if let Some((_meta, return_value)) =
                                                response.to_result_meta()
                                            {
                                                break Some(return_value);
                                            } else {
                                                break Some(None);
                                            }
                                        }
                                        TransactionStatus::Failed => {
                                            return Err(StellarError::Custom {
                                                operation: ErrorOperation::Mutate,
                                                reason: "Transaction failed".to_owned(),
                                            });
                                        }
                                        TransactionStatus::NotFound => {
                                            // Continue waiting
                                            continue;
                                        }
                                    }
                                }
                                _ if Instant::now().duration_since(start).as_secs() > 35 => {
                                    break None
                                }
                                _ => continue,
                            }
                        }
                    } else {
                        Some(None)
                    }
                }
                Err(err) => {
                    return Err(StellarError::Custom {
                        operation: ErrorOperation::Mutate,
                        reason: format!("Transaction failed: {:?}", err),
                    })
                }
            },
            None => {
                return Err(StellarError::Custom {
                    operation: ErrorOperation::Mutate,
                    reason: "Transaction failed".to_owned(),
                })
            }
        };
        match result.flatten() {
            Some(sc_val) => match sc_val {
                ScVal::Void => Ok(vec![]),
                val @ (ScVal::Bool(_)
                | ScVal::Error(_)
                | ScVal::U32(_)
                | ScVal::I32(_)
                | ScVal::U64(_)
                | ScVal::I64(_)
                | ScVal::Timepoint(_)
                | ScVal::Duration(_)
                | ScVal::U128(_)
                | ScVal::I128(_)
                | ScVal::U256(_)
                | ScVal::I256(_)
                | ScVal::Bytes(_)
                | ScVal::String(_)
                | ScVal::Symbol(_)
                | ScVal::Vec(_)
                | ScVal::Map(_)
                | ScVal::Address(_)
                | ScVal::LedgerKeyContractInstance
                | ScVal::LedgerKeyNonce(_)
                | ScVal::ContractInstance(_)) => {
                    let xdr_bytes =
                        val.to_xdr_base64(Limits::none())
                            .map_err(|_| StellarError::Custom {
                                operation: ErrorOperation::Mutate,
                                reason: "Failed to encode XDR".to_owned(),
                            })?;
                    base64::engine::general_purpose::STANDARD
                        .decode(xdr_bytes)
                        .map_err(|_| StellarError::Custom {
                            operation: ErrorOperation::Mutate,
                            reason: "Failed to decode XDR".to_owned(),
                        })
                }
            },
            None => Err(StellarError::Custom {
                operation: ErrorOperation::Mutate,
                reason: "No value returned".to_owned(),
            }),
        }
    }
}
