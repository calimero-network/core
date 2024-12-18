use starknet::core::codec::{Decode, Encode, Error, FeltWriter};
use starknet::core::types::{Felt, U256};
use eyre::anyhow;

use crate::repr::{Repr, ReprBytes, ReprTransmute};
use crate::types::{ContextIdentity, ContextStorageEntry, ProposalId, SignerId};
use crate::{
    Proposal, ProposalAction, ProposalApprovalWithSigner, ProposalWithApprovals, ProxyMutateRequest,
};

#[derive(Clone, Copy, Debug, Encode, Decode)]
pub struct FeltPair {
    pub high: Felt,
    pub low: Felt,
}

#[derive(Clone, Copy, Debug, Encode, Decode)]
pub struct StarknetIdentity(pub FeltPair);

#[derive(Clone, Copy, Debug, Encode, Decode)]
pub struct StarknetProposalId(pub FeltPair);

#[derive(Clone, Copy, Debug, Encode, Decode)]
pub struct StarknetU256(pub FeltPair);

#[derive(Debug, Clone, Decode)]
pub struct ContextVariableKey(pub Vec<u8>);

// Implement From for the conversion
impl From<Vec<u8>> for ContextVariableKey {
    fn from(key: Vec<u8>) -> Self {
        ContextVariableKey(key)
    }
}

// Implement Encode for ContextVariableKey
impl Encode for ContextVariableKey {
    fn encode<W: FeltWriter>(&self, writer: &mut W) -> Result<(), Error> {
        let bytes = &self.0;

        // Use exactly 16 bytes per chunk
        let chunk_size = 16;
        #[allow(clippy::integer_division, reason = "Using integer division for ceiling calculation is intentional here")]
        let num_chunks = (bytes.len() + chunk_size - 1) / chunk_size;

        // Write number of chunks first
        writer.write(Felt::from(num_chunks));

        // Process each chunk
        for i in 0..num_chunks {
            let start = i * chunk_size;
            let end = std::cmp::min((i + 1) * chunk_size, bytes.len());
            let chunk = &bytes[start..end];

            let chunk_hex = hex::encode(chunk);
            let chunk_felt = Felt::from_hex(&format!("0x{}", chunk_hex))
                .map_err(|e| Error::custom(&format!("Invalid chunk hex: {}", e)))?;

            writer.write(chunk_felt);
        }

        Ok(())
    }
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetProposal {
    pub proposal_id: StarknetProposalId,
    pub author_id: StarknetIdentity,
    pub actions: StarknetProposalActionWithArgs,
}

#[derive(Clone, Copy, Debug, Encode, Decode)]
pub struct StarknetConfirmationRequest {
    pub proposal_id: StarknetProposalId,
    pub signer_id: StarknetIdentity,
    pub added_timestamp: Felt,
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetProxyMutateRequest {
    pub signer_id: StarknetIdentity,
    pub kind: StarknetProxyMutateRequestKind,
}

#[derive(Debug, Encode, Decode)]
pub enum StarknetProxyMutateRequestKind {
    Propose(StarknetProposal),
    Approve(StarknetConfirmationRequest),
}

#[derive(Debug, Encode, Decode)]
pub enum StarknetProposalActionWithArgs {
    ExternalFunctionCall(Felt, Felt, StarknetU256, Vec<Felt>),
    Transfer(Felt, StarknetU256),
    SetNumApprovals(Felt),
    SetActiveProposalsLimit(Felt),
    SetContextValue(Vec<Felt>, Vec<Felt>),
    DeleteProposal(StarknetProposalId),
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetSignedRequest {
    pub payload: Vec<Felt>,
    pub signature_r: Felt,
    pub signature_s: Felt,
}

#[derive(Clone, Copy, Debug, Decode)]
pub struct StarknetProposalWithApprovals {
    pub proposal_id: StarknetProposalId,
    pub num_approvals: Felt,
}

#[derive(Debug, Decode)]
pub struct StarknetApprovers {
    pub approvers: Vec<StarknetIdentity>,
}

#[derive(Debug, Decode)]
pub struct StarknetProposals {
    pub proposals: Vec<StarknetProposal>,
}

impl From<StarknetProposals> for Vec<Proposal> {
    fn from(value: StarknetProposals) -> Self {
        value.proposals.into_iter().map(Into::into).collect()
    }
}

// Conversions for StarknetIdentity
impl From<SignerId> for FeltPair {
    fn from(value: SignerId) -> Self {
        let bytes = value.as_bytes();
        let mid_point = bytes.len().checked_div(2).expect("Length should be even");
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);
        FeltPair {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

impl From<FeltPair> for SignerId {
    fn from(value: FeltPair) -> Self {
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&value.high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&value.low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

impl From<SignerId> for StarknetIdentity {
    fn from(value: SignerId) -> Self {
        StarknetIdentity(value.into())
    }
}

impl From<StarknetIdentity> for SignerId {
    fn from(value: StarknetIdentity) -> Self {
        value.0.into()
    }
}

// Conversions for ProposalId
impl From<ProposalId> for FeltPair {
    fn from(value: ProposalId) -> Self {
        let bytes = value.as_bytes();
        let mid_point = bytes.len().checked_div(2).expect("Length should be even");
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);
        FeltPair {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

impl From<ProposalId> for StarknetProposalId {
    fn from(value: ProposalId) -> Self {
        StarknetProposalId(value.into())
    }
}

impl From<Repr<ProposalId>> for StarknetProposalId {
    fn from(value: Repr<ProposalId>) -> Self {
        StarknetProposalId((*value).into())
    }
}

impl From<Repr<SignerId>> for StarknetIdentity {
    fn from(value: Repr<SignerId>) -> Self {
        StarknetIdentity((*value).into())
    }
}

// Conversions for U256
impl From<U256> for StarknetU256 {
    fn from(value: U256) -> Self {
        StarknetU256(FeltPair {
            high: Felt::from(value.high()),
            low: Felt::from(value.low()),
        })
    }
}

impl From<u128> for StarknetU256 {
    fn from(value: u128) -> Self {
        StarknetU256(FeltPair {
            high: Felt::ZERO,
            low: Felt::from(value),
        })
    }
}

// Conversions for ProxyMutateRequest
impl From<(SignerId, ProxyMutateRequest)> for StarknetProxyMutateRequest {
    fn from((signer_id, request): (SignerId, ProxyMutateRequest)) -> Self {
        StarknetProxyMutateRequest {
            signer_id: signer_id.into(),
            kind: request.into(),
        }
    }
}

impl From<ProxyMutateRequest> for StarknetProxyMutateRequestKind {
    fn from(request: ProxyMutateRequest) -> Self {
        match request {
            ProxyMutateRequest::Propose { proposal } => {
                StarknetProxyMutateRequestKind::Propose(proposal.into())
            }
            ProxyMutateRequest::Approve { approval } => {
                StarknetProxyMutateRequestKind::Approve(approval.into())
            }
        }
    }
}

// Conversions for Proposal
impl From<Proposal> for StarknetProposal {
    fn from(proposal: Proposal) -> Self {
        StarknetProposal {
            proposal_id: proposal.id.into(),
            author_id: proposal.author_id.into(),
            actions: proposal.actions.into(),
        }
    }
}

impl From<StarknetProposal> for Proposal {
    fn from(value: StarknetProposal) -> Self {
        Proposal {
            id: Repr::new(value.proposal_id.into()),
            author_id: Repr::new(value.author_id.into()),
            actions: vec![value.actions.into()],
        }
    }
}

// Conversions for ProposalApproval
impl From<ProposalApprovalWithSigner> for StarknetConfirmationRequest {
    fn from(approval: ProposalApprovalWithSigner) -> Self {
        StarknetConfirmationRequest {
            proposal_id: approval.proposal_id.into(),
            signer_id: approval.signer_id.into(),
            added_timestamp: Felt::from(approval.added_timestamp),
        }
    }
}

// Conversions for Actions
impl From<Vec<ProposalAction>> for StarknetProposalActionWithArgs {
    fn from(actions: Vec<ProposalAction>) -> Self {
        let action = actions
            .into_iter()
            .next()
            .expect("At least one action required");
        match action {
            ProposalAction::ExternalFunctionCall {
                receiver_id,
                method_name,
                args,
                deposit,
                ..
            } => {
                // Parse the JSON string into a Value first
                let args_value: serde_json::Value =
                    serde_json::from_str(&args).expect("Invalid JSON arguments");
                // Convert JSON values to Starknet-compatible felt arguments
                let felt_args = match args_value {
                    serde_json::Value::Object(map) => {
                        map.into_iter()
                            .map(|(_, value)| json_value_to_felt(value))
                            .collect::<Result<Vec<_>, _>>()
                            .expect("Failed to convert arguments to Felt")
                    },
                    serde_json::Value::Array(arr) => {
                        arr.into_iter()
                            .map(json_value_to_felt)
                            .collect::<Result<Vec<_>, _>>()
                            .expect("Failed to convert arguments to Felt")
                    },
                    value => vec![json_value_to_felt(value)
                        .expect("Failed to convert argument to Felt")],
                };

                StarknetProposalActionWithArgs::ExternalFunctionCall(
                    Felt::from_bytes_be_slice(receiver_id.as_bytes()),
                    Felt::from_bytes_be_slice(method_name.as_bytes()),
                    StarknetU256::from(deposit),
                    felt_args,
                )
            }
            ProposalAction::Transfer {
                receiver_id,
                amount,
            } => StarknetProposalActionWithArgs::Transfer(
                Felt::from_bytes_be_slice(receiver_id.as_bytes()),
                amount.into(),
            ),
            ProposalAction::SetNumApprovals { num_approvals } => {
                StarknetProposalActionWithArgs::SetNumApprovals(Felt::from(num_approvals))
            }
            ProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            } => StarknetProposalActionWithArgs::SetActiveProposalsLimit(Felt::from(
                active_proposals_limit,
            )),
            ProposalAction::SetContextValue { key, value } => {
                StarknetProposalActionWithArgs::SetContextValue(
                    key.chunks(16).map(Felt::from_bytes_be_slice).collect(),
                    value.chunks(16).map(Felt::from_bytes_be_slice).collect(),
                )
            }
            ProposalAction::DeleteProposal { proposal_id } => {
                StarknetProposalActionWithArgs::DeleteProposal(proposal_id.into())
            }
        }
    }
}

impl From<StarknetProposalActionWithArgs> for ProposalAction {
    fn from(action: StarknetProposalActionWithArgs) -> Self {
        match action {
            StarknetProposalActionWithArgs::ExternalFunctionCall(
                contract,
                selector,
                amount,
                calldata,
            ) => ProposalAction::ExternalFunctionCall {
                receiver_id: format!("0x{}", hex::encode(contract.to_bytes_be())),
                method_name: format!("0x{}", hex::encode(selector.to_bytes_be())),
                args: calldata
                    .iter()
                    .map(|felt| format!("0x{}", hex::encode(felt.to_bytes_be())))
                    .collect::<Vec<_>>()
                    .join(","),
                deposit: u128::from_be_bytes(
                    amount.0.low.to_bytes_be()[16..32].try_into().unwrap(),
                ) + (u128::from_be_bytes(
                    amount.0.high.to_bytes_be()[16..32].try_into().unwrap(),
                ) << 64),
                gas: 0,
            },
            StarknetProposalActionWithArgs::Transfer(receiver, amount) => {
                let FeltPair { high, low } = amount.0;
                ProposalAction::Transfer {
                    receiver_id: format!("0x{}", hex::encode(receiver.to_bytes_be())),
                    amount: u128::from_be_bytes(low.to_bytes_be()[16..32].try_into().unwrap())
                        + (u128::from_be_bytes(high.to_bytes_be()[16..32].try_into().unwrap())
                            << 64),
                }
            }
            StarknetProposalActionWithArgs::SetNumApprovals(num) => {
                ProposalAction::SetNumApprovals {
                    num_approvals: u32::from_be_bytes(
                        num.to_bytes_be()[28..32].try_into().unwrap(),
                    ),
                }
            }
            StarknetProposalActionWithArgs::SetActiveProposalsLimit(limit) => {
                ProposalAction::SetActiveProposalsLimit {
                    active_proposals_limit: u32::from_be_bytes(
                        limit.to_bytes_be()[28..32].try_into().unwrap(),
                    ),
                }
            }
            StarknetProposalActionWithArgs::SetContextValue(key, value) => {
                ProposalAction::SetContextValue {
                    key: key.iter().flat_map(|felt| felt.to_bytes_be()).collect(),
                    value: value.iter().flat_map(|felt| felt.to_bytes_be()).collect(),
                }
            }
            StarknetProposalActionWithArgs::DeleteProposal(proposal_id) => {
                ProposalAction::DeleteProposal {
                    proposal_id: Repr::new(proposal_id.into()),
                }
            }
        }
    }
}

impl From<StarknetProposalWithApprovals> for ProposalWithApprovals {
    fn from(value: StarknetProposalWithApprovals) -> Self {
        ProposalWithApprovals {
            proposal_id: Repr::new(value.proposal_id.into()),
            num_approvals: u32::from_be_bytes(
                value.num_approvals.to_bytes_be()[28..32]
                    .try_into()
                    .unwrap(),
            ) as usize,
        }
    }
}

impl From<StarknetApprovers> for Vec<ContextIdentity> {
    fn from(value: StarknetApprovers) -> Self {
        value
            .approvers
            .into_iter()
            .map(|identity| {
                let mut bytes = [0u8; 32];
                bytes[..16].copy_from_slice(&identity.0.high.to_bytes_be()[16..]);
                bytes[16..].copy_from_slice(&identity.0.low.to_bytes_be()[16..]);
                bytes.rt().expect("Infallible conversion")
            })
            .collect()
    }
}

#[derive(Default, Debug)]
pub struct CallData(pub Vec<u8>);

impl FeltWriter for CallData {
    fn write(&mut self, felt: Felt) {
        self.0.extend(felt.to_bytes_be())
    }
}

#[derive(Clone, Copy, Debug, Encode)]
pub struct StarknetProposalsRequest {
    pub offset: Felt,
    pub length: Felt,
}

impl From<FeltPair> for ProposalId {
    fn from(value: FeltPair) -> Self {
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&value.high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&value.low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

impl From<StarknetProposalId> for ProposalId {
    fn from(value: StarknetProposalId) -> Self {
        value.0.into()
    }
}

#[derive(Clone, Copy, Debug, Encode)]
pub struct StarknetContextStorageEntriesRequest {
    pub offset: Felt,
    pub length: Felt,
}

// First, create a type to represent the response structure
#[derive(Debug)]
pub struct ContextStorageEntriesResponse {
    pub entries: Vec<(Vec<Felt>, Vec<Felt>)>,
}

impl<'a> Decode<'a> for ContextStorageEntriesResponse {
    fn decode_iter<T>(iter: &mut T) -> Result<Self, Error>
    where
        T: Iterator<Item = &'a Felt>,
    {
        // First felt is number of entries
        let num_entries = match iter.next() {
            Some(felt) => felt.to_bytes_be()[31] as usize,
            None => return Ok(Self { entries: vec![] }),
        };

        let mut entries = Vec::new();

        // Read exactly num_entries pairs
        for _ in 0..num_entries {
            // Get key array length and contents
            if let Some(key_len) = iter.next() {
                let key_len = key_len.to_bytes_be()[31] as usize;
                let mut key = Vec::new();
                for _ in 0..key_len {
                    if let Some(felt) = iter.next() {
                        key.push(*felt);
                    }
                }

                // Get value array length and contents
                if let Some(value_len) = iter.next() {
                    let value_len = value_len.to_bytes_be()[31] as usize;
                    let mut value = Vec::new();
                    for _ in 0..value_len {
                        if let Some(felt) = iter.next() {
                            value.push(*felt);
                        }
                    }
                    entries.push((key, value));
                }
            }
        }

        Ok(Self { entries })
    }
}

impl From<(Vec<Felt>, Vec<Felt>)> for ContextStorageEntry {
    fn from((key_felts, value_felts): (Vec<Felt>, Vec<Felt>)) -> Self {
        let key = key_felts
            .iter()
            .flat_map(|f| f.to_bytes_be())
            .filter(|&b| b != 0)
            .collect();

        let value = value_felts
            .iter()
            .flat_map(|f| f.to_bytes_be())
            .filter(|&b| b != 0)
            .collect();

        ContextStorageEntry { key, value }
    }
}

// Helper function to convert JSON values to Felts
fn json_value_to_felt(value: serde_json::Value) -> Result<Felt, eyre::Error> {
    match value {
        serde_json::Value::String(s) => {
            if s.starts_with("0x") {
                // Handle hex strings directly
                Felt::from_hex(&s).map_err(|e| anyhow!("Invalid hex string: {}", e))
            } else {
                Ok(Felt::from_bytes_be_slice(s.as_bytes()))
            }
        },
        serde_json::Value::Number(n) => {
            if let Some(n) = n.as_u64() {
                // Handle integers directly
                Ok(Felt::from(n))
            } else {
                // Fall back to string conversion for other numbers
                Ok(Felt::from_bytes_be_slice(n.to_string().as_bytes()))
            }
        },
        serde_json::Value::Array(arr) => {
            let json_str = serde_json::to_string(&arr)
                .map_err(|e| anyhow!("Failed to serialize array: {}", e))?;
            Ok(Felt::from_bytes_be_slice(json_str.as_bytes()))
        },
        serde_json::Value::Object(obj) => {
            let json_str = serde_json::to_string(&obj)
                .map_err(|e| anyhow!("Failed to serialize object: {}", e))?;
            Ok(Felt::from_bytes_be_slice(json_str.as_bytes()))
        },
        serde_json::Value::Bool(b) => Ok(Felt::from(b as u64)),
        serde_json::Value::Null => Ok(Felt::ZERO),
    }
}
