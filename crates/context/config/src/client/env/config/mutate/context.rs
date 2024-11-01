use ed25519_dalek::ed25519::signature::SignerMut;
use ed25519_dalek::SigningKey;

use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::Method;
use crate::types::Signed;
use crate::{Request, RequestKind};

#[derive(Debug)]
pub struct Mutate<'a> {
    pub(crate) signer_id: [u8; 32],
    pub(crate) nonce: u64,
    pub(crate) kind: RequestKind<'a>,
}

impl<'a> Method<Mutate<'a>> for Near {
    const METHOD: &'static str = "mutate";

    type Returns = ();

    fn encode(params: &Mutate) -> eyre::Result<Vec<u8>> {
        let signed = Signed::new(&Request::new(todo!(), params.kind), |b| {
            SigningKey::from_bytes(&params.signer_id).sign(b)
        })?;

        let encoded = serde_json::to_vec(&signed)?;

        Ok(encoded)
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        Ok(())
    }
}

impl<'a> Method<Mutate<'a>> for Starknet {
    type Returns = ();

    const METHOD: &'static str = "mutate";

    fn encode(params: &Mutate) -> eyre::Result<Vec<u8>> {
        // sign the params, encode it and return
        // since you will have a `Vec<Felt>` here, you can
        // `Vec::with_capacity(32 * calldata.len())` and then
        // extend the `Vec` with each `Felt::to_bytes_le()`
        // when this `Vec<u8>` makes it to `StarknetTransport`,
        // reconstruct the `Vec<Felt>` from it
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
