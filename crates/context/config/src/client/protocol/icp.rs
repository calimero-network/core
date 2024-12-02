use std::borrow::Cow;
use std::collections::BTreeMap;

use ed25519_consensus::SigningKey;
use ic_agent::agent::CallResponse;
use ic_agent::export::Principal;
use ic_agent::Agent;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use super::Protocol;
use crate::client::transport::{
    AssociatedTransport, Operation, ProtocolTransport, TransportRequest,
};

use crate::client::env::config::types::icp::{
    ICApplication, ICApplicationId, ICBlobId, ICContextId, ICContextIdentity, ICSignerId,
    ICPContextRequestKind, ICPRequest, ICPRequestKind, ICPContextRequest, ICPSigned
};
use rand::rngs::OsRng;

use ed25519_dalek::{Signer, SigningKey as DSigningKey};

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
    pub public_key: Vec<u8>,
    pub secret_key: SigningKey,
}

mod serde_creds {
    use candid::Principal;
    use ed25519_consensus::SigningKey;
    use serde::{Deserialize, Serialize};
    use thiserror::Error;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        account_id: Principal,
        public_key: Vec<u8>,
        secret_key: SigningKey,
    }

    #[derive(Copy, Clone, Debug, Error)]
    pub enum CredentialsError {}

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
    pub secret_key: SigningKey,
}

#[derive(Debug)]
pub struct IcpConfig<'a> {
    pub networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

#[derive(Clone, Debug)]
struct Network {
    client: Agent,
    account_id: Principal,
    secret_key: SigningKey,
}

#[derive(Clone, Debug)]
pub struct IcpTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, Network>,
}

impl<'a> IcpTransport<'a> {
    #[must_use]
    pub fn new(config: &IcpConfig<'a>) -> Self {
        let mut networks = BTreeMap::new();

        for (network_id, network_config) in &config.networks {
            let client = Agent::builder()
                .with_url(network_config.rpc_url.clone())
                .build()
                .unwrap();

            let _ignored = networks.insert(
                network_id.clone(),
                Network {
                    client,
                    account_id: network_config.account_id.clone(),
                    secret_key: network_config.secret_key.clone(),
                },
            );
        }

        Self { networks }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IcpError {
    #[error("unsupported protocol `{0}`")]
    UnsupportedProtocol(String),
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
        println!("Here access to query");
        self.client
            .fetch_root_key()
            .await
            .map_err(|_| IcpError::Custom {
                operation: ErrorOperation::Query,
                reason: "Failed to fetch root key".to_string(),
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
                reason: "Error while quering".to_string(),
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
        let mut rng = OsRng;
        let current_time: u64 = 1733150708000u64;
        let context_sk = self.secret_key.clone();
        let context_pk = context_sk.verification_key();

        let sign_key = DSigningKey::from_bytes(&context_sk.to_bytes());

        let context_id = ICContextId::new(context_pk.to_bytes());

        let request = ICPRequest {
            kind: ICPRequestKind::Context(ICPContextRequest {
                context_id: context_id.clone(),
                kind: ICPContextRequestKind::Add {
                    author_id: ICContextIdentity::new([0u8; 32]),
                    application: ICApplication {
                        id: ICApplicationId::new([0u8; 32]),
                        blob: ICBlobId::new([0u8; 32]),
                        size: 0,
                        source: String::new(),
                        metadata: vec![],
                    },
                },
            }),
            signer_id: ICSignerId::new(context_id.as_bytes()),
            timestamp_ms: current_time,
        };

        let sign_req =  ICPSigned::new(request, |bytes| sign_key.sign(bytes))
        .expect("Failed to create signed request");


        let args_encoded = candid::encode_one(sign_req).unwrap();
        self.client
            .fetch_root_key()
            .await
            .map_err(|_| IcpError::Custom {
                operation: ErrorOperation::Query,
                reason: "Failed to fetch root key".to_string(),
            })?;

        let response = self
            .client
            .update(canister_id, method)
            .with_arg(args_encoded)
            .call()
            .await;
        match response {
            Ok(CallResponse::Response((data, _certificate))) => Ok(data),
            Ok(CallResponse::Poll(_)) => Err(IcpError::Custom {
                operation: ErrorOperation::Query,
                reason: "Unexpected Poll response".to_string(),
            }),
            Err(err) => Err(IcpError::Custom {
                operation: ErrorOperation::Query,
                reason: err.to_string(),
            }),
        }
    }
}
