use std::collections::BTreeMap;

use alloy::primitives::Address as AlloyAddress;
use alloy_sol_types::SolValue;

use crate::client::env::config::requests::{
    ApplicationRequest, ApplicationRevisionRequest, FetchNonceRequest, HasMemberRequest,
    MembersRequest, MembersRevisionRequest, PrivilegesRequest, ProxyContractRequest,
};
use crate::client::env::config::types::ethereum::{
    SolApplication, SolCapability, SolUserCapabilities,
};
use crate::client::env::Method;
use crate::client::protocol::ethereum::Ethereum;
use crate::repr::ReprTransmute;
use crate::types::{Application, Capability, ContextIdentity, Revision, SignerId};

impl Method<Ethereum> for ApplicationRequest {
    type Returns = Application<'static>;

    const METHOD: &'static str = "application(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let application: SolApplication = SolValue::abi_decode(&response, false)?;
        let application: Application<'static> = application.into();

        Ok(application)
    }
}

impl Method<Ethereum> for ApplicationRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "applicationRevision(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let revision: u64 = SolValue::abi_decode(&response, false)?;

        Ok(revision)
    }
}

impl Method<Ethereum> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members(bytes32,uint256,uint256)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        let offset_val: u64 = self.offset as u64;
        let length_val: u64 = self.length as u64;

        Ok((context_id, offset_val, length_val).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Decode Vec<B256> directly from response
        let decoded: Vec<alloy::primitives::B256> = SolValue::abi_decode(&response, false)?;

        // Convert each B256 to ContextIdentity
        Ok(decoded
            .into_iter()
            .map(|b| b.rt().expect("infallible conversion"))
            .collect())
    }
}

impl Method<Ethereum> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "hasMember(bytes32,bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let identity_bytes: [u8; 32] = self.identity.rt().expect("infallible conversion");

        Ok((context_id, identity_bytes).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let result: bool = SolValue::abi_decode(&response, false)?;
        Ok(result)
    }
}

impl Method<Ethereum> for MembersRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "membersRevision(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let revision: u64 = SolValue::abi_decode(&response, false)?;

        Ok(revision)
    }
}

impl<'a> Method<Ethereum> for PrivilegesRequest<'a> {
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    const METHOD: &'static str = "privileges";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        let identities: Vec<[u8; 32]> = self
            .identities
            .into_iter()
            .map(|id| id.rt().expect("infallible conversion"))
            .collect();

        Ok((context_id, identities).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let user_caps: Vec<SolUserCapabilities> = SolValue::abi_decode(&response, false)?;

        let mut result = BTreeMap::new();

        for user_cap in user_caps {
            let user_id = user_cap.userId.rt().expect("infallible conversion");

            let capabilities: Result<Vec<_>, _> = user_cap
                .capabilities
                .into_iter()
                .map(|cap| -> Result<_, eyre::Report> {
                    Ok(match cap {
                        SolCapability::ManageApplication => Capability::ManageApplication,
                        SolCapability::ManageMembers => Capability::ManageMembers,
                        SolCapability::Proxy => Capability::Proxy,
                        SolCapability::__Invalid => {
                            eyre::bail!("Invalid capability encountered in response")
                        }
                    })
                })
                .collect();

            if result.insert(user_id, capabilities?).is_some() {
                eyre::bail!("Duplicate user ID in response");
            }
        }

        Ok(result)
    }
}

impl Method<Ethereum> for ProxyContractRequest {
    type Returns = String;

    const METHOD: &'static str = "proxyContract(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let contract_address: AlloyAddress = SolValue::abi_decode(&response, false)?;

        Ok(contract_address.to_string())
    }
}

impl Method<Ethereum> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetchNonce(bytes32,bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let member_id: [u8; 32] = self.member_id.rt().expect("infallible conversion");

        Ok((context_id, member_id).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let nonce: u64 = SolValue::abi_decode(&response, false)?;

        Ok(Some(nonce))
    }
}
