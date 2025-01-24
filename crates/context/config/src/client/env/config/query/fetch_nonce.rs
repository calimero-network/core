use candid::{Decode, Encode};
use serde::Serialize;
use starknet::core::codec::Encode as StarknetEncode;

use crate::client::env::config::types::starknet::{
    CallData, ContextId as StarknetContextId, ContextIdentity as StarknetContextIdentity,
};
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;
use crate::repr::Repr;
use crate::types::{ContextId, ContextIdentity};

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct FetchNonceRequest {
    pub(super) context_id: Repr<ContextId>,
    pub(super) member_id: Repr<ContextIdentity>,
}

impl FetchNonceRequest {
    pub const fn new(context_id: ContextId, member_id: ContextIdentity) -> Self {
        Self {
            context_id: Repr::new(context_id),
            member_id: Repr::new(member_id),
        }
    }
}

impl Method<Near> for FetchNonceRequest {
    const METHOD: &'static str = "fetch_nonce";

    type Returns = Option<u64>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Starknet> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();

        // Dereference Repr and encode context_id
        let context_id: StarknetContextId = (*self.context_id).into();
        context_id.encode(&mut call_data)?;

        let member_id: StarknetContextIdentity = (*self.member_id).into();
        member_id.encode(&mut call_data)?;

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 8 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 8 bytes, got {}",
                response.len()
            ));
        }

        let nonce = u64::from_be_bytes(
            response
                .try_into()
                .map_err(|_| eyre::eyre!("Failed to convert response to u64"))?,
        );

        Ok(Some(nonce))
    }
}

impl Method<Icp> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        let member_id = ICRepr::new(*self.member_id);

        // Encode arguments separately
        Encode!(&context_id, &member_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, Option<u64>)?;

        Ok(decoded)
    }
}

impl Method<Stellar> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
