#![cfg(feature = "icp_client")]

//! ICP-specific implementations for context config mutations.

use ed25519_dalek::{Signer, SigningKey};

use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::icp::types::{ICRequest, ICSigned};
use crate::repr::ReprTransmute;

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
