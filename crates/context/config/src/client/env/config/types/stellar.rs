use serde::{Deserialize, Serialize};
use soroban_client::xdr::BytesM;

use crate::RequestKind;

#[derive(Serialize, Deserialize, Debug)]
#[serde(bound(deserialize = "'de: 'a"))]
pub struct StellarRequest<'a> {
    pub kind: RequestKind<'a>,
    pub signer_id: BytesM<32>,
    pub nonce: u64,
}

impl<'a> StellarRequest<'a> {
    pub fn new(signer_id: &str, kind: RequestKind<'a>, nonce: u64) -> Self {
        Self {
            signer_id: BytesM::<32>::try_from(signer_id.as_bytes())
                .expect("signer_id must be 32 bytes"),
            kind,
            nonce,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(bound(deserialize = "'de: 'a"))]
pub struct StellarSignedRequest<'a> {
    pub request: StellarRequest<'a>,
    pub signature: BytesM<64>,
}

impl<'a> StellarSignedRequest<'a> {
    pub fn new(request: StellarRequest<'a>, signature: Vec<u8>) -> Self {
        Self {
            request,
            signature: BytesM::<64>::try_from(signature.as_slice())
                .expect("signature must be 64 bytes"),
        }
    }
}
