//! The half of the root guard that needs a cut: whose proof it is, whether its
//! counter is current, and whether its chain reaches the epoch in force. The
//! proof's own validity is checked where the payload is built
//! (`calimero-op-adapter`), so these views take it as given.

use calimero_account::{AccountGenesis, RootKeyHandoff};
use calimero_context_config::types::ContextGroupId;
use calimero_op::OpPayload;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;

use super::support::{bind_account, op_with};
use crate::view::{AccountBinding, AclView};
use crate::{authorize, Rejected};

const GROUP: [u8; 32] = [0x5A; 32];

fn root() -> PrivateKey {
    PrivateKey::from([0x21; 32])
}

fn guarded(carried: OpPayload, counter: u64, chain: Vec<RootKeyHandoff>) -> OpPayload {
    let genesis = AccountGenesis::new(root().public_key());
    OpPayload::RootGuarded {
        carried: Box::new(carried),
        group: ContextGroupId::from(GROUP),
        account: genesis.account_id(),
        counter,
        genesis,
        chain,
    }
}

/// The owner (the scope's root admin, whose root signed the proof).
fn view() -> AclView {
    let owner = AccountGenesis::new(root().public_key()).account_id();
    bind_account(
        AclView {
            root_admin: Some(owner),
            ..Default::default()
        },
        owner,
    )
}

fn owner() -> calimero_account::AccountId {
    AccountGenesis::new(root().public_key()).account_id()
}

#[test]
fn a_bare_owner_level_payload_is_refused_even_for_the_owner() {
    let view = view();
    for payload in [
        OpPayload::AdminChanged { new_admin: owner() },
        OpPayload::TeeAuthoringPolicySet {
            group: ContextGroupId::from(GROUP),
            allowed_mrtd: vec![],
        },
    ] {
        assert_eq!(
            authorize(&op_with(owner(), payload), &view),
            Err(Rejected::RootProofRequired)
        );
    }
}

#[test]
fn a_guarded_payload_with_the_authors_proof_at_the_current_counter_is_authorized() {
    let view = view();
    let op = op_with(
        owner(),
        guarded(OpPayload::AdminChanged { new_admin: owner() }, 0, vec![]),
    );
    assert_eq!(authorize(&op, &view), Ok(()));
}

#[test]
fn a_proof_from_another_account_is_refused() {
    let stranger = calimero_account::AccountId::from([0x44; 32]);
    let view = bind_account(view(), stranger);
    let op = op_with(
        stranger,
        guarded(
            OpPayload::AdminChanged {
                new_admin: stranger,
            },
            0,
            vec![],
        ),
    );
    assert!(matches!(
        authorize(&op, &view),
        Err(Rejected::RootProofNotTheAuthors { .. })
    ));
}

#[test]
fn a_spent_counter_is_refused() {
    let mut view = view();
    let _ = view.owner_op_counts.insert(ContextGroupId::from(GROUP), 1);
    let op = op_with(
        owner(),
        guarded(OpPayload::AdminChanged { new_admin: owner() }, 0, vec![]),
    );
    assert_eq!(
        authorize(&op, &view),
        Err(Rejected::OwnerOpCounterStale {
            expected: 1,
            found: 0
        })
    );
    let current = op_with(
        owner(),
        guarded(OpPayload::AdminChanged { new_admin: owner() }, 1, vec![]),
    );
    assert_eq!(authorize(&current, &view), Ok(()));
}

#[test]
fn a_chain_that_stops_below_the_resolved_epoch_is_refused() {
    let next = PrivateKey::from([0x22; 32]);
    let handoff = RootKeyHandoff::sign(&root(), owner(), 0, &next.public_key()).unwrap();
    let mut view = view();
    let _ = view.accounts.insert(
        owner(),
        AccountBinding {
            epoch: 1,
            root_pk: next.public_key(),
        },
    );
    let short = op_with(
        owner(),
        guarded(OpPayload::AdminChanged { new_admin: owner() }, 0, vec![]),
    );
    assert_eq!(
        authorize(&short, &view),
        Err(Rejected::RootProofBelowResolvedEpoch {
            account: owner(),
            epoch: 1
        })
    );
    let full = op_with(
        owner(),
        guarded(
            OpPayload::AdminChanged { new_admin: owner() },
            0,
            vec![handoff],
        ),
    );
    assert_eq!(authorize(&full, &view), Ok(()));
}

#[test]
fn the_guard_does_not_grant_the_authority_the_carried_op_needs() {
    // The author's own valid proof, but not the root admin: still refused.
    let mut view = view();
    view.root_admin = Some(calimero_account::AccountId::from([0x66; 32]));
    let op = op_with(
        owner(),
        guarded(OpPayload::AdminChanged { new_admin: owner() }, 0, vec![]),
    );
    assert_eq!(authorize(&op, &view), Err(Rejected::NotRootAdmin));

    // A TEE policy stays admin-level: a group admin with its own proof passes.
    let group = ContextGroupId::from(GROUP);
    let _ = view
        .groups
        .entry(group)
        .or_default()
        .insert(owner(), GroupMemberRole::Admin);
    let policy = op_with(
        owner(),
        guarded(
            OpPayload::TeeAuthoringPolicySet {
                group,
                allowed_mrtd: vec!["aa".to_owned()],
            },
            0,
            vec![],
        ),
    );
    assert_eq!(authorize(&policy, &view), Ok(()));
}
