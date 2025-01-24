use std::fmt::Debug;

use ed25519_dalek::{Signer, SigningKey};
use hex;
use soroban_client::keypair::{Keypair, KeypairBehavior};
use starknet::core::codec::Encode as StarknetEncode;
use starknet::signers::SigningKey as StarknetSigningKey;
use starknet_crypto::{poseidon_hash_many, Felt};

use super::types::starknet::{Request as StarknetRequest, Signed as StarknetSigned};
use super::types::stellar::StellarSignedRequest;
use crate::client::env::config::types::stellar::StellarRequest;
use crate::client::env::{utils, Method};
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::icp::types::{ICRequest, ICSigned};
use crate::repr::{Repr, ReprTransmute};
use crate::types::Signed;
use crate::{ContextIdentity, Request, RequestKind};
pub mod methods;

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
        let secret_key = hex::encode(self.signing_key);
        let keypair = Keypair::from_secret(&secret_key).unwrap();

        let request = StellarRequest::new(keypair.public_key().as_str(), self.kind, self.nonce);

        let serialized_request = bincode::serialize(&request)?;
        let signature = keypair.sign(&serialized_request).unwrap();

        let signed_request = StellarSignedRequest::new(request, signature);

        let encoded = bincode::serialize(&signed_request)?;

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
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
