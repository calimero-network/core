use std::fmt::Debug;

use alloy::primitives::{keccak256, B256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::{Signature, SignerSync};
use alloy_sol_types::SolValue;
use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{BytesN, Env};
use starknet::core::codec::Encode as StarknetEncode;
use starknet::signers::SigningKey as StarknetSigningKey;
use starknet_crypto::{poseidon_hash_many, Felt};

pub mod methods;

use super::types::ethereum::{SolRequest, SolRequestKind, SolSignedRequest};
use super::types::starknet::{Request as StarknetRequest, Signed as StarknetSigned};
use crate::client::env::config::types::ethereum::ToSol;
use crate::client::env::{utils, Method};
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::icp::types::{ICRequest, ICSigned};
use crate::repr::{Repr, ReprTransmute};
use crate::stellar::stellar_types::{
    FromWithEnv, StellarRequest, StellarRequestKind, StellarSignedRequest,
    StellarSignedRequestPayload,
};
use crate::types::Signed;
use crate::{ContextIdentity, Request, RequestKind};

#[derive(Debug)]
pub struct ContextConfigMutate<'a, T> {
    pub client: CallClient<'a, T>,
}

#[derive(Debug)]
pub struct ContextConfigMutateRequest<'a, T> {
    client: CallClient<'a, T>,
    kind: RequestKind<'a>,
}

#[derive(Debug)]
struct Mutate<'a> {
    pub(crate) signing_key: [u8; 32],
    pub(crate) nonce: u64,
    pub(crate) kind: RequestKind<'a>,
}

impl<'a> Method<Near> for Mutate<'a> {
    const METHOD: &'static str = "mutate";

    type Returns = ();

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let request = Request::new(signer_sk.verifying_key().rt()?, self.kind, self.nonce);

        let signed = Signed::new(&request, |b| signer_sk.sign(b))?;

        let encoded = serde_json::to_vec(&signed)?;

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if !response.is_empty() {
            eyre::bail!("unexpected response {:?}", response);
        }

        Ok(())
    }
}

impl<'a> Method<Starknet> for Mutate<'a> {
    type Returns = ();
    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Derive ecdsa key from private key
        let secret_scalar = Felt::from_bytes_be(&self.signing_key);
        let signing_key = StarknetSigningKey::from_secret_scalar(secret_scalar);
        let verifying_key = signing_key.verifying_key().scalar();
        let verifying_key_bytes = verifying_key.to_bytes_be();

        // Derive ed25519 key from private key
        let user_key = SigningKey::from_bytes(&self.signing_key).verifying_key();
        let user_key_bytes = user_key.to_bytes();

        // Create Repr wrapped ContextIdentity instances
        let signer_id = verifying_key_bytes
            .rt::<ContextIdentity>()
            .map_err(|e| eyre::eyre!("Failed to convert verifying key: {}", e))?;
        let signer_id = Repr::new(signer_id);

        let user_id = user_key_bytes
            .rt::<ContextIdentity>()
            .map_err(|e| eyre::eyre!("Failed to convert user key: {}", e))?;
        let user_id = Repr::new(user_id);

        // Create the Request structure using into() conversions
        let request = StarknetRequest {
            signer_id: signer_id.into(),
            user_id: user_id.into(),
            nonce: self.nonce,
            kind: self.kind.into(),
        };

        // Serialize the request
        let mut serialized_request = vec![];
        request.encode(&mut serialized_request)?;

        // Hash the serialized request
        let hash = poseidon_hash_many(&serialized_request);

        // Sign the hash with the signing key
        let signature = signing_key.sign(&hash)?;

        let signed_request = StarknetSigned {
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

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        Ok(())
    }
}

impl<'a> Method<Icp> for Mutate<'a> {
    type Returns = ();

    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let request = ICRequest::new(
            signer_sk.verifying_key().rt()?,
            self.kind.into(),
            self.nonce,
        );

        let signed = ICSigned::new(request, |b| signer_sk.sign(b))?;

        let encoded = candid::encode_one(&signed)?;

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        match candid::decode_one::<Result<(), String>>(&response) {
            Ok(decoded) => match decoded {
                Ok(()) => Ok(()),
                Err(err_msg) => eyre::bail!("unexpected response {:?}", err_msg),
            },
            Err(e) => {
                eyre::bail!("unexpected response {:?}", e)
            }
        }
    }
}

impl<'a> Method<Stellar> for Mutate<'a> {
    type Returns = ();
    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let signer_id: [u8; 32] = signer_sk.verifying_key().rt()?;
        let signer_id = BytesN::from_array(&env, &signer_id);

        let request = StellarRequest::new(
            signer_id,
            StellarRequestKind::from_with_env(self.kind, &env),
            self.nonce,
        );

        let signed_request_payload = StellarSignedRequestPayload::Context(request);

        let signed_request =
            StellarSignedRequest::new(&env, signed_request_payload, |b| Ok(signer_sk.sign(b)))
                .map_err(|e| eyre::eyre!("Failed to sign request: {:?}", e))?;

        let bytes: Vec<u8> = signed_request.to_xdr(&env).into_iter().collect();

        Ok(bytes)
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        Ok(())
    }
}

impl<'a> Method<Ethereum> for Mutate<'a> {
    type Returns = ();
    // The method needs to be encoded as a tuple with arguments that it expects
    const METHOD: &'static str =
        "mutate(((bytes32,bytes32,uint64,uint8,bytes),bytes32,bytes32,uint8))";

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

        let context_request = match &self.kind {
            RequestKind::Context(req) => req.to_sol(),
        };

        let encoded_request = context_request.abi_encode();

        let sol_request = SolRequest {
            signerId: ecdsa_public_key,
            userId: user_id,
            nonce: self.nonce,
            kind: SolRequestKind::Context,
            data: encoded_request.into(),
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

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        Ok(())
    }
}

impl<'a, T: Transport> ContextConfigMutateRequest<'a, T> {
    pub async fn send(self, signing_key: [u8; 32], nonce: u64) -> Result<(), ClientError<T>> {
        let request = Mutate {
            signing_key,
            nonce,
            kind: self.kind,
        };

        utils::send(&self.client, Operation::Write(request)).await
    }
}
