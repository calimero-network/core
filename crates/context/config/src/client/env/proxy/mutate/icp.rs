#![cfg(feature = "icp_client")]

//! Internet Computer (ICP) specific implementations for context proxy mutations.

use candid::Decode;
use ed25519_dalek::{Signer, SigningKey};

use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::icp::types::ICSigned;
use crate::icp::{ICProposalWithApprovals, ICProxyMutateRequest};
use crate::ProposalWithApprovals;

impl Method<Icp> for Mutate {
    type Returns = Option<ProposalWithApprovals>;

    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let payload: ICProxyMutateRequest =
            self.raw_request.try_into().map_err(eyre::Report::msg)?;

        let signed = ICSigned::new(payload, |b| signer_sk.sign(b))?;

        let encoded = candid::encode_one(&signed)?;

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, Option<ICProposalWithApprovals>)?;
        Ok(decoded.map(Into::into))
    }
}
