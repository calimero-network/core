use ed25519_dalek::{Signer, SigningKey};

use crate::client::env::{utils, Method};
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::repr::ReprTransmute;
use crate::types::Signed;
use crate::{Request, RequestKind};

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
    pub(crate) kind: RequestKind<'a>,
}

impl<'a> Method<Near> for Mutate<'a> {
    const METHOD: &'static str = "mutate";

    type Returns = ();

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let request = Request::new(signer_sk.verifying_key().rt()?, self.kind);

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
        // sign the params, encode it and return
        // since you will have a `Vec<Felt>` here, you can
        // `Vec::with_capacity(32 * calldata.len())` and then
        // extend the `Vec` with each `Felt::to_bytes_le()`
        // when this `Vec<u8>` makes it to `StarknetTransport`,
        // reconstruct the `Vec<Felt>` from it
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}

impl<'a, T: Transport> ContextConfigMutateRequest<'a, T> {
    pub async fn send(self, signing_key: [u8; 32]) -> Result<(), ClientError<T>> {
        let request = Mutate {
            signing_key,
            // todo! when nonces are implemented in context
            // todo! config contract, we fetch it here first
            // nonce: _,
            kind: self.kind,
        };

        utils::send_near_or_starknet(&self.client, Operation::Write(request)).await
    }
}
