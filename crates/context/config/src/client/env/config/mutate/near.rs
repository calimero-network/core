#![cfg(feature = "near_client")]

//! NEAR-specific implementations for context config mutations.

use ed25519_dalek::{Signer, SigningKey};

use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::repr::ReprTransmute;
use crate::types::Signed;
use crate::Request;

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
