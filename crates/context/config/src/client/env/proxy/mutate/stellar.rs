#![cfg(feature = "stellar_client")]

//! Stellar specific implementations for context proxy mutations.

use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::xdr::{FromXdr, ToXdr};
use soroban_sdk::{Bytes, Env};

use super::Mutate;
use crate::client::env::Method;
use crate::client::protocol::stellar::Stellar;
use crate::stellar::stellar_types::{
    FromWithEnv, StellarSignedRequest, StellarSignedRequestPayload,
};
use crate::stellar::{StellarProposalWithApprovals, StellarProxyMutateRequest};
use crate::ProposalWithApprovals;

impl Method<Stellar> for Mutate {
    type Returns = Option<ProposalWithApprovals>;

    const METHOD: &'static str = "mutate";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let env = Env::default();
        let signer_sk = SigningKey::from_bytes(&self.signing_key);

        let payload: StellarProxyMutateRequest =
            StellarProxyMutateRequest::from_with_env(self.raw_request, &env);

        let signed_request_payload = StellarSignedRequestPayload::Proxy(payload);

        let signed_request =
            StellarSignedRequest::new(&env, signed_request_payload, |b| Ok(signer_sk.sign(b)))
                .map_err(|e| eyre::eyre!("Failed to sign request: {:?}", e))?;

        let bytes: Vec<u8> = signed_request.to_xdr(&env).into_iter().collect();

        Ok(bytes)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(None);
        }
        let env = Env::default();
        let env_bytes = Bytes::from_slice(&env, &response);

        let stellar_proposal = StellarProposalWithApprovals::from_xdr(&env, &env_bytes)
            .map_err(|_| eyre::eyre!("Failed to deserialize response"))?;

        let proposal: ProposalWithApprovals = stellar_proposal.into();

        Ok(Some(proposal))
    }
}
