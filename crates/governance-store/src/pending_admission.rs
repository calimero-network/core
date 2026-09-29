//! Whether a namespace op that cannot apply yet may wait for its parents.
//!
//! An op parks in the governance DAG's pending buffer before any authority gate
//! has run, because those gates need the op's causal cut and the cut is exactly
//! what is missing. What can be asked without it is whether the signing key is
//! one this namespace has ever certified, or whether the op is the kind that
//! introduces its own signer. Both are answered from this node's own rows, so a
//! key nobody vouched for cannot occupy buffer space.

use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use eyre::Result as EyreResult;

use crate::AccountBindingRepository;

/// Why a namespace op may wait in the pending buffer, or that it may not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingStanding {
    /// The signing key has been certified in this namespace at some point.
    /// Removal and revocation do not change this: a key that legitimately
    /// signed earlier still has ops that arrive out of order.
    Certified,
    /// The key is unknown here, but the op is the kind that carries the
    /// credential introducing its signer, so it can be judged once its parents
    /// arrive.
    Introducing,
    /// Neither: nothing local vouches for the signing key.
    Unknown,
}

/// Classify `op` for the pending buffer.
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
    Ok(if introduces_its_signer(&op.op) {
        PendingStanding::Introducing
    } else {
        PendingStanding::Unknown
    })
}

/// The ops whose signer is not yet bound when they are published: a joiner
/// signs its own join, and the founder signs the genesis. Each carries the
/// credential that binds the key.
fn introduces_its_signer(op: &NamespaceOp) -> bool {
    matches!(
        op,
        NamespaceOp::Root(
            RootOp::MemberJoined { .. }
                | RootOp::MemberJoinedAt { .. }
                | RootOp::NamespaceCreatedV2 { .. }
        ) | NamespaceOp::RootSealedForGroup { .. }
    )
}
