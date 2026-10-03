//! Whether a namespace op that cannot apply yet may wait for its parents.
//!
//! The authority gates need the op's causal cut, which is what is missing, so
//! only local rows can be asked: has this namespace certified the signing key.

use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use eyre::Result as EyreResult;

use crate::AccountBindingRepository;

/// Why a namespace op may wait in the pending buffer, or that it may not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingStanding {
    /// The signing key has been certified in this namespace at some point.
    /// Removal and revocation keep the row, so out-of-order older ops still wait.
    Certified,
    /// The key is not certified here yet, but the op is a join, which carries
    /// the credential that certifies its own signer.
    Introducing,
    /// Neither. A key without a `signer_account` row also lands here: a member
    /// recorded without a binding cannot act (authority is resolved through
    /// live bindings), so its ops would be refused at apply anyway.
    Unknown,
}

/// Classify `op` for the pending buffer.
///
/// An op sealed under a namespace or relay envelope is judged by its outer
/// signer, so one from a key not yet certified is `Unknown` until the join that
/// certifies it has applied; ordering or a later sync covers that gap.
///
/// # Errors
/// Propagates the store read failure.
pub fn pending_standing(
    store: &calimero_store::Store,
    op: &SignedNamespaceOp,
) -> EyreResult<PendingStanding> {
    let namespace = ContextGroupId::from(op.namespace_id.to_bytes());
    if AccountBindingRepository::new(store)
        .signer_account(&namespace, &op.signer)?
        .is_some()
    {
        return Ok(PendingStanding::Certified);
    }
    Ok(if introduces_its_signer(&op.signer, &op.op) {
        PendingStanding::Introducing
    } else {
        PendingStanding::Unknown
    })
}

/// A joiner signs its own join, which carries the credential binding its key,
/// so a cleartext join counts only when that credential certifies the signer.
/// A group-sealed join cannot be read here. The genesis has no parents, so it
/// never waits and is not listed.
fn introduces_its_signer(signer: &PublicKey, op: &NamespaceOp) -> bool {
    match op {
        NamespaceOp::Root(
            RootOp::MemberJoined { account, .. } | RootOp::MemberJoinedAt { account, .. },
        ) => calimero_op_adapter::join_credential_certifies(signer, account),
        NamespaceOp::RootSealedForGroup { .. } => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_config::types::{
        GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };
    use calimero_governance_types::{EncryptedGroupOp, EncryptedRootOp};
    use calimero_primitives::identity::{PrivateKey, PublicKey};

    use super::*;
    use crate::test_fixtures::{account_for, enrol_member, real_join_account, test_store};
    use crate::AccountBindingRepository;

    const NAMESPACE: [u8; 32] = [0x81; 32];

    fn namespace() -> ContextGroupId {
        ContextGroupId::from(NAMESPACE)
    }

    fn invitation() -> SignedGroupOpenInvitation {
        SignedGroupOpenInvitation {
            invitation: GroupInvitationFromAdmin {
                inviter_identity: SignerId::from([0x82; 32]),
                group_id: namespace(),
                expiration_timestamp: 0,
                invitation_nonce: [0x83; 32],
                invited_role: 1,
                admitters: Vec::new(),
            },
            inviter_signature: String::new(),
            inviter_account: None,
            application_id: None,
            bytecode_id: None,
            admitter_addrs: Vec::new(),
        }
    }

    fn signed(signer: &PrivateKey, op: NamespaceOp) -> SignedNamespaceOp {
        SignedNamespaceOp::sign(signer, NAMESPACE.into(), vec![[0xEE; 32]], 1, op)
            .expect("sign a namespace op")
    }

    fn root_sealed() -> NamespaceOp {
        NamespaceOp::RootSealed {
            key_id: [0u8; 32].into(),
            encrypted: EncryptedRootOp {
                nonce: [0; 12],
                ciphertext: vec![1],
            },
        }
    }

    fn group_op() -> NamespaceOp {
        NamespaceOp::Group {
            group_id: namespace(),
            key_id: [0u8; 32].into(),
            encrypted: EncryptedGroupOp {
                nonce: [0; 12],
                ciphertext: vec![1],
            },
            key_rotation: None,
        }
    }

    fn policy_update() -> NamespaceOp {
        NamespaceOp::Root(RootOp::PolicyUpdated {
            policy_bytes: vec![1],
        })
    }

    fn join_at(key: &PublicKey) -> NamespaceOp {
        NamespaceOp::Root(RootOp::MemberJoinedAt {
            member: account_for(key),
            signed_invitation: invitation(),
            joined_at: 0,
            account: real_join_account(key),
        })
    }

    fn join(key: &PublicKey) -> NamespaceOp {
        NamespaceOp::Root(RootOp::MemberJoined {
            member: account_for(key),
            signed_invitation: invitation(),
            account: real_join_account(key),
        })
    }

    fn sealed_for_group() -> NamespaceOp {
        NamespaceOp::RootSealedForGroup {
            group_id: namespace(),
            key_id: [0u8; 32].into(),
            encrypted: EncryptedRootOp {
                nonce: [0; 12],
                ciphertext: vec![1],
            },
        }
    }

    #[test]
    fn a_join_from_an_unlisted_signer_is_introducing() {
        let store = test_store();
        let sk = PrivateKey::from([0x91; 32]);
        for op in [
            join(&sk.public_key()),
            join_at(&sk.public_key()),
            sealed_for_group(),
        ] {
            assert_eq!(
                pending_standing(&store, &signed(&sk, op)).expect("standing"),
                PendingStanding::Introducing
            );
        }
    }

    #[test]
    fn a_join_whose_credential_certifies_another_key_is_unknown() {
        let store = test_store();
        let sk = PrivateKey::from([0x96; 32]);
        let other = PrivateKey::from([0x97; 32]);
        for op in [join(&other.public_key()), join_at(&other.public_key())] {
            assert_eq!(
                pending_standing(&store, &signed(&sk, op)).expect("standing"),
                PendingStanding::Unknown
            );
        }
    }

    #[test]
    fn any_other_op_from_an_unlisted_signer_is_unknown() {
        let store = test_store();
        let sk = PrivateKey::from([0x92; 32]);
        for op in [root_sealed(), group_op(), policy_update()] {
            assert_eq!(
                pending_standing(&store, &signed(&sk, op)).expect("standing"),
                PendingStanding::Unknown
            );
        }
    }

    #[test]
    fn a_certified_signer_is_certified_for_every_op_kind() {
        let store = test_store();
        let sk = PrivateKey::from([0x93; 32]);
        let _account = enrol_member(&store, &namespace(), &sk.public_key());
        for op in [
            root_sealed(),
            group_op(),
            policy_update(),
            join(&sk.public_key()),
        ] {
            assert_eq!(
                pending_standing(&store, &signed(&sk, op)).expect("standing"),
                PendingStanding::Certified
            );
        }
    }

    #[test]
    fn a_revoked_signer_stays_certified() {
        let store = test_store();
        let sk = PrivateKey::from([0x94; 32]);
        let _account = enrol_member(&store, &namespace(), &sk.public_key());
        let bindings = AccountBindingRepository::new(&store);
        let device = bindings
            .binding_for_sign_pk(&namespace(), &sk.public_key())
            .expect("read the binding")
            .expect("the key is bound")
            .device;
        bindings
            .apply_revocation(&namespace(), device)
            .expect("revoke the device");

        assert_eq!(
            pending_standing(&store, &signed(&sk, policy_update())).expect("standing"),
            PendingStanding::Certified
        );
    }

    #[test]
    fn a_key_certified_in_another_namespace_is_not_certified_here() {
        let store = test_store();
        let sk = PrivateKey::from([0x95; 32]);
        let other = ContextGroupId::from([0x99; 32]);
        let _account = enrol_member(&store, &other, &sk.public_key());

        assert_eq!(
            pending_standing(&store, &signed(&sk, policy_update())).expect("standing"),
            PendingStanding::Unknown
        );
    }
}
