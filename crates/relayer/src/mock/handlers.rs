//! Request handlers for the mock relayer

use borsh::BorshDeserialize;
use calimero_context_config::client::transport::Operation;
use calimero_context_config::repr::Repr;
use calimero_context_config::types::{ContextId, ContextIdentity, ContextStorageEntry, SignerId};
use calimero_context_config::{
    ContextRequest, ContextRequestKind, Proposal, ProposalApprovalWithSigner,
    ProposalWithApprovals, ProxyMutateRequest, RequestKind,
};
use eyre::{eyre, Result as EyreResult};

use super::state::MockState;

/// Handlers for mock relayer operations
pub struct MockHandlers;

/// Helper to convert ContextIdentity to SignerId (they have the same underlying [u8; 32] representation)
fn identity_to_signer_id(identity: &ContextIdentity) -> SignerId {
    // ContextIdentity is Copy, so we can dereference it
    unsafe { std::mem::transmute(*identity) }
}

impl MockHandlers {
    /// Handle a request based on operation type
    pub fn handle_operation(
        state: &mut MockState,
        operation: &Operation<'_>,
        payload: &[u8],
    ) -> EyreResult<Vec<u8>> {
        match operation {
            Operation::Read { method } => Self::handle_read(state, method, payload),
            Operation::Write { method } => Self::handle_write(state, method, payload),
        }
    }

    /// Handle read operations (queries)
    fn handle_read(state: &mut MockState, method: &str, payload: &[u8]) -> EyreResult<Vec<u8>> {
        match method {
            // Context-config query methods
            "application" => Self::handle_application(state, payload),
            "application_revision" => Self::handle_application_revision(state, payload),
            "members" => Self::handle_members(state, payload),
            "members_revision" => Self::handle_members_revision(state, payload),
            "has_member" => Self::handle_has_member(state, payload),
            "privileges" => Self::handle_privileges(state, payload),
            "get_proxy_contract" => Self::handle_get_proxy_contract(state, payload),
            "fetch_nonce" => Self::handle_fetch_nonce(state, payload),

            // Proxy query methods
            "proposals" => Self::handle_proposals(state, payload),
            "proposal" => Self::handle_proposal(state, payload),
            "get_number_of_active_proposals" => {
                Self::handle_get_number_of_active_proposals(state, payload)
            }
            "get_number_of_proposal_approvals" => {
                Self::handle_get_number_of_proposal_approvals(state, payload)
            }
            "get_proposal_approvers" => Self::handle_get_proposal_approvers(state, payload),
            "get_context_value" => Self::handle_get_context_value(state, payload),
            "get_context_storage_entries" => {
                Self::handle_get_context_storage_entries(state, payload)
            }

            _ => Err(eyre!("Unknown read method: {}", method)),
        }
    }

    /// Handle write operations (mutations)
    fn handle_write(state: &mut MockState, method: &str, payload: &[u8]) -> EyreResult<Vec<u8>> {
        match method {
            "mutate" => Self::handle_mutate(state, payload),
            "proxy_mutate" => Self::handle_proxy_mutate(state, payload),
            _ => Err(eyre!("Unknown write method: {}", method)),
        }
    }

    // Context-config query handlers

    fn handle_application(state: &MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            context_id: Repr<ContextId>,
        }

        let request: Request = borsh::from_slice(payload)?;
        let context_id = request.context_id.into_inner();

        let context = state
            .get_context(&context_id)
            .ok_or_else(|| eyre!("Context not found"))?;

        Ok(borsh::to_vec(&context.application)?)
    }

    fn handle_application_revision(state: &MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            context_id: Repr<ContextId>,
        }

        let request: Request = borsh::from_slice(payload)?;
        let context_id = request.context_id.into_inner();

        let context = state
            .get_context(&context_id)
            .ok_or_else(|| eyre!("Context not found"))?;

        Ok(borsh::to_vec(&context.application_revision)?)
    }

    fn handle_members(state: &MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            context_id: Repr<ContextId>,
            offset: usize,
            length: usize,
        }

        let request: Request = borsh::from_slice(payload)?;
        let context_id = request.context_id.into_inner();

        let context = state
            .get_context(&context_id)
            .ok_or_else(|| eyre!("Context not found"))?;

        let members: Vec<ContextIdentity> = context
            .members
            .iter()
            .skip(request.offset)
            .take(request.length)
            .copied()
            .collect();

        Ok(borsh::to_vec(&members)?)
    }

    fn handle_members_revision(state: &MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            context_id: Repr<ContextId>,
        }

        let request: Request = borsh::from_slice(payload)?;
        let context_id = request.context_id.into_inner();

        let context = state
            .get_context(&context_id)
            .ok_or_else(|| eyre!("Context not found"))?;

        Ok(borsh::to_vec(&context.members_revision)?)
    }

    fn handle_has_member(state: &MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            context_id: Repr<ContextId>,
            identity: Repr<ContextIdentity>,
        }

        let request: Request = borsh::from_slice(payload)?;
        let context_id = request.context_id.into_inner();
        let identity = request.identity.into_inner();

        let context = state
            .get_context(&context_id)
            .ok_or_else(|| eyre!("Context not found"))?;

        let has_member = context.members.contains(&identity);
        Ok(borsh::to_vec(&has_member)?)
    }

    fn handle_privileges(_state: &MockState, _payload: &[u8]) -> EyreResult<Vec<u8>> {
        // For mock mode, capabilities/privileges are simplified
        // Return empty vector (no privileges in mock mode)
        Ok(Vec::new())
    }

    fn handle_get_proxy_contract(state: &MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            context_id: Repr<ContextId>,
        }

        let request: Request = borsh::from_slice(payload)?;
        let context_id = request.context_id.into_inner();

        let context = state
            .get_context(&context_id)
            .ok_or_else(|| eyre!("Context not found"))?;

        Ok(borsh::to_vec(&context.proxy_contract_id)?)
    }

    fn handle_fetch_nonce(state: &mut MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            context_id: Repr<ContextId>,
            member_id: Repr<ContextIdentity>,
        }

        let request: Request = borsh::from_slice(payload)?;
        let context_id = request.context_id.into_inner();
        let member_id = request.member_id.into_inner();

        if !state.has_context(&context_id) {
            return Err(eyre!("Context not found"));
        }

        let nonce = state.get_nonce(&context_id, &member_id);
        let result = Some(nonce);
        Ok(borsh::to_vec(&result)?)
    }

    // Context-config mutate handlers

    fn handle_mutate(state: &mut MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        // Decode the signed request
        // Skip signature verification in mock mode
        // Deserialize directly from bytes
        let kind: RequestKind<'_> = serde_json::from_slice(payload)?;

        match kind {
            RequestKind::Context(context_request) => {
                Self::handle_context_request(state, context_request)?;
                Ok(Vec::new())
            }
        }
    }

    fn handle_context_request(state: &mut MockState, request: ContextRequest) -> EyreResult<()> {
        let context_id = request.context_id.into_inner();

        match request.kind {
            ContextRequestKind::Add {
                author_id,
                application,
            } => {
                let author = author_id.into_inner();
                // Convert to owned version by creating a new Application with 'static lifetime
                use calimero_context_config::types::Application;
                let app_owned = Application::new(
                    application.id,
                    application.blob,
                    application.size,
                    application.source.to_owned(),
                    application.metadata.to_owned(),
                );
                state.add_context(context_id, app_owned, author);
                Ok(())
            }
            ContextRequestKind::UpdateApplication { application } => {
                let context = state
                    .get_context_mut(&context_id)
                    .ok_or_else(|| eyre!("Context not found"))?;

                // Convert to owned version by creating a new Application with 'static lifetime
                use calimero_context_config::types::Application;
                let owned_app = Application::new(
                    application.id,
                    application.blob,
                    application.size,
                    application.source.to_owned(),
                    application.metadata.to_owned(),
                );
                context.application = owned_app;
                context.application_revision += 1;
                Ok(())
            }
            ContextRequestKind::AddMembers { members } => {
                let context = state
                    .get_context_mut(&context_id)
                    .ok_or_else(|| eyre!("Context not found"))?;

                for member_repr in members.iter() {
                    context.members.insert((*member_repr).into_inner());
                }
                context.members_revision += 1;
                Ok(())
            }
            ContextRequestKind::RemoveMembers { members } => {
                let context = state
                    .get_context_mut(&context_id)
                    .ok_or_else(|| eyre!("Context not found"))?;

                for member_repr in members.iter() {
                    context.members.remove(&(*member_repr).into_inner());
                }
                context.members_revision += 1;
                Ok(())
            }
            ContextRequestKind::Grant { capabilities } => {
                let context = state
                    .get_context_mut(&context_id)
                    .ok_or_else(|| eyre!("Context not found"))?;

                for (identity_repr, capability) in capabilities.iter() {
                    let identity = (*identity_repr).into_inner();
                    let signer_id = identity_to_signer_id(&identity);
                    context
                        .capabilities
                        .entry(signer_id)
                        .or_insert_with(Vec::new)
                        .push(*capability);
                }
                Ok(())
            }
            ContextRequestKind::Revoke { capabilities } => {
                let context = state
                    .get_context_mut(&context_id)
                    .ok_or_else(|| eyre!("Context not found"))?;

                for (identity_repr, capability) in capabilities.iter() {
                    let identity = (*identity_repr).into_inner();
                    let signer_id = identity_to_signer_id(&identity);
                    if let Some(caps) = context.capabilities.get_mut(&signer_id) {
                        caps.retain(|c| c != capability);
                    }
                }
                Ok(())
            }
            ContextRequestKind::UpdateProxyContract => {
                // In mock mode, proxy contract is deterministic, so this is a no-op
                Ok(())
            }
            ContextRequestKind::CommitOpenInvitation { .. }
            | ContextRequestKind::RevealOpenInvitation { .. } => {
                // For mock mode, we can treat invitations as no-ops
                Ok(())
            }
        }
    }

    // Proxy query handlers

    fn handle_proposals(state: &MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            offset: usize,
            length: usize,
        }

        let request: Request = borsh::from_slice(payload)?;

        // For mock mode, we need to aggregate proposals from all contexts
        // In a real implementation, this would be context-specific
        let mut all_proposals: Vec<Proposal> = Vec::new();
        for context in state.contexts.values() {
            all_proposals.extend(context.proposals.values().cloned());
        }

        let proposals: Vec<Proposal> = all_proposals
            .into_iter()
            .skip(request.offset)
            .take(request.length)
            .collect();

        Ok(borsh::to_vec(&proposals)?)
    }

    fn handle_proposal(state: &MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            proposal_id: Repr<calimero_context_config::types::ProposalId>,
        }

        let request: Request = borsh::from_slice(payload)?;

        // Search for the proposal in all contexts
        for context in state.contexts.values() {
            if let Some(proposal) = context.proposals.get(&request.proposal_id) {
                let result = Some(proposal.clone());
                return Ok(borsh::to_vec(&result)?);
            }
        }

        let result: Option<Proposal> = None;
        Ok(borsh::to_vec(&result)?)
    }

    fn handle_get_number_of_active_proposals(
        state: &MockState,
        _payload: &[u8],
    ) -> EyreResult<Vec<u8>> {
        // Aggregate active proposals from all contexts
        let count: u16 = state
            .contexts
            .values()
            .map(|ctx| ctx.active_proposals_count())
            .sum();

        Ok(borsh::to_vec(&count)?)
    }

    fn handle_get_number_of_proposal_approvals(
        state: &MockState,
        payload: &[u8],
    ) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            proposal_id: Repr<calimero_context_config::types::ProposalId>,
        }

        let request: Request = borsh::from_slice(payload)?;

        // Search for the proposal in all contexts
        for context in state.contexts.values() {
            if context.proposals.contains_key(&request.proposal_id) {
                let num_approvals = context.get_approvals(&request.proposal_id);
                let result = ProposalWithApprovals {
                    proposal_id: request.proposal_id,
                    num_approvals,
                };
                // ProposalWithApprovals uses serde, not borsh
                return Ok(serde_json::to_vec(&result)?);
            }
        }

        Err(eyre!("Proposal not found"))
    }

    fn handle_get_proposal_approvers(state: &MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            proposal_id: Repr<calimero_context_config::types::ProposalId>,
        }

        let request: Request = borsh::from_slice(payload)?;

        // Search for the proposal in all contexts
        for context in state.contexts.values() {
            if context.proposals.contains_key(&request.proposal_id) {
                let approvers = context.get_approvers(&request.proposal_id);
                return Ok(borsh::to_vec(&approvers)?);
            }
        }

        Ok(borsh::to_vec(&Vec::<ContextIdentity>::new())?)
    }

    fn handle_get_context_value(state: &MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            key: Vec<u8>,
        }

        let request: Request = borsh::from_slice(payload)?;

        // In mock mode, we aggregate storage from all contexts
        // In production, this would be context-specific
        for context in state.contexts.values() {
            if let Some(value) = context.storage.get(&request.key) {
                return Ok(borsh::to_vec(value)?);
            }
        }

        // Return empty vec if not found
        Ok(borsh::to_vec(&Vec::<u8>::new())?)
    }

    fn handle_get_context_storage_entries(
        state: &MockState,
        payload: &[u8],
    ) -> EyreResult<Vec<u8>> {
        #[derive(BorshDeserialize)]
        struct Request {
            offset: usize,
            limit: usize,
        }

        let request: Request = borsh::from_slice(payload)?;

        // Aggregate storage from all contexts
        let mut all_entries = Vec::new();
        for context in state.contexts.values() {
            all_entries.extend(context.get_storage_entries(0, usize::MAX));
        }

        let entries: Vec<ContextStorageEntry> = all_entries
            .into_iter()
            .skip(request.offset)
            .take(request.limit)
            .collect();

        Ok(borsh::to_vec(&entries)?)
    }

    // Proxy mutate handlers

    fn handle_proxy_mutate(state: &mut MockState, payload: &[u8]) -> EyreResult<Vec<u8>> {
        // Decode the proxy mutate request
        let request_json: serde_json::Value = serde_json::from_slice(payload)?;

        // Skip signature verification in mock mode
        let proxy_request: ProxyMutateRequest = serde_json::from_value(request_json)?;

        match proxy_request {
            ProxyMutateRequest::Propose { proposal } => {
                let proposal_id = proposal.id;
                Self::handle_propose(state, proposal)?;
                let result = Some(ProposalWithApprovals {
                    proposal_id,
                    num_approvals: 0,
                });
                // ProposalWithApprovals uses serde, not borsh
                Ok(serde_json::to_vec(&result)?)
            }
            ProxyMutateRequest::Approve { approval } => {
                Self::handle_approve(state, approval)?;
                // Count approvals after adding this one
                let num_approvals = state
                    .contexts
                    .values()
                    .find_map(|ctx| {
                        if ctx.proposals.contains_key(&approval.proposal_id) {
                            Some(ctx.get_approvals(&approval.proposal_id))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);

                let result = Some(ProposalWithApprovals {
                    proposal_id: approval.proposal_id,
                    num_approvals,
                });
                // ProposalWithApprovals uses serde, not borsh
                Ok(serde_json::to_vec(&result)?)
            }
        }
    }

    fn handle_propose(state: &mut MockState, proposal: Proposal) -> EyreResult<()> {
        // Store proposal in the first context (in mock mode we simplify this)
        // In production, proposals would be tied to a specific context
        if let Some(context) = state.contexts.values_mut().next() {
            context.proposals.insert(proposal.id, proposal.clone());
            context.approvals.insert(proposal.id, Vec::new());
            Ok(())
        } else {
            Err(eyre!("No context available for proposal"))
        }
    }

    fn handle_approve(
        state: &mut MockState,
        approval: ProposalApprovalWithSigner,
    ) -> EyreResult<()> {
        // Find the context containing this proposal
        for context in state.contexts.values_mut() {
            if context.proposals.contains_key(&approval.proposal_id) {
                context
                    .approvals
                    .entry(approval.proposal_id)
                    .or_insert_with(Vec::new)
                    .push(approval);
                return Ok(());
            }
        }

        Err(eyre!("Proposal not found"))
    }
}
