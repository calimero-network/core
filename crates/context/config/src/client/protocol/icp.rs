use std::borrow::Cow;
use std::collections::BTreeMap;

use ed25519_consensus::SigningKey;
use ic_agent::export::Principal;
use ic_agent::identity::BasicIdentity;
use ic_agent::Agent;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use super::Protocol;
use crate::client::transport::{
    AssociatedTransport, Operation, ProtocolTransport, TransportRequest,
};

#[derive(Copy, Clone, Debug)]
pub enum Icp {}

impl Protocol for Icp {
    const PROTOCOL: &'static str = "icp";
}

impl AssociatedTransport for IcpTransport<'_> {
    type Protocol = Icp;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "serde_creds::Credentials")]
pub struct Credentials {
    pub account_id: Principal,
    pub public_key: String,
    pub secret_key: String,
}

mod serde_creds {
    use candid::Principal;
    use hex::FromHexError;
    use serde::{Deserialize, Serialize};
    use thiserror::Error;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        account_id: Principal,
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
    pub account_id: Principal,
    pub secret_key: String,
}

#[derive(Debug)]
pub struct IcpConfig<'a> {
    pub networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

#[derive(Clone, Debug)]
struct Network {
    client: Agent,
    _account_id: Principal,
    _secret_key: String,
}

#[derive(Clone, Debug)]
pub struct IcpTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, Network>,
}

impl<'a> IcpTransport<'a> {
    #[must_use]
    pub fn new(config: &IcpConfig<'a>) -> Self {
        let mut networks: BTreeMap<Cow<'a, str>, Network> = BTreeMap::new();

        for (network_id, network_config) in &config.networks {
            let secret_key_byes = hex::decode(network_config.secret_key.clone()).unwrap();
            let secret_key_array: [u8; 32] = secret_key_byes.try_into().unwrap();
            let secret_key: SigningKey = secret_key_array.into();

            let identity = BasicIdentity::from_signing_key(secret_key.clone());

            let client = Agent::builder()
                .with_url(network_config.rpc_url.clone())
                .with_identity(identity)
                .build()
                .unwrap();

            let _ignored = networks.insert(
                network_id.clone(),
                Network {
                    client,
                    _account_id: network_config.account_id.clone(),
                    _secret_key: network_config.secret_key.clone(),
                },
            );
        }

        Self { networks }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IcpError {
    #[error("unknown network `{0}`")]
    UnknownNetwork(String),
    #[error("invalid canister id `{0}`")]
    InvalidCanisterId(String),
    #[error("invalid response from RPC while {operation}")]
    InvalidResponse { operation: ErrorOperation },
    #[error(
        "access key does not have permission to call method `{method}` on canister {canister}"
    )]
    NotPermittedToCallMethod { canister: String, method: String },
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
    #[error("querying canister")]
    Query,
    #[error("updating canister")]
    Mutate,
}

impl ProtocolTransport for IcpTransport<'_> {
    type Error = IcpError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let Some(network) = self.networks.get(&request.network_id) else {
            return Err(IcpError::UnknownNetwork(request.network_id.into_owned()));
        };

        let canister_id = Principal::from_text(&request.contract_id)
            .map_err(|_| IcpError::InvalidCanisterId(request.contract_id.into_owned()))?;

        match request.operation {
            Operation::Read { method } => {
                network
                    .query(&canister_id, method.into_owned(), payload)
                    .await
            }
            Operation::Write { method } => {
                network
                    .mutate(&canister_id, method.into_owned(), payload)
                    .await
            }
        }
    }
}

impl Network {
    async fn query(
        &self,
        canister_id: &Principal,
        method: String,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, IcpError> {
        self.client
            .fetch_root_key()
            .await
            .map_err(|_| IcpError::Custom {
                operation: ErrorOperation::Query,
                reason: "Failed to fetch root key".to_owned(),
            })?;

        let response = self
            .client
            .query(canister_id, method)
            .with_arg(args)
            .call()
            .await;

        response.map_or(
            Err(IcpError::Custom {
                operation: ErrorOperation::Query,
                reason: "Error while quering".to_owned(),
            }),
            |response| Ok(response),
        )
    }

    async fn mutate(
        &self,
        canister_id: &Principal,
        method: String,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, IcpError> {
        self.client
            .fetch_root_key()
            .await
            .map_err(|_| IcpError::Custom {
                operation: ErrorOperation::Mutate,
                reason: "Failed to fetch root key".to_owned(),
            })?;

        let response = self
            .client
            .update(canister_id, method)
            .with_arg(args)
            .call_and_wait()
            .await;

        match response {
            Ok(data) => Ok(data),
            Err(err) => Err(IcpError::Custom {
                operation: ErrorOperation::Mutate,
                reason: err.to_string(),
            }),
        }
    }
}
