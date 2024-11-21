use ed25519_dalek::{Signer, SigningKey};
use starknet_crypto::{poseidon_hash_many, Felt};
use starknet::signers::SigningKey as StarknetSigningKey;
use starknet::core::codec::Encode;
use crate::client::env::proxy::starknet::StarknetProposalWithApprovals;
use crate::Repr;
use crate::repr::ReprBytes;

use crate::client::env::{utils, Method};
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::types::{ProposalId, Signed, SignerId};
use crate::{ProposalWithApprovals, ProxyMutateRequest};
use starknet::core::codec::Decode;

use super::types::starknet::{StarknetProxyMutateRequest, StarknetSignedRequest};

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
        let signer_id = Repr::new(SignerId::from_bytes(|bytes| {
            bytes.copy_from_slice(&verifying_key_bytes);
            Ok(32)
        })?);

        // Create request with signer_id
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
        println!("response: {:?}", response);
        if response.is_empty() {
            return Ok(None);
        }

        // Skip first 32 bytes (array length)
        let response = &response[32..];

        // Get proposal_id from the next 32 bytes (using only low part)
        let proposal_id = Repr::new(ProposalId::from_bytes(|bytes| {
            bytes.copy_from_slice(&response[..32]);
            Ok(32)
        })?);

        // Get num_approvals from the last 32 bytes
        let num_approvals = u32::from_be_bytes(response[32..][28..32].try_into()?)
            as usize;

        Ok(Some(ProposalWithApprovals {
            proposal_id,
            num_approvals,
        }))
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

        utils::send_near_or_starknet(&self.client, Operation::Write(request)).await
    }
}
