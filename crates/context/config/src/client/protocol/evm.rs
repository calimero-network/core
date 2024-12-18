use std::borrow::Cow;
use std::collections::BTreeMap;
use std::vec;

use serde::Serialize;
use thiserror::Error;
use url::Url;

use super::Protocol;
use crate::client::transport::{
    AssociatedTransport, Operation, ProtocolTransport, TransportRequest,
};

#[derive(Copy, Clone, Debug)]
pub enum Evm {}

impl Protocol for Evm {
    const PROTOCOL: &'static str = "evm";
}

impl AssociatedTransport for EvmTransport<'_> {
    type Protocol = Evm;
}
#[derive(Clone, Debug, Serialize)]
#[serde(try_from = "serde_creds::Credentials")]
pub struct Credentials {
    pub account_id: String,
    pub public_key: String,
    pub secret_key: String,
}

mod serde_creds {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        account_id: String,
        public_key: String,
        secret_key: String,
    }

    impl TryFrom<Credentials> for super::Credentials {
        type Error = &'static str;

        fn try_from(creds: Credentials) -> Result<Self, Self::Error> {
            Ok(Self {
                account_id: creds.account_id,
                public_key: creds.public_key,
                secret_key: creds.secret_key,
            })
        }
    }
}

#[derive(Debug)]
pub struct NetworkConfig {
    pub rpc_url: Url,
    pub account_id: String,
    pub access_key: String,
}

#[derive(Debug)]
pub struct EvmConfig<'a> {
    pub networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

#[derive(Clone, Debug)]
struct Network {
    client: String,
    account_id: String,
    secret_key: String,
}

#[derive(Clone, Debug)]
pub struct EvmTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, Network>,
}

impl<'a> EvmTransport<'a> {
    #[must_use]
    pub fn new(config: &EvmConfig<'a>) -> Self {
        let mut networks = BTreeMap::new();

        for (network_id, network_config) in &config.networks {
            let client = "client".to_string();

            let _ignored = networks.insert(
                network_id.clone(),
                Network {
                    client,
                    account_id: network_config.account_id.clone(),
                    secret_key: network_config.access_key.clone(),
                },
            );
        }

        Self { networks }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EvmError {
    #[error("unknown network `{0}`")]
    UnknownNetwork(String),
    #[error("invalid response from RPC while {operation}")]
    InvalidResponse { operation: ErrorOperation },
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
    #[error("mutating contract")]
    Mutate,
}

impl ProtocolTransport for EvmTransport<'_> {
    type Error = EvmError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let Some(network) = self.networks.get(&request.network_id) else {
            return Err(EvmError::UnknownNetwork(request.network_id.into_owned()));
        };

        let contract_id = request.contract_id.into_owned();

        match request.operation {
            Operation::Read { method } => {
                network
                    .query(contract_id, method.into_owned(), payload)
                    .await
            }
            Operation::Write { method } => {
                network
                    .mutate(contract_id, method.into_owned(), payload)
                    .await
            }
        }
    }
}

impl Network {
    async fn query(
        &self,
        _contract_id: String,
        _method: String,
        _args: Vec<u8>,
    ) -> Result<Vec<u8>, EvmError> {
        Ok(vec![])
    }

    async fn mutate(
        &self,
        _contract_id: String,
        _method: String,
        _args: Vec<u8>,
    ) -> Result<Vec<u8>, EvmError> {
        Ok(vec![])
    }
}
