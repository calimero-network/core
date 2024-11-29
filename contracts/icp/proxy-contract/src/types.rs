use std::collections::{HashMap, HashSet};
use candid::CandidType;
use serde::{Deserialize, Serialize};
use ed25519_dalek::{Signature, VerifyingKey, Verifier};

/// Base identity type
pub type Identity = [u8; 32];

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq, Default)]
pub struct ICSignerId(pub Identity);

impl ICSignerId {
  pub fn new(bytes: [u8; 32]) -> Self {
      Self(bytes)
  }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq, Default)]
pub struct ICContextId(pub Identity);

impl ICContextId {
  pub fn new(bytes: [u8; 32]) -> Self {
      Self(bytes)
  }
}

pub type ICGas = u64;
pub type ICNativeToken = u128;

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
pub struct ICProposalId(pub [u8; 32]);

impl ICProposalId {
  pub fn new(bytes: [u8; 32]) -> Self {
      Self(bytes)
  }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ICProposalAction {
    ExternalFunctionCall {
        receiver_id: Identity,
        method_name: String,
        args: String,
        deposit: ICNativeToken,
    },
    Transfer {
        receiver_id: Identity,
        amount: ICNativeToken,
    },
    SetNumApprovals {
        num_approvals: u32,
    },
    SetActiveProposalsLimit {
        active_proposals_limit: u32,
    },
    SetContextValue {
        key: Vec<u8>,
        value: Vec<u8>,
    },
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICProposal {
    pub id: ICProposalId,
    pub author_id: ICSignerId,
    pub actions: Vec<ICProposalAction>,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICProposalWithApprovals {
    pub proposal_id: ICProposalId,
    pub num_approvals: usize,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICProposalApprovalWithSigner {
    pub proposal_id: ICProposalId,
    pub signer_id: ICSignerId,
    pub added_timestamp: u64,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub enum ICRequestKind {
    Propose { proposal: ICProposal },
    Approve { approval: ICProposalApprovalWithSigner },
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICRequest {
    pub kind: ICRequestKind,
    pub signer_id: ICSignerId,
    pub timestamp_ms: u64,
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ICPSigned<T: CandidType + Serialize> {
    pub payload: T,
    pub signature: Vec<u8>,
}

impl<T: CandidType + Serialize> ICPSigned<T> {
    pub fn parse<F>(&self, f: F) -> Result<&T, &'static str>
    where
        F: FnOnce(&T) -> &ICSignerId,
    {
        // Get the signer's public key from the payload
        let signer_id = f(&self.payload);

        // Convert signer_id to VerifyingKey (public key)
        let verifying_key =
            VerifyingKey::from_bytes(&signer_id.0).map_err(|_| "invalid public key")?;

        // Serialize the payload for verification
        let message =
            candid::encode_one(&self.payload).map_err(|_| "failed to serialize payload")?;

        // Convert signature bytes to ed25519::Signature
        let signature = Signature::from_slice(&self.signature)
            .map_err(|_| "invalid signature format")?;

        // Verify the signature
        verifying_key
            .verify(&message, &signature)
            .map_err(|_| "invalid signature")?;

        // Return the payload after successful verification
        Ok(&self.payload)
    }
}

#[derive(CandidType, Serialize, Deserialize, Default)]
pub struct ICProxyContract {
    pub context_id: ICContextId,
    pub context_config_id: String,
    pub num_approvals: u32,
    pub proposals: HashMap<ICProposalId, ICProposal>,
    pub approvals: HashMap<ICProposalId, HashSet<ICSignerId>>,
    pub num_proposals_pk: HashMap<ICSignerId, u32>,
    pub active_proposals_limit: u32,
    pub context_storage: HashMap<Vec<u8>, Vec<u8>>,
    pub code_size: (u64, Option<u64>),
}

impl ICProxyContract {
  pub fn new(context_id: ICContextId) -> Self {
      Self {
          context_id,
          context_config_id: ic_cdk::api::id().to_string(),
          num_approvals: 3,
          proposals: HashMap::new(),
          approvals: HashMap::new(),
          num_proposals_pk: HashMap::new(),
          active_proposals_limit: 10,
          context_storage: HashMap::new(),
          code_size: (0, None),
      }
  }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct LedgerTransferArgs {
  pub to: String,
  pub amount: ICNativeToken,
}
