//! Mock relayer state management

use std::collections::{BTreeMap, BTreeSet};

use calimero_context_config::repr::Repr;
use calimero_context_config::types::{
    Application, Capability, ContextId, ContextIdentity, ContextStorageEntry, Revision, SignerId,
};
use calimero_context_config::{Proposal, ProposalApprovalWithSigner};

/// In-memory state for the mock relayer
#[derive(Debug, Default)]
pub struct MockState {
    /// Context metadata indexed by context ID
    pub contexts: BTreeMap<ContextId, ContextData>,
    /// Nonce tracking per member per context
    pub nonces: BTreeMap<(ContextId, ContextIdentity), u64>,
}

/// Data associated with a single context
#[derive(Debug, Clone)]
pub struct ContextData {
    /// Application metadata
    pub application: Application<'static>,
    /// Application revision counter
    pub application_revision: Revision,
    /// Set of context members
    pub members: BTreeSet<ContextIdentity>,
    /// Members revision counter
    pub members_revision: Revision,
    /// Proxy contract ID (deterministically generated from context ID)
    pub proxy_contract_id: String,
    /// Proposals for this context
    pub proposals: BTreeMap<Repr<calimero_context_config::types::ProposalId>, Proposal>,
    /// Approvals for proposals
    pub approvals:
        BTreeMap<Repr<calimero_context_config::types::ProposalId>, Vec<ProposalApprovalWithSigner>>,
    /// Context storage (key-value store)
    pub storage: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Capabilities per signer
    pub capabilities: BTreeMap<SignerId, Vec<Capability>>,
}

impl ContextData {
    /// Create new context data from an application
    pub fn new(application: Application<'static>, author_id: ContextIdentity) -> Self {
        let context_id = ContextId::from([0u8; 32]); // Placeholder for proxy contract generation
        let proxy_contract_id = Self::generate_proxy_contract_id(&context_id);

        let mut members = BTreeSet::new();
        members.insert(author_id);

        Self {
            application,
            application_revision: 1,
            members,
            members_revision: 1,
            proxy_contract_id,
            proposals: BTreeMap::new(),
            approvals: BTreeMap::new(),
            storage: BTreeMap::new(),
            capabilities: BTreeMap::new(),
        }
    }

    /// Update the proxy contract ID based on the actual context ID
    pub fn update_proxy_contract_id(&mut self, context_id: &ContextId) {
        self.proxy_contract_id = Self::generate_proxy_contract_id(context_id);
    }

    /// Generate a deterministic proxy contract ID from a context ID
    fn generate_proxy_contract_id(context_id: &ContextId) -> String {
        // Use the context ID bytes to create a deterministic proxy contract ID
        let bytes = context_id.to_bytes();
        format!("mock-proxy-{}", bs58::encode(bytes).into_string())
    }

    /// Get the number of active proposals
    pub fn active_proposals_count(&self) -> u16 {
        self.proposals.len() as u16
    }

    /// Get approvals for a proposal
    pub fn get_approvals(
        &self,
        proposal_id: &Repr<calimero_context_config::types::ProposalId>,
    ) -> usize {
        self.approvals
            .get(proposal_id)
            .map(|approvals| approvals.len())
            .unwrap_or(0)
    }

    /// Get approvers for a proposal
    pub fn get_approvers(
        &self,
        proposal_id: &Repr<calimero_context_config::types::ProposalId>,
    ) -> Vec<ContextIdentity> {
        self.approvals
            .get(proposal_id)
            .map(|approvals| {
                approvals
                    .iter()
                    .map(|approval| {
                        // Convert SignerId to ContextIdentity (they have the same underlying type)
                        // Extract bytes from Repr<SignerId> using transmute since they're the same type
                        let signer_id = approval.signer_id;
                        let signer_bytes: [u8; 32] = unsafe { std::mem::transmute(signer_id) };
                        ContextIdentity::from(signer_bytes)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get storage entries with pagination
    pub fn get_storage_entries(&self, offset: usize, limit: usize) -> Vec<ContextStorageEntry> {
        self.storage
            .iter()
            .skip(offset)
            .take(limit)
            .map(|(key, value)| ContextStorageEntry {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

impl MockState {
    /// Create a new empty mock state
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a context exists
    pub fn has_context(&self, context_id: &ContextId) -> bool {
        self.contexts.contains_key(context_id)
    }

    /// Get context data
    pub fn get_context(&self, context_id: &ContextId) -> Option<&ContextData> {
        self.contexts.get(context_id)
    }

    /// Get mutable context data
    pub fn get_context_mut(&mut self, context_id: &ContextId) -> Option<&mut ContextData> {
        self.contexts.get_mut(context_id)
    }

    /// Add a new context
    pub fn add_context(
        &mut self,
        context_id: ContextId,
        application: Application<'static>,
        author_id: ContextIdentity,
    ) {
        let mut context_data = ContextData::new(application, author_id);
        context_data.update_proxy_contract_id(&context_id);
        self.contexts.insert(context_id, context_data);
    }

    /// Get or initialize nonce for a member in a context
    pub fn get_nonce(&mut self, context_id: &ContextId, member_id: &ContextIdentity) -> u64 {
        *self.nonces.entry((*context_id, *member_id)).or_insert(0)
    }

    /// Increment nonce for a member in a context
    #[allow(dead_code)]
    pub fn increment_nonce(&mut self, context_id: &ContextId, member_id: &ContextIdentity) {
        let nonce = self.nonces.entry((*context_id, *member_id)).or_insert(0);
        *nonce += 1;
    }
}
