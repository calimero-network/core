//! Unit tests for mock relayer handlers

use std::borrow::Cow;

use borsh::BorshSerialize;
use calimero_context_config::client::transport::Operation;
use calimero_context_config::repr::Repr;
use calimero_context_config::types::{
    Application, ApplicationId, ApplicationMetadata, ApplicationSource, BlobId, Capability,
    ContextId, ContextIdentity, ProposalId, SignerId,
};
use calimero_context_config::{
    ContextRequest, ContextRequestKind, Proposal, ProposalAction, ProposalApprovalWithSigner,
    ProxyMutateRequest, RequestKind,
};

use super::handlers::MockHandlers;
use super::state::MockState;

/// Helper to convert ContextIdentity to SignerId (they have the same underlying [u8; 32] representation)
fn identity_to_signer_id(identity: &ContextIdentity) -> SignerId {
    // ContextIdentity is Copy, so we can dereference it
    unsafe { std::mem::transmute(*identity) }
}

/// Helper to create SignerId from bytes
fn bytes_to_signer_id(bytes: [u8; 32]) -> SignerId {
    unsafe { std::mem::transmute(bytes) }
}

fn create_test_application() -> Application<'static> {
    use calimero_context_config::repr::ReprBytes;

    let app_id = ApplicationId::from_bytes(|bytes| {
        *bytes = [1u8; 32];
        Ok(32)
    })
    .expect("valid application id");

    let blob_id = BlobId::from_bytes(|bytes| {
        *bytes = [2u8; 32];
        Ok(32)
    })
    .expect("valid blob id");

    Application::new(
        Repr::new(app_id),
        Repr::new(blob_id),
        1024,
        ApplicationSource(Cow::Owned("test-source".to_string())),
        ApplicationMetadata(Repr::new(Cow::Owned(vec![1, 2, 3]))),
    )
}

fn create_test_context_id() -> ContextId {
    ContextId::from([42u8; 32])
}

fn create_test_identity() -> ContextIdentity {
    ContextIdentity::from([100u8; 32])
}

#[test]
fn test_add_context() {
    let mut state = MockState::new();
    let context_id = create_test_context_id();
    let author_id = create_test_identity();
    let application = create_test_application();

    // Create add context request
    let request = RequestKind::Context(ContextRequest::new(
        Repr::new(context_id),
        ContextRequestKind::Add {
            author_id: Repr::new(author_id),
            application: application.clone(),
        },
    ));

    let payload = serde_json::to_vec(&request).unwrap();
    let operation = Operation::Write {
        method: Cow::Borrowed("mutate"),
    };

    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
    assert!(result.is_ok(), "Failed to add context: {result:?}");

    // Verify context was added
    assert!(state.has_context(&context_id));
    let context = state.get_context(&context_id).unwrap();
    assert_eq!(context.application.id, application.id);
    assert!(context.members.contains(&author_id));
}

#[test]
fn test_query_application() {
    let mut state = MockState::new();
    let context_id = create_test_context_id();
    let author_id = create_test_identity();
    let application = create_test_application();

    // Add context first
    state.add_context(context_id, application.clone(), author_id);

    // Query application
    #[derive(BorshSerialize)]
    struct Request {
        context_id: Repr<ContextId>,
    }

    let request = Request {
        context_id: Repr::new(context_id),
    };
    let payload = borsh::to_vec(&request).unwrap();
    let operation = Operation::Read {
        method: Cow::Borrowed("application"),
    };

    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
    assert!(result.is_ok(), "Failed to query application: {result:?}");

    let response: Application = borsh::from_slice(&result.unwrap()).unwrap();
    assert_eq!(response.id, application.id);
}

#[test]
fn test_add_members() {
    let mut state = MockState::new();
    let context_id = create_test_context_id();
    let author_id = create_test_identity();
    let application = create_test_application();

    // Add context first
    state.add_context(context_id, application, author_id);

    // Add new members
    let new_member1 = ContextIdentity::from([101u8; 32]);
    let new_member2 = ContextIdentity::from([102u8; 32]);

    let request = RequestKind::Context(ContextRequest::new(
        Repr::new(context_id),
        ContextRequestKind::AddMembers {
            members: Cow::Owned(vec![Repr::new(new_member1), Repr::new(new_member2)]),
        },
    ));

    let payload = serde_json::to_vec(&request).unwrap();
    let operation = Operation::Write {
        method: Cow::Borrowed("mutate"),
    };

    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
    assert!(result.is_ok(), "Failed to add members: {result:?}");

    // Verify members were added
    let context = state.get_context(&context_id).unwrap();
    assert!(context.members.contains(&new_member1));
    assert!(context.members.contains(&new_member2));
    assert_eq!(context.members_revision, 2); // Should be incremented
}

#[test]
fn test_query_members() {
    let mut state = MockState::new();
    let context_id = create_test_context_id();
    let author_id = create_test_identity();
    let application = create_test_application();

    state.add_context(context_id, application, author_id);

    // Query members
    #[derive(BorshSerialize)]
    struct Request {
        context_id: Repr<ContextId>,
        offset: usize,
        length: usize,
    }

    let request = Request {
        context_id: Repr::new(context_id),
        offset: 0,
        length: 10,
    };
    let payload = borsh::to_vec(&request).unwrap();
    let operation = Operation::Read {
        method: Cow::Borrowed("members"),
    };

    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
    assert!(result.is_ok(), "Failed to query members: {result:?}");

    let members: Vec<ContextIdentity> = borsh::from_slice(&result.unwrap()).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0], author_id);
}

#[test]
fn test_fetch_nonce() {
    let mut state = MockState::new();
    let context_id = create_test_context_id();
    let member_id = create_test_identity();
    let application = create_test_application();

    state.add_context(context_id, application, member_id);

    // Fetch nonce
    #[derive(BorshSerialize)]
    struct Request {
        context_id: Repr<ContextId>,
        member_id: Repr<ContextIdentity>,
    }

    let request = Request {
        context_id: Repr::new(context_id),
        member_id: Repr::new(member_id),
    };
    let payload = borsh::to_vec(&request).unwrap();
    let operation = Operation::Read {
        method: Cow::Borrowed("fetch_nonce"),
    };

    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
    assert!(result.is_ok(), "Failed to fetch nonce: {result:?}");

    let nonce: Option<u64> = borsh::from_slice(&result.unwrap()).unwrap();
    assert_eq!(nonce, Some(0));

    // Increment nonce
    state.increment_nonce(&context_id, &member_id);

    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
    let nonce: Option<u64> = borsh::from_slice(&result.unwrap()).unwrap();
    assert_eq!(nonce, Some(1));
}

#[test]
fn test_grant_capabilities() {
    let mut state = MockState::new();
    let context_id = create_test_context_id();
    let author_id = create_test_identity();
    let application = create_test_application();

    state.add_context(context_id, application, author_id);

    let member = ContextIdentity::from([150u8; 32]);
    let request = RequestKind::Context(ContextRequest::new(
        Repr::new(context_id),
        ContextRequestKind::Grant {
            capabilities: Cow::Owned(vec![
                (Repr::new(member), Capability::ManageMembers),
                (Repr::new(member), Capability::Proxy),
            ]),
        },
    ));

    let payload = serde_json::to_vec(&request).unwrap();
    let operation = Operation::Write {
        method: Cow::Borrowed("mutate"),
    };

    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
    assert!(result.is_ok(), "Failed to grant capabilities: {result:?}");

    // Verify capabilities were granted
    let context = state.get_context(&context_id).unwrap();
    let signer_id = identity_to_signer_id(&member);
    let caps = context.capabilities.get(&signer_id).unwrap();
    assert_eq!(caps.len(), 2);
    assert!(caps.contains(&Capability::ManageMembers));
    assert!(caps.contains(&Capability::Proxy));
}

#[test]
fn test_proxy_proposal() {
    use calimero_context_config::repr::ReprBytes;

    let mut state = MockState::new();
    let context_id = create_test_context_id();
    let author_id = create_test_identity();
    let application = create_test_application();

    state.add_context(context_id, application, author_id);

    let proposal_id = ProposalId::from_bytes(|bytes| {
        *bytes = [200u8; 32];
        Ok(32)
    })
    .expect("valid proposal id");
    let signer_id = identity_to_signer_id(&author_id);

    let proposal = Proposal {
        id: Repr::new(proposal_id),
        author_id: Repr::new(signer_id),
        actions: vec![ProposalAction::SetContextValue {
            key: vec![1, 2, 3].into_boxed_slice(),
            value: vec![4, 5, 6].into_boxed_slice(),
        }],
    };

    let request = ProxyMutateRequest::Propose {
        proposal: proposal.clone(),
    };

    let payload = serde_json::to_vec(&request).unwrap();
    let operation = Operation::Write {
        method: Cow::Borrowed("proxy_mutate"),
    };

    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
    assert!(result.is_ok(), "Failed to create proposal: {result:?}");

    // Verify proposal was created
    let context = state.get_context(&context_id).unwrap();
    assert!(context.proposals.contains_key(&proposal.id));
}

#[test]
fn test_proxy_approve() {
    use calimero_context_config::repr::ReprBytes;

    let mut state = MockState::new();
    let context_id = create_test_context_id();
    let author_id = create_test_identity();
    let application = create_test_application();

    state.add_context(context_id, application, author_id);

    // Create a proposal first
    let proposal_id = ProposalId::from_bytes(|bytes| {
        *bytes = [200u8; 32];
        Ok(32)
    })
    .expect("valid proposal id");
    let signer_id = identity_to_signer_id(&author_id);

    let proposal = Proposal {
        id: Repr::new(proposal_id),
        author_id: Repr::new(signer_id),
        actions: vec![],
    };

    let create_request = ProxyMutateRequest::Propose {
        proposal: proposal.clone(),
    };
    let create_payload = serde_json::to_vec(&create_request).unwrap();
    let operation = Operation::Write {
        method: Cow::Borrowed("proxy_mutate"),
    };

    MockHandlers::handle_operation(&mut state, &operation, &create_payload).unwrap();

    // Now approve the proposal
    let approver_id = bytes_to_signer_id([201u8; 32]);
    let approval = ProposalApprovalWithSigner {
        proposal_id: Repr::new(proposal_id),
        signer_id: Repr::new(approver_id),
        added_timestamp: 1234567890,
    };

    let approve_request = ProxyMutateRequest::Approve { approval };
    let approve_payload = serde_json::to_vec(&approve_request).unwrap();

    let result = MockHandlers::handle_operation(&mut state, &operation, &approve_payload);
    assert!(result.is_ok(), "Failed to approve proposal: {result:?}");

    // Verify approval was recorded
    let context = state.get_context(&context_id).unwrap();
    let approvals = context.approvals.get(&proposal.id).unwrap();
    assert_eq!(approvals.len(), 1);
}

#[test]
fn test_get_proxy_contract() {
    let mut state = MockState::new();
    let context_id = create_test_context_id();
    let author_id = create_test_identity();
    let application = create_test_application();

    state.add_context(context_id, application, author_id);

    #[derive(BorshSerialize)]
    struct Request {
        context_id: Repr<ContextId>,
    }

    let request = Request {
        context_id: Repr::new(context_id),
    };
    let payload = borsh::to_vec(&request).unwrap();
    let operation = Operation::Read {
        method: Cow::Borrowed("get_proxy_contract"),
    };

    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
    assert!(result.is_ok(), "Failed to get proxy contract: {result:?}");

    let proxy_contract_id: String = borsh::from_slice(&result.unwrap()).unwrap();
    assert!(proxy_contract_id.starts_with("mock-proxy-"));
    // Verify it's deterministic
    assert_eq!(
        proxy_contract_id,
        state.get_context(&context_id).unwrap().proxy_contract_id
    );
}

#[test]
fn test_context_not_found() {
    let mut state = MockState::new();
    let context_id = create_test_context_id();

    #[derive(BorshSerialize)]
    struct Request {
        context_id: Repr<ContextId>,
    }

    let request = Request {
        context_id: Repr::new(context_id),
    };
    let payload = borsh::to_vec(&request).unwrap();
    let operation = Operation::Read {
        method: Cow::Borrowed("application"),
    };

    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Context not found"));
}
