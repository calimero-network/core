//! Admin/namespace plane: [`payload_from_root_op`] across every join shape,
//! the scope-tree ops, and genesis.

use calimero_account::AccountId;
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation,
};
use calimero_governance_types::RootOp;
use calimero_op::{OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;

use crate::payload_from_root_op;
use crate::tests::support::{real_join_account_for, test_join_account_for};

/// An admin-signed invitation for `group`, granting Admin (`invited_role: 0`).
fn invitation_for(group: [u8; 32]) -> SignedGroupOpenInvitation {
    SignedGroupOpenInvitation {
        inviter_account: None,
        invitation: GroupInvitationFromAdmin {
            inviter_identity: [0xA1; 32].into(),
            group_id: ContextGroupId::from(group),
            expiration_timestamp: 1_700_000_000,
            invitation_nonce: [0x33; 32],
            invited_role: 0, // Admin
            admitters: Vec::new(),
        },
        inviter_signature: "deadbeef".to_string(),
        application_id: None,
        bytecode_id: None,
        admitter_hints: Vec::new(),
    }
}

/// A credential that is not the joiner's folds like the apply path refuses it.
///
/// Both planes have to reach the same verdict. If the projection admitted a
/// lifted credential the apply path refuses, a replayed device would be bound
/// in the folded view and absent from the materialized rows — the same split
/// this encoder exists to close, running the other way.
#[test]
fn a_replayed_credential_folds_no_device_on_either_join_shape() {
    // The op names the account being admitted, while the credential beside
    // it certifies somebody ELSE's — the replay this test is about.
    let m = AccountId::from([7u8; 32]);
    let stranger = PublicKey::from([8u8; 32]);
    let gid = [0x44; 32];

    // Open self-join: back to the graph-only node, no device.
    assert_eq!(
        payload_from_root_op(&RootOp::MemberJoinedOpen {
            member: m,
            group_id: gid.into(),
            account: test_join_account_for(stranger),
        }),
        Some(OpPayload::Noop)
    );

    // Invitation join: the membership still stands, the device does not.
    assert_eq!(
        payload_from_root_op(&RootOp::MemberJoined {
            member: m,
            signed_invitation: invitation_for(gid),
            account: test_join_account_for(stranger),
        }),
        Some(OpPayload::MemberAdded {
            group: ContextGroupId::from(gid),
            member: m,
            role: GroupMemberRole::Admin,
        })
    );
}

#[test]
fn root_op_encoder_mapping() {
    let admin = AccountId::from([1u8; 32]);
    let m_key = PublicKey::from([0x55; 32]);
    let gid = [3u8; 32];

    assert_eq!(
        payload_from_root_op(&RootOp::AdminChanged { new_admin: admin }),
        Some(OpPayload::AdminChanged { new_admin: admin })
    );
    assert_eq!(
        payload_from_root_op(&RootOp::PolicyUpdated {
            policy_bytes: vec![1, 2, 3],
        }),
        Some(OpPayload::PolicyUpdated {
            policy_bytes: vec![1, 2, 3],
        })
    );
    // An open-subgroup self-join folds its CREDENTIAL and nothing else: the
    // membership is re-derived by the inheritance walk, so a direct row here
    // would outlive the anchor that grants it.
    let joined_open = real_join_account_for(m_key, 0x61);
    assert_eq!(
        payload_from_root_op(&RootOp::MemberJoinedOpen {
            member: joined_open.statement.account,
            group_id: gid.into(),
            account: joined_open.clone(),
        }),
        Some(OpPayload::DeviceLinked {
            genesis: joined_open.genesis,
            chain: joined_open.chain.clone(),
            cert: joined_open.statement,
        })
    );
    // Invitation-based join: group_id + role decoded off the admin-signed
    // invitation (invited_role 0 = Admin). The joiner can't escalate — the
    // role is under the admin's signature.
    let signed_invitation = invitation_for(gid);
    let invited = real_join_account_for(m_key, 0x62);
    assert_eq!(
        payload_from_root_op(&RootOp::MemberJoined {
            member: invited.statement.account,
            signed_invitation: signed_invitation.clone(),
            account: invited.clone(),
        }),
        Some(OpPayload::MemberJoinedWithDevice {
            group: ContextGroupId::from(gid),
            member: invited.statement.account,
            role: GroupMemberRole::Admin,
            genesis: invited.genesis,
            chain: invited.chain.clone(),
            cert: invited.statement,
        })
    );
    // `MemberJoinedAt` (the timestamped invitation join `join_group` emits)
    // decodes identically — it is NOT out-of-model.
    let invited_at = real_join_account_for(m_key, 0x63);
    assert_eq!(
        payload_from_root_op(&RootOp::MemberJoinedAt {
            member: invited_at.statement.account,
            signed_invitation,
            joined_at: 42,
            account: invited_at.clone(),
        }),
        Some(OpPayload::MemberJoinedWithDevice {
            group: ContextGroupId::from(gid),
            member: invited_at.statement.account,
            role: GroupMemberRole::Admin,
            genesis: invited_at.genesis,
            chain: invited_at.chain.clone(),
            cert: invited_at.statement,
        })
    );
    let parent = [0x70; 32]; // placeholder parent id
    assert_eq!(
        payload_from_root_op(&RootOp::GroupCreated {
            group_id: gid.into(),
            parent_id: parent.into(),
            restricted: true,
            admin: AccountId::from([0x5C; 32]),
        }),
        Some(OpPayload::SubgroupCreated {
            child: ScopeId::from(gid),
            parent: ScopeId::from(parent),
            restricted: true,
            // The account the OP carries, never one derived from the signer:
            // a derived id names no principal the account-keyed rows know, so
            // folding one puts two id spaces in one view and whichever the
            // resolver prefers, the other side mismatches.
            admin: AccountId::from([0x5C; 32]),
        })
    );
    // Scope-tree restructure ops now map to the structural OpPayload arms.
    assert_eq!(
        payload_from_root_op(&RootOp::GroupReparented {
            child_group_id: gid.into(),
            new_parent_id: [9u8; 32].into(),
        }),
        Some(OpPayload::SubgroupReparented {
            child: ScopeId::from(gid),
            new_parent: ScopeId::from([9u8; 32]),
        })
    );
    assert_eq!(
        payload_from_root_op(&RootOp::GroupDeleted {
            root_group_id: gid.into(),
            cascade_group_ids: vec![],
            cascade_context_ids: vec![],
        }),
        Some(OpPayload::SubgroupDeleted {
            scope: ScopeId::from(gid),
        })
    );
}

/// Genesis must fold the founder's DEVICE, not a Noop.
///
/// It is the only op that ever binds the founder — no join admits it — so if
/// this plane does not learn the link here it can never turn the founder's
/// signing key into the account the live rows are keyed by. The at-cut admin
/// check then answers "not admin" for the namespace's own founder, and every
/// receiver rejects ops the publisher accepted: a split that no later op
/// repairs. This regressed every multi-node group scenario at once.
#[test]
fn namespace_created_folds_the_founders_device_link() {
    let founder_pk = PublicKey::from([0x21u8; 32]);
    let credential = real_join_account_for(founder_pk, 0x21);
    let founder = credential.statement.account;

    assert_eq!(
        payload_from_root_op(&RootOp::NamespaceCreated {
            founder,
            account: credential.clone(),
        }),
        Some(OpPayload::DeviceLinked {
            genesis: credential.genesis,
            chain: credential.chain.clone(),
            cert: credential.statement,
        }),
        "genesis is the only place the founder's device is bound"
    );
}

/// A genesis whose credential is for somebody else binds nothing.
///
/// Same rule as the join arms: the credential has to speak for the principal
/// the op names, or the device half is not this founder's to record.
#[test]
fn namespace_created_with_a_foreign_credential_folds_no_device() {
    let founder_pk = PublicKey::from([0x22u8; 32]);
    let stranger = real_join_account_for(PublicKey::from([0x23u8; 32]), 0x23);

    assert_eq!(
        payload_from_root_op(&RootOp::NamespaceCreated {
            // The account the op names is NOT the one the credential
            // certifies, so the pair proves nothing.
            founder: real_join_account_for(founder_pk, 0x22).statement.account,
            account: stranger,
        }),
        Some(OpPayload::Noop)
    );
}
