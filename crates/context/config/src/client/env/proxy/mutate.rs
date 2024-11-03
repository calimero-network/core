use std::marker::PhantomData;

use ed25519_dalek::{Signer, SigningKey};

use crate::client::env::{utils, Method};
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::types::Signed;
use crate::{ProposalWithApprovals, ProxyMutateRequest};

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
struct Mutate<'a> {
    pub(crate) signing_key: [u8; 32],
    pub(crate) raw_request: ProxyMutateRequest,
    _marker: PhantomData<&'a ()>,
}

impl<'a> Method<Near> for Mutate<'a> {
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

impl<'a> Method<Starknet> for Mutate<'a> {
    type Returns = Option<ProposalWithApprovals>;

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

impl<'a, T: Transport> ContextProxyMutateRequest<'a, T> {
    pub async fn send(
        self,
        signing_key: [u8; 32],
    ) -> Result<Option<ProposalWithApprovals>, ClientError<T>> {
        let request = Mutate {
            signing_key,
            _marker: PhantomData,
            raw_request: self.raw_request,
        };

        utils::send_near_or_starknet(&self.client, Operation::Write(request)).await
    }
}
