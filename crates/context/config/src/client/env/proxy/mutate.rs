use alloy::primitives::{keccak256, B256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::{Signature, SignerSync};
use alloy_sol_types::SolValue;
use candid::Decode;
use ed25519_dalek::{Signer, SigningKey};
use eyre::WrapErr;
use soroban_sdk::xdr::{FromXdr, ToXdr};
use soroban_sdk::{Bytes, Env};
use starknet::core::codec::Encode;
use starknet::signers::SigningKey as StarknetSigningKey;
use starknet_crypto::{poseidon_hash_many, Felt};

use super::ethereum::{SolProposal, SolProposalApprovalWithSigner};
use super::types::ethereum::{SolRequest, SolRequestKind, SolSignedRequest};
use super::types::starknet::{StarknetProxyMutateRequest, StarknetSignedRequest};
use crate::client::env::proxy::ethereum::SolProposalWithApprovals;
use crate::client::env::{utils, Method};
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::icp::types::ICSigned;
use crate::icp::{ICProposalWithApprovals, ICProxyMutateRequest};
use crate::repr::ReprTransmute;
use crate::stellar::stellar_types::{
    FromWithEnv, StellarSignedRequest, StellarSignedRequestPayload,
};
use crate::stellar::{StellarProposalWithApprovals, StellarProxyMutateRequest};
use crate::types::Signed;
use crate::{ProposalWithApprovals, ProxyMutateRequest, Repr};

pub mod methods;

#[derive(Debug)]
pub struct ContextProxyMutate<'a, T> {
    pub client: CallClient<'a, T>,
}

#[derive(Debug)]
pub struct ContextProxyMutateRequest<'a, T> {
    client: CallClient<'a, T>,
    raw_request: ProxyMutateRequest,
}

#[derive(Debug)]
struct Mutate {
    pub(crate) signing_key: [u8; 32],
    pub(crate) raw_request: ProxyMutateRequest,
}

impl Method<Near> for Mutate {
    const METHOD: &'static str = "mutate";

    type Returns = Option<ProposalWithApprovals>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let signed = Signed::new(&self.raw_request, |b| signer_sk.sign(b))?;

        let encoded = serde_json::to_vec(&signed)?;

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for Mutate {
    type Returns = Option<ProposalWithApprovals>;
    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Derive ECDSA key for signing
        let secret_scalar = Felt::from_bytes_be(&self.signing_key);
        let signing_key = StarknetSigningKey::from_secret_scalar(secret_scalar);
        let verifying_key = signing_key.verifying_key().scalar();
        let verifying_key_bytes = verifying_key.to_bytes_be();

        // Create signer_id from ECDSA verifying key for signature verification
        let signer_id = verifying_key_bytes.rt().wrap_err("Infallible conversion")?;

        // Create request with signer_id (no Repr)
        let request = StarknetProxyMutateRequest::from((signer_id, self.raw_request));

        // Serialize -> Hash -> Sign with ECDSA
        let mut serialized_request = vec![];
        request.encode(&mut serialized_request)?;
        let hash = poseidon_hash_many(&serialized_request);
        let signature = signing_key.sign(&hash)?;

        let signed_request = StarknetSignedRequest {
            payload: serialized_request,
            signature_r: signature.r,
            signature_s: signature.s,
        };

        let mut signed_request_serialized = vec![];
        signed_request.encode(&mut signed_request_serialized)?;

        let bytes: Vec<u8> = signed_request_serialized
            .iter()
            .flat_map(|felt| felt.to_bytes_be())
            .collect();

        Ok(bytes)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(None);
        }

        // Skip first 32 bytes (array length)
        let response = &response[32..];

        // Get proposal_id from the next 64 bytes (32 for high, 32 for low)
        let mut proposal_bytes = [0u8; 32];
        proposal_bytes[..16].copy_from_slice(&response[16..32]); // Last 16 bytes of high
        proposal_bytes[16..].copy_from_slice(&response[48..64]); // Last 16 bytes of low
        let proposal_id = Repr::new(proposal_bytes.rt()?);

        // Get num_approvals from the last 32 bytes
        let num_approvals = u32::from_be_bytes(response[64..][28..32].try_into()?) as usize;

        Ok(Some(ProposalWithApprovals {
            proposal_id,
            num_approvals,
        }))
    }
}

impl Method<Icp> for Mutate {
    type Returns = Option<ProposalWithApprovals>;

    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let payload: ICProxyMutateRequest =
            self.raw_request.try_into().map_err(eyre::Report::msg)?;

        let signed = ICSigned::new(payload, |b| signer_sk.sign(b))?;

        let encoded = candid::encode_one(&signed)?;

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, Option<ICProposalWithApprovals>)?;
        Ok(decoded.map(Into::into))
    }
}

impl Method<Stellar> for Mutate {
    type Returns = Option<ProposalWithApprovals>;

    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let payload: StellarProxyMutateRequest =
            StellarProxyMutateRequest::from_with_env(self.raw_request, &env);

        let signed_request_payload = StellarSignedRequestPayload::Proxy(payload);

        let signed_request =
            StellarSignedRequest::new(&env, signed_request_payload, |b| Ok(signer_sk.sign(b)))
                .map_err(|e| eyre::eyre!("Failed to sign request: {:?}", e))?;

        let bytes: Vec<u8> = signed_request.to_xdr(&env).into_iter().collect();

        Ok(bytes)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(None);
        }
        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let stellar_proposal = StellarProposalWithApprovals::from_xdr(&env, &env_bytes)
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        let proposal: ProposalWithApprovals = stellar_proposal.into();

        Ok(Some(proposal))
    }
}

impl Method<Ethereum> for Mutate {
    type Returns = Option<ProposalWithApprovals>;

    const METHOD: &'static str = "mutate(((bytes32,bytes32,uint8,bytes),bytes32,bytes32,uint8))";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let ed25519_key = SigningKey::from_bytes(&self.signing_key);
        let user_id_bytes = ed25519_key.verifying_key().to_bytes();
        let user_id = B256::from_slice(&user_id_bytes);

        let ecdsa_private_key_input =
            ["ECDSA_DERIVE".as_bytes(), &self.signing_key.as_slice()].concat();
        let ecdsa_private_key_bytes = keccak256(&ecdsa_private_key_input);
        let signer = PrivateKeySigner::from_bytes(&ecdsa_private_key_bytes)?;
        let address = signer.address();
        let ecdsa_public_key = address.into_word();

        let kind = SolRequestKind::from(&self.raw_request);

        let request_data = match self.raw_request {
            ProxyMutateRequest::Propose { proposal } => {
                SolProposal::try_from(proposal)?.abi_encode()
            }
            ProxyMutateRequest::Approve { approval } => {
                SolProposalApprovalWithSigner::from(approval).abi_encode()
            }
        };

        let sol_request = SolRequest {
            signerId: ecdsa_public_key,
            userId: user_id,
            kind,
            data: request_data.into(),
        };

        let request_message = sol_request.abi_encode();

        let message_hash = keccak256(&request_message);
        let signature: Signature = signer.sign_message_sync(&message_hash.as_slice())?;

        let r = B256::from(signature.r());
        let s = B256::from(signature.s());
        let v = if signature.recid().to_byte() == 0 {
            27
        } else {
            28
        };

        let signed_request = SolSignedRequest {
            payload: sol_request,
            r,
            s,
            v,
        };

        let encoded = signed_request.abi_encode();
        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded: SolProposalWithApprovals = SolValue::abi_decode(&response, false)?;

        let proposal = ProposalWithApprovals {
            proposal_id: decoded.proposalId.rt().wrap_err("infallible conversion")?,
            num_approvals: decoded.numApprovals as usize,
        };

        Ok(Some(proposal))
    }
}

impl<'a, T: Transport> ContextProxyMutateRequest<'a, T> {
    pub async fn send(
        self,
        signing_key: [u8; 32],
    ) -> Result<Option<ProposalWithApprovals>, ClientError<T>> {
        let request = Mutate {
            signing_key,
            raw_request: self.raw_request,
        };

        utils::send(&self.client, Operation::Write(request)).await
    }
}
