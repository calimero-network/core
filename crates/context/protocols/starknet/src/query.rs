//! Starknet-specific query implementations.

use std::collections::BTreeMap;

use num_traits::identities::Zero;
use starknet::core::codec::{Decode, Encode, FeltWriter};
use starknet::core::types::Felt;

use calimero_context_config_core::repr::{Repr, ReprTransmute};
use calimero_context_config_core::types::{Application, Capability, ContextId, ContextIdentity, Revision, SignerId};

use crate::types::{
    Application as StarknetApplication, CallData, FeltPair, StarknetMembers,
    StarknetMembersRequest, StarknetPrivileges, StarknetApplicationRevisionRequest,
};

// Trait for method implementations
pub trait Method<Protocol> {
    type Returns;
    const METHOD: &'static str;

    fn encode(self) -> eyre::Result<Vec<u8>>;
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns>;
}

// Starknet protocol marker
pub struct Starknet;

#[derive(Copy, Clone, Debug)]
pub struct ApplicationRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Starknet> for ApplicationRequest {
    type Returns = Application<'static>;
    const METHOD: &'static str = "application";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Convert Repr<ContextId> to ContextId, then to FeltPair
        let context_id: ContextId = self.context_id.rt().expect("infallible conversion");
        let felt_pair: FeltPair = context_id.into();

        let mut call_data = CallData::default();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Starknet>>::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No application found"));
        }

        if response.len() % 32 != 0 {
            return Err(eyre::eyre!("Invalid response length"));
        }

        let felts: Vec<Felt> = response
            .chunks(32)
            .map(|chunk| {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(chunk);
                Felt::from_bytes_be_slice(&bytes)
            })
            .collect();

        let mut iter = felts.iter();
        let application: StarknetApplication = Decode::decode_iter(&mut iter)?;
        Ok(application.into())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct MembersRequest {
    pub context_id: Repr<ContextId>,
    pub offset: u32,
    pub length: u32,
}

impl Method<Starknet> for MembersRequest {
    type Returns = Vec<ContextIdentity>;
    const METHOD: &'static str = "members";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: ContextId = self.context_id.rt().expect("infallible conversion");
        let felt_pair: FeltPair = context_id.into();

        let request = StarknetMembersRequest {
            context_id: felt_pair.into(),
            offset: self.offset,
            length: self.length,
        };

        let mut call_data = CallData::default();
        request.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Starknet>>::Returns> {
        if response.is_empty() {
            return Ok(Vec::new());
        }

        if response.len() % 32 != 0 {
            return Err(eyre::eyre!("Invalid response length"));
        }

        let felts: Vec<Felt> = response
            .chunks(32)
            .map(|chunk| {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(chunk);
                Felt::from_bytes_be_slice(&bytes)
            })
            .collect();

        let mut iter = felts.iter();
        let members: StarknetMembers = Decode::decode_iter(&mut iter)?;
        Ok(members.into())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ApplicationRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Starknet> for ApplicationRevisionRequest {
    type Returns = Revision;
    const METHOD: &'static str = "application_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: ContextId = self.context_id.rt().expect("infallible conversion");
        let felt_pair: FeltPair = context_id.into();

        let request = StarknetApplicationRevisionRequest {
            context_id: felt_pair.into(),
        };

        let mut call_data = CallData::default();
        request.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Starknet>>::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!("Invalid response length"));
        }

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&response);
        let felt = Felt::from_bytes_be_slice(&bytes);

        Ok(felt.to_bytes_be()[31] as u64)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct HasMemberRequest {
    pub context_id: Repr<ContextId>,
    pub identity: Repr<ContextIdentity>,
}

impl Method<Starknet> for HasMemberRequest {
    type Returns = bool;
    const METHOD: &'static str = "has_member";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: ContextId = self.context_id.rt().expect("infallible conversion");
        let identity: ContextIdentity = self.identity.rt().expect("infallible conversion");

        let context_felt_pair: FeltPair = context_id.into();
        let identity_felt_pair: FeltPair = identity.into();

        let mut call_data = CallData::default();
        context_felt_pair.encode(&mut call_data)?;
        identity_felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Starknet>>::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!("Invalid response length"));
        }

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&response);
        let felt = Felt::from_bytes_be_slice(&bytes);

        Ok(!felt.is_zero())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct MembersRevisionRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Starknet> for MembersRevisionRequest {
    type Returns = Revision;
    const METHOD: &'static str = "members_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: ContextId = self.context_id.rt().expect("infallible conversion");
        let felt_pair: FeltPair = context_id.into();

        let mut call_data = CallData::default();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Starknet>>::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!("Invalid response length"));
        }

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&response);
        let felt = Felt::from_bytes_be_slice(&bytes);

        Ok(felt.to_bytes_be()[31] as u64)
    }
}

#[derive(Clone, Debug)]
pub struct PrivilegesRequest {
    pub context_id: Repr<ContextId>,
    pub identities: Vec<Repr<ContextIdentity>>,
}

impl Method<Starknet> for PrivilegesRequest {
    type Returns = BTreeMap<SignerId, Vec<Capability>>;
    const METHOD: &'static str = "privileges";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: ContextId = self.context_id.rt().expect("infallible conversion");
        let context_felt_pair: FeltPair = context_id.into();

        let identities: Vec<FeltPair> = self
            .identities
            .into_iter()
            .map(|id| {
                let identity: ContextIdentity = id.rt().expect("infallible conversion");
                identity.into()
            })
            .collect();

        let mut call_data = CallData::default();
        context_felt_pair.encode(&mut call_data)?;
        
        // Encode the number of identities
        Felt::from(identities.len()).encode(&mut call_data)?;
        
        // Encode each identity
        for identity in identities {
            identity.encode(&mut call_data)?;
        }

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Starknet>>::Returns> {
        if response.is_empty() {
            return Ok(BTreeMap::new());
        }

        if response.len() % 32 != 0 {
            return Err(eyre::eyre!("Invalid response length"));
        }

        let felts: Vec<Felt> = response
            .chunks(32)
            .map(|chunk| {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(chunk);
                Felt::from_bytes_be_slice(&bytes)
            })
            .collect();

        let mut iter = felts.iter();
        let privileges: StarknetPrivileges = Decode::decode_iter(&mut iter)?;
        Ok(privileges.into())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ProxyContractRequest {
    pub context_id: Repr<ContextId>,
}

impl Method<Starknet> for ProxyContractRequest {
    type Returns = String;
    const METHOD: &'static str = "proxy_contract";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: ContextId = self.context_id.rt().expect("infallible conversion");
        let felt_pair: FeltPair = context_id.into();

        let mut call_data = CallData::default();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Starknet>>::Returns> {
        if response.is_empty() {
            return Ok(String::new());
        }

        if response.len() % 32 != 0 {
            return Err(eyre::eyre!("Invalid response length"));
        }

        let felts: Vec<Felt> = response
            .chunks(32)
            .map(|chunk| {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(chunk);
                Felt::from_bytes_be_slice(&bytes)
            })
            .collect();

        let mut iter = felts.iter();
        let contract_address: EncodableString = Decode::decode_iter(&mut iter)?;
        Ok(contract_address.0)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct FetchNonceRequest {
    pub context_id: Repr<ContextId>,
    pub member_id: Repr<ContextIdentity>,
}

impl Method<Starknet> for FetchNonceRequest {
    type Returns = Option<u64>;
    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id: ContextId = self.context_id.rt().expect("infallible conversion");
        let member_id: ContextIdentity = self.member_id.rt().expect("infallible conversion");

        let context_felt_pair: FeltPair = context_id.into();
        let member_felt_pair: FeltPair = member_id.into();

        let mut call_data = CallData::default();
        context_felt_pair.encode(&mut call_data)?;
        member_felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<<Self as Method<Starknet>>::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!("Invalid response length"));
        }

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&response);
        let felt = Felt::from_bytes_be_slice(&bytes);

        if felt.is_zero() {
            Ok(None)
        } else {
            Ok(Some(felt.to_bytes_be()[31] as u64))
        }
    }
}

// Helper type for encoding strings in Starknet
#[derive(Clone, Debug)]
pub struct EncodableString(pub String);

impl Encode for EncodableString {
    fn encode<W: FeltWriter>(&self, writer: &mut W) -> Result<(), starknet::core::codec::Error> {
        const WORD_SIZE: usize = 31;
        let bytes = self.0.as_bytes();

        // Calculate full words and pending word
        #[allow(clippy::integer_division, reason = "Not harmful here")]
        let full_words_count = bytes.len() / WORD_SIZE;
        let pending_len = bytes.len() % WORD_SIZE;

        // Write number of full words
        writer.write(Felt::from(full_words_count));

        // Write full words (31 chars each)
        for i in 0..full_words_count {
            let start = i * WORD_SIZE;
            let word_bytes = &bytes[start..start + WORD_SIZE];
            let word_hex = hex::encode(word_bytes);
            let felt = Felt::from_hex(&format!("0x{}", word_hex))
                .map_err(|e| starknet::core::codec::Error::custom(&format!("Invalid word hex: {}", e)))?;
            writer.write(felt);
        }

        // Write pending word if exists
        if pending_len > 0 {
            let pending_bytes = &bytes[full_words_count * WORD_SIZE..];
            let pending_hex = hex::encode(pending_bytes);
            let felt = Felt::from_hex(&format!("0x{}", pending_hex))
                .map_err(|e| starknet::core::codec::Error::custom(&format!("Invalid pending hex: {}", e)))?;
            writer.write(felt);
        } else {
            writer.write(Felt::ZERO);
        }

        // Write pending word length
        writer.write(Felt::from(pending_len));

        Ok(())
    }
}

impl<'a> Decode<'a> for EncodableString {
    fn decode_iter<T>(iter: &mut T) -> Result<Self, starknet::core::codec::Error>
    where
        T: Iterator<Item = &'a Felt>,
    {
        const WORD_SIZE: usize = 31;

        // Get number of full words
        let first_felt = iter.next().ok_or_else(starknet::core::codec::Error::input_exhausted)?;

        let full_words_count = first_felt.to_bytes_be()[31] as usize;

        let mut bytes = Vec::new();

        // Read full words
        for _ in 0..full_words_count {
            let word = iter.next().ok_or_else(starknet::core::codec::Error::input_exhausted)?;
            let word_bytes = word.to_bytes_be();
            bytes.extend_from_slice(&word_bytes[1..WORD_SIZE + 1]);
        }

        // Read pending word
        let pending_word = iter.next().ok_or_else(starknet::core::codec::Error::input_exhausted)?;
        let pending_bytes = pending_word.to_bytes_be();

        // Read pending length
        let pending_len = iter
            .next()
            .ok_or_else(starknet::core::codec::Error::input_exhausted)?
            .to_bytes_be()[31] as usize;

        // Handle pending bytes - find first non-zero byte and take all remaining bytes
        if pending_len > 0 {
            let start = pending_bytes.iter().position(|&x| x != 0).unwrap_or(1);
            bytes.extend_from_slice(&pending_bytes[start..]);
        }

        let string = String::from_utf8(bytes).map_err(|_| starknet::core::codec::Error::custom("Invalid UTF-8"))?;

        Ok(EncodableString(string))
    }
}
