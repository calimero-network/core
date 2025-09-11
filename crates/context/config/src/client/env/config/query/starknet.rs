use std::collections::BTreeMap;

use starknet::core::codec::{Decode as StarknetDecode, Encode as StarknetEncode, FeltWriter};
use starknet_crypto::Felt;

use crate::client::env::config::requests::{
    ApplicationRequest, ApplicationRevisionRequest, FetchNonceRequest, HasMemberRequest,
    MembersRequest, MembersRevisionRequest, PrivilegesRequest, ProxyContractRequest,
};
use crate::client::env::config::types::starknet::{
    Application as StarknetApplication, CallData, ContextId as StarknetContextId,
    ContextIdentity as StarknetContextIdentity, FeltPair, StarknetMembers, StarknetMembersRequest,
    StarknetPrivileges,
};
use crate::client::env::Method;
use crate::client::protocol::starknet::Starknet;
// use crate::repr::ReprTransmute;  // Unused import
use crate::types::{Application, Capability, ContextIdentity, Revision, SignerId};

impl Method<Starknet> for ApplicationRequest {
    type Returns = Application<'static>;

    const METHOD: &'static str = "application";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let felt_pair: FeltPair = self.context_id.into();
        let mut call_data = CallData::default();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No application found"));
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

        if felts.is_empty() {
            return Err(eyre::eyre!("No felts decoded from response"));
        }

        // Skip version felt and decode the application
        let application = StarknetApplication::decode(&felts[1..])
            .map_err(|e| eyre::eyre!("Failed to decode application: {:?}", e))?;

        Ok(application.into())
    }
}

impl Method<Starknet> for ApplicationRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "application_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let felt_pair: FeltPair = self.context_id.into();
        let mut call_data = CallData::default();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 32 bytes, got {}",
                response.len()
            ));
        }

        // Response should be a single u64 in the last 8 bytes of a felt
        let revision_bytes = &response[24..32]; // Take last 8 bytes
        let revision = u64::from_be_bytes(revision_bytes.try_into()?);

        Ok(revision)
    }
}

impl Method<Starknet> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let req: StarknetMembersRequest = self.into();
        let mut call_data = CallData::default();
        req.encode(&mut call_data)?;
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

        // Check if it's a None response (single zero Felt)
        if felts.len() == 1 && felts[0] == Felt::ZERO {
            return Ok(Vec::new());
        }

        // Decode directly from the felts slice - the Decode trait will handle the array structure
        let members = StarknetMembers::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode members: {:?}", e))?;

        Ok(members.into())
    }
}

impl Method<Starknet> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "has_member";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();

        // Encode context_id
        let context_pair: FeltPair = self.context_id.into();
        context_pair.encode(&mut call_data)?;

        // Encode identity
        let identity_pair: FeltPair = self.identity.into();
        identity_pair.encode(&mut call_data)?;

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 32 bytes, got {}",
                response.len()
            ));
        }

        // Check if all bytes except the last one are zero
        if !response[..31].iter().all(|&b| b == 0) {
            return Err(eyre::eyre!(
                "Invalid response format: non-zero bytes in prefix"
            ));
        }

        // Check the last byte is either 0 or 1
        match response[31] {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(eyre::eyre!("Invalid boolean value: {}", v)),
        }
    }
}

impl Method<Starknet> for MembersRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "members_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Dereference Repr and encode context_id
        let context_id: StarknetContextId = (*self.context_id).into();

        let mut call_data = CallData::default();
        context_id.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.len() != 32 {
            return Err(eyre::eyre!(
                "Invalid response length: expected 32 bytes, got {}",
                response.len()
            ));
        }

        // Response should be a single u64 in the last 8 bytes of a felt
        // First 24 bytes should be zero
        if !response[..24].iter().all(|&b| b == 0) {
            return Err(eyre::eyre!(
                "Invalid response format: non-zero bytes in prefix"
            ));
        }

        let revision_bytes = &response[24..32];
        let revision = u64::from_be_bytes(revision_bytes.try_into()?);

        Ok(revision)
    }
}

impl<'a> Method<Starknet> for PrivilegesRequest<'a> {
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    const METHOD: &'static str = "privileges";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();

        // Dereference Repr and encode context_id
        let context_id: StarknetContextId = (*self.context_id).into();
        context_id.encode(&mut call_data)?;

        // Add array length
        call_data.write(Felt::from(self.identities.len() as u64));

        // Add each identity using StarknetIdentity
        for identity in self.identities {
            let starknet_id: StarknetContextIdentity = (*identity).into();
            starknet_id.encode(&mut call_data)?;
        }

        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Ok(BTreeMap::new());
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

        // Check if it's a None response (single zero Felt)
        if felts.len() == 1 && felts[0] == Felt::ZERO {
            return Ok(BTreeMap::new());
        }

        let privileges = StarknetPrivileges::decode(&felts)
            .map_err(|e| eyre::eyre!("Failed to decode privileges: {:?}", e))?;

        Ok(privileges.into())
    }
}

impl Method<Starknet> for ProxyContractRequest {
    const METHOD: &'static str = "proxy_contract";

    type Returns = String;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut call_data = CallData::default();
        let felt_pair: FeltPair = self.context_id.into();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No proxy contract found"));
        }

        // Check if it's a None response (single zero Felt)
        if response.iter().all(|&x| x == 0) {
            return Err(eyre::eyre!("No proxy contract found"));
        }

        // Parse bytes as Felt
        let felt = Felt::from_bytes_be_slice(&response);

        // Format felt as hex string with 0x prefix
        Ok(format!("0x{:x}", felt))
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
