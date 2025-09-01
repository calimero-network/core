//! Ethereum-specific query implementations.

use std::collections::BTreeMap;

use alloy::primitives::{Address as AlloyAddress, B256};
use alloy_sol_types::SolValue;

use calimero_context_config_core::repr::{Repr, ReprTransmute};
use calimero_context_config_core::types::{Application, Capability, ContextId, ContextIdentity, Revision, SignerId};

use crate::types::{SolApplication, SolCapability, SolUserCapabilities};

// Trait for method implementations
pub trait Method<Protocol> {
    type Returns;
    const METHOD: &'static str;

    fn encode(self) -> eyre::Result<Vec<u8>>;
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns>;
}

// Ethereum protocol marker
pub struct Ethereum;

#[derive(Copy, Clone, Debug)]
pub struct ApplicationRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Ethereum> for ApplicationRequest {
    type Returns = Application<'static>;
    const METHOD: &'static str = "application(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Ethereum>>::Returns> {
        let application: SolApplication = SolValue::abi_decode(&response, false)?;
        let application: Application<'static> = application.into();
        Ok(application)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct MembersRequest {
    pub context_id: Repr<ContextId>,
    pub offset: usize,
    pub length: usize,
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

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Ethereum>>::Returns> {
        // Decode Vec<B256> directly from response
        let decoded: Vec<B256> = SolValue::abi_decode(&response, false)?;

        // Convert each B256 to ContextIdentity
        Ok(decoded
            .into_iter()
            .map(|b| b.rt().expect("infallible conversion"))
            .collect())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FetchNonceRequest {
    pub context_id: Repr<ContextId>,
    pub member_id: Repr<ContextIdentity>,
}

impl Method<Ethereum> for FetchNonceRequest {
    type Returns = Option<u64>;
    const METHOD: &'static str = "fetchNonce(bytes32,bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let member_id: [u8; 32] = self.member_id.rt().expect("infallible conversion");

        Ok((context_id, member_id).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Ethereum>>::Returns> {
        let nonce: u64 = SolValue::abi_decode(&response, false)?;

        Ok(Some(nonce))
    }
}

#[derive(Copy, Clone, Debug)]
pub struct HasMemberRequest {
    pub context_id: Repr<ContextId>,
    pub identity: Repr<ContextIdentity>,
}

impl Method<Ethereum> for HasMemberRequest {
    type Returns = bool;
    const METHOD: &'static str = "hasMember(bytes32,bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");
        let identity_bytes: [u8; 32] = self.identity.rt().expect("infallible conversion");

        Ok((context_id, identity_bytes).abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Ethereum>>::Returns> {
        let result: bool = SolValue::abi_decode(&response, false)?;
        Ok(result)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct PrivilegesRequest<'a> {
    pub context_id: Repr<ContextId>,
    pub identities: &'a [Repr<ContextIdentity>],
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

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Ethereum>>::Returns> {
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

#[derive(Copy, Clone, Debug)]
pub struct MembersRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Ethereum> for MembersRevisionRequest {
    type Returns = Revision;
    const METHOD: &'static str = "membersRevision(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Ethereum>>::Returns> {
        let revision: u64 = SolValue::abi_decode(&response, false)?;

        Ok(revision)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ProxyContractRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Ethereum> for ProxyContractRequest {
    type Returns = String;
    const METHOD: &'static str = "proxyContract(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Ethereum>>::Returns> {
        let contract_address: AlloyAddress = SolValue::abi_decode(&response, false)?;

        Ok(contract_address.to_string())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ApplicationRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Ethereum> for ApplicationRevisionRequest {
    type Returns = Revision;
    const METHOD: &'static str = "applicationRevision(bytes32)";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: [u8; 32] = self.context_id.rt().expect("infallible conversion");

        Ok(context_id.abi_encode())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Ethereum>>::Returns> {
        let revision: u64 = SolValue::abi_decode(&response, false)?;

        Ok(revision)
    }
}
