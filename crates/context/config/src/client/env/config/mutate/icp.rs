#![cfg(feature = "icp_client")]

//! ICP-specific implementations for context config mutations.
//!
//! This module provides ICP (Internet Computer Protocol) blockchain-specific implementations
//! of the `Method<Icp>` trait for context config mutation operations. It handles ICP's
//! Candid-based encoding and decoding.
//!
//! ## Key Features
//!
//! - **Candid Encoding**: Uses Candid for parameter encoding and response decoding
//! - **ED25519 Signing**: Implements ED25519 signature generation and verification
//! - **Type Safety**: Leverages Candid for type-safe interactions
//! - **Error Handling**: Converts ICP-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! The mutation request is encoded using Candid:
//! - ED25519 keys are used directly for ICP compatibility
//! - Request data is encoded using Candid serialization
//! - Signatures are generated using ED25519
//! - Responses are decoded using Candid deserialization

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
