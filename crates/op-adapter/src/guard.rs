//! Owner-level ops: a `RootGuarded` wrapper → `OpPayload::RootGuarded`.
//!
//! The bare owner-level ops (`TransferOwnership`, `AdminChanged`,
//! `GroupDelete`, the TEE policy ops) fold to nothing. The live apply refuses
//! them since schema 20, so no replica holds one it applied, and a fold that
//! read one would hand ownership to whoever signed it with a stolen device.
//! Only the wrapped form folds, and only when its proof is internally valid for
//! the op it wraps, which is the op-local half of the guard (the same rule as a
//! device descope's statement, checked where the payload is built). The rest of
//! the guard needs a cut and is `calimero-authz`'s: see
//! [`OpPayload::RootGuarded`].

use calimero_account::{OwnerOpKind, SignedOwnerOp};
use calimero_context_config::types::ContextGroupId;
use calimero_op::OpPayload;

/// The `RootGuarded` payload for an op of `kind` whose borsh digest is
/// `digest`, carried as `carried`, or `None` if `proof` does not authorise it.
///
/// `group` is the group the op acts on: the envelope's group for a group op,
/// the namespace root for a root op.
pub(crate) fn guarded_payload(
    group: ContextGroupId,
    kind: OwnerOpKind,
    digest: [u8; 32],
    proof: &SignedOwnerOp,
    carried: OpPayload,
) -> Option<OpPayload> {
    let statement = &proof.statement;
    let authorises = statement.kind == kind
        && statement.op_digest == digest
        && statement.group_id == group.to_bytes()
        && proof.verify(statement.account).is_ok();
    authorises.then(|| OpPayload::RootGuarded {
        carried: Box::new(carried),
        group,
        account: statement.account,
        counter: statement.counter,
        genesis: proof.genesis,
        chain: proof.chain.clone(),
    })
}
