#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
use std::io::Cursor;
use std::mem;

use alloy::primitives::B256;
use alloy_sol_types::SolValue;
use candid::{Decode, Encode};
use serde::Serialize;
use soroban_sdk::xdr::{Limited, Limits, ReadXdr, ScVal, ToXdr};
use soroban_sdk::{BytesN, Env, IntoVal, TryIntoVal};
use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode};
use starknet::core::types::Felt;

use crate::client::env::proxy::starknet::CallData;
use crate::client::env::proxy::types::starknet::{StarknetApprovers, StarknetProposalId};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::client::protocol::icp::Icp;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::stellar::Stellar;
use crate::icp::repr::ICRepr;
use crate::repr::{Repr, ReprTransmute};
use crate::types::{ContextIdentity, ProposalId};

#[derive(Clone, Debug, Serialize)]
pub(super) struct ProposalApproversRequest {
    pub(super) proposal_id: Repr<ProposalId>,
}

impl Method<Near> for ProposalApproversRequest {
    const METHOD: &'static str = "get_proposal_approvers";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let members: Vec<Repr<ContextIdentity>> = serde_json::from_slice(&response)?;

        // safety: `Repr<T>` is a transparent wrapper around `T`
        #[expect(
            clippy::transmute_undefined_repr,
            reason = "Repr<T> is a transparent wrapper around T"
        )]
        let members =
            unsafe { mem::transmute::<Vec<Repr<ContextIdentity>>, Vec<ContextIdentity>>(members) };

        Ok(members)
    }
}

impl Method<Starknet> for ProposalApproversRequest {
    const METHOD: &'static str = "proposal_approvers";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let starknet_id: StarknetProposalId = self.proposal_id.into();

        let mut call_data = CallData::default();
        starknet_id.encode(&mut call_data)?;

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(Vec::new());
        }

        if response.len() % 32 != 0 {
            return Err(eyre::eyre!(
                "Invalid response length: {} bytes is not a multiple of 32",
                response.len()
            ));
        }

        // Convert bytes to Felts
        let mut felts = Vec::new();
        let chunks = response.chunks_exact(32);

        // Verify no remainder
        if !chunks.remainder().is_empty() {
            return Err(eyre::eyre!("Response length is not a multiple of 32 bytes"));
        }

        for chunk in chunks {
            let chunk_array: [u8; 32] = chunk
                .try_into()
                .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
            felts.push(Felt::from_bytes_be(&chunk_array));
        }

        let approvers = StarknetApprovers::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode approvers: {:?}", e))?;

        Ok(approvers.into())
    }
}

impl Method<Icp> for ProposalApproversRequest {
    const METHOD: &'static str = "proposal_approvers";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let payload = ICRepr::new(*self.proposal_id);
        Encode!(&payload).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let Some(identities) = Decode!(&response, Option<Vec<ICRepr<ContextIdentity>>>)? else {
            return Ok(Vec::new()); // Return empty Vec when None
        };

        // safety: `ICRepr<T>` is a transparent wrapper around `T`
        #[expect(
            clippy::transmute_undefined_repr,
            reason = "ICRepr<T> is a transparent wrapper around T"
        )]
        unsafe {
            Ok(mem::transmute::<
                Vec<ICRepr<ContextIdentity>>,
                Vec<ContextIdentity>,
            >(identities))
        }
    }
}

impl Method<Stellar> for ProposalApproversRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "proposal_approvers";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let proposal_id_raw: [u8; 32] = self.proposal_id.rt().expect("infallible conversion");
        let proposal_id_val: BytesN<32> = proposal_id_raw.into_val(&env);

        let args = (proposal_id_val,);

        let xdr = args.to_xdr(&env);
        Ok(xdr.to_alloc_vec())
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let cursor = Cursor::new(response);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        // Handle None case
        if sc_val == ScVal::Void {
            return Ok(Vec::new()); // Return empty vec if no approvers
        }

        let env = Env::default();
        let approvers: soroban_sdk::Vec<BytesN<32>> = sc_val
            .try_into_val(&env)
            .map_err(|e| eyre::eyre!("Failed to convert to approvers: {:?}", e))?;

        // Convert each BytesN<32> to ContextIdentity
        approvers
            .iter()
            .map(|bytes| {
                bytes
                    .to_array()
                    .rt()
                    .map_err(|e| eyre::eyre!("Failed to convert bytes to identity: {}", e))
            })
            .collect()
    }
}

impl Method<Ethereum> for ProposalApproversRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "proposalApprovers(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let proposal_id: [u8; 32] = self.proposal_id.rt().expect("infallible conversion");

        Ok(proposal_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded: Vec<B256> = SolValue::abi_decode(&response, false)?;

        let context_identities: Vec<ContextIdentity> = decoded
            .into_iter()
            .map(|bytes| bytes.rt().expect("infallible conversion"))
            .collect();

        Ok(context_identities)
    }
}
