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
use crate::icp::repr::ICRepr;
use crate::repr::Repr;
use crate::types::{ContextId, ContextIdentity};

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct FetchNonceRequest {
    pub(super) context_id: Repr<ContextId>,
    pub(super) member: Repr<ContextIdentity>,
}

impl FetchNonceRequest {
    pub const fn new(context_id: ContextId, member: ContextIdentity) -> Self {
      
        Self {
            context_id: Repr::new(context_id),
            member: Repr::new(member),
        }
    }
}

impl Method<Near> for FetchNonceRequest {
    const METHOD: &'static str = "fetch_nonce";

    type Returns = u64;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let nonce: u64 =
            serde_json::from_slice(&response)?;
       
        Ok(nonce)
    }
}

impl Method<Starknet> for FetchNonceRequest {
    type Returns = u64;

    const METHOD: &'static str  = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();

        // Dereference Repr and encode context_id
        let context_id: StarknetContextId = (*self.context_id).into();
        context_id.encode(&mut call_data)?;

        let member: StarknetContextIdentity = (*self.member).into();
        member.encode(&mut call_data)?;

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
    
        Ok(nonce)
    }
}

impl Method<Icp> for FetchNonceRequest {
    type Returns = u64;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        let member = ICRepr::new(*self.member);

        let payload = (context_id, member);

        Encode!(&payload).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, u64)?;

        Ok(decoded)
    }
}
