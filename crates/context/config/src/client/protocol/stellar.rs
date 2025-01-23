use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use serde::{Deserialize, Serialize};
use soroban_client::contract::{ContractBehavior, Contracts};
use soroban_client::error::Error;
use soroban_client::keypair::{Keypair, KeypairBehavior};
use soroban_client::server::{Options, Server};
use soroban_client::soroban_rpc::{
    GetTransactionResponse, RawSimulateHostFunctionResult, RawSimulateTransactionResponse,
    SendTransactionStatus,
};
use soroban_client::transaction::{TransactionBehavior, TransactionBuilder};
use soroban_client::transaction_builder::TransactionBuilderBehavior;
use soroban_client::xdr::{ScBytes, ScVal};
use stellar_baselib::xdr::{self, ReadXdr};
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
#[serde(try_from = "serde_creds::Credentials")]
pub struct Credentials {
    pub public_key: String,
    pub secret_key: String,
}

mod serde_creds {
    use hex::FromHexError;
    use serde::{Deserialize, Serialize};
    use thiserror::Error;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        public_key: String,
        secret_key: String,
    }

    #[derive(Clone, Debug, Error)]
    pub enum CredentialsError {
        #[error("failed to parse SigningKey from hex")]
        ParseError(#[from] FromHexError),
        #[error("failed to parse SigningKey from string")]
        IntoError(String),
    }

    impl TryFrom<Credentials> for super::Credentials {
        type Error = CredentialsError;

        fn try_from(creds: Credentials) -> Result<Self, Self::Error> {
            Ok(Self {
                public_key: creds.public_key,
                secret_key: creds.secret_key,
            })
        }
    }
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

            const OPTIONS: Options = Options {
                allow_http: None,
                timeout: Some(1000),
                headers: None,
            };
            let server = Server::new(network_config.rpc_url.as_str(), OPTIONS).unwrap();

            let _ignored = networks.insert(
                network_id.clone(),
                Network {
                    client: Arc::new(server),
                    keypair,
                    network: network_config.network.clone(),
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
    #[error("failed to prepare transactions `{0}`")]
    FailedToPrepareTransactions(String),
    #[error("invalid response from RPC while {operation}")]
    InvalidResponse { operation: ErrorOperation },
    #[error(
        "access key does not have permission to call method `{method}` on contract {contract}"
    )]
    NotPermittedToCallMethod { contract: String, method: String },
    #[error("transaction timed out")]
    TransactionTimeout,
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

        let args = if args.is_empty() {
            None
        } else {
            let sc_bytes = ScBytes::try_from(args).map_err(|e| StellarError::Custom {
                operation: ErrorOperation::Query,
                reason: e.to_string(),
            })?;
            let scval_bytes = ScVal::Bytes(sc_bytes);
            Some(vec![scval_bytes])
        };

        let transaction = TransactionBuilder::new(source_account, self.network.as_str(), None)
            .fee(10000u32)
            .add_operation(contract.call(method, args))
            .set_timeout(15)
            .expect("Transaction timeout")
            .build();

        let result: Result<RawSimulateTransactionResponse, Error> = self
            .client
            .simulate_transaction(transaction.clone(), None)
            .await;
        let xdr_results: Vec<RawSimulateHostFunctionResult> = result.unwrap().results.unwrap();

        match xdr_results.first().and_then(|xdr| xdr.xdr.as_ref()) {
            Some(xdr_bytes) => {
                let xdr_bytes = base64::engine::general_purpose::STANDARD
                    .decode(xdr_bytes)
                    .map_err(|_| StellarError::Custom {
                        operation: ErrorOperation::Query,
                        reason: "Failed to decode XDR response".to_owned(),
                    })?;

                let cursor = Cursor::new(xdr_bytes);
                let mut limited = xdr::Limited::new(cursor, xdr::Limits::none());
                match ScVal::read_xdr(&mut limited) {
                    Ok(ScVal::Bytes(bytes)) => Ok(bytes.into()),
                    Ok(_) => Err(StellarError::Custom {
                        operation: ErrorOperation::Query,
                        reason: "Unexpected XDR response type; expected ScVal::Bytes".to_owned(),
                    }),
                    Err(_) => Err(StellarError::Custom {
                        operation: ErrorOperation::Query,
                        reason: "Failed to parse XDR type; expected ScVal::Bytes".to_owned(),
                    }),
                }
            }
            None => Err(StellarError::Custom {
                operation: ErrorOperation::Query,
                reason: "No XDR results found or XDR field is missing".to_owned(),
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

        let args = if args.is_empty() {
            None
        } else {
            let sc_bytes = ScBytes::try_from(args).map_err(|e| StellarError::Custom {
                operation: ErrorOperation::Mutate,
                reason: e.to_string(),
            })?;
            let scval_bytes = ScVal::Bytes(sc_bytes);
            Some(vec![scval_bytes])
        };

        let transaction = TransactionBuilder::new(source_account, self.network.as_str(), None)
            .fee(10000u32)
            .add_operation(contract.call(method, args))
            .set_timeout(15)
            .expect("Transaction timeout")
            .build();

        let signed_tx = {
            let prepared_tx = self
                .client
                .prepare_transaction(transaction, self.network.as_str())
                .await;
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
                    let hash = response.base.hash;
                    let status = response.base.status;
                    let start = Instant::now();

                    if matches!(
                        status,
                        SendTransactionStatus::Pending | SendTransactionStatus::Success
                    ) {
                        loop {
                            match self.client.get_transaction(hash.as_str()).await {
                                Ok(GetTransactionResponse::Successful(info)) => {
                                    break Some(info.returnValue)
                                }
                                Ok(GetTransactionResponse::Failed(f)) => {
                                    return Err(StellarError::Custom {
                                        operation: ErrorOperation::Mutate,
                                        reason: format!("Transaction failed: {:?}", f),
                                    })
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
            Some(ScVal::Bytes(bytes)) => Ok(bytes.into()),
            Some(ScVal::Void) => Ok(vec![]),
            Some(other) => Err(StellarError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Unexpected return type: {:?}", other),
            }),
            None => Err(StellarError::Custom {
                operation: ErrorOperation::Mutate,
                reason: "No value returned".to_owned(),
            }),
        }
    }
}
