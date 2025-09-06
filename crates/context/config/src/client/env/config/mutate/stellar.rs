#![cfg(feature = "stellar_client")]

//! Stellar-specific implementations for context config mutations.

use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{BytesN, Env};

use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::stellar::Stellar;
use crate::repr::ReprTransmute;
use crate::stellar::stellar_types::{
    FromWithEnv, StellarRequest, StellarRequestKind, StellarSignedRequest,
    StellarSignedRequestPayload,
};

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
