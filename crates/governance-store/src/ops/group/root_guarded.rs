//! `GroupOp::RootGuarded` apply handler: an owner-level group op carrying the
//! account root's authorisation. See [`crate::owner_guard`] for what the proof
//! must show and why.

use calimero_account::SignedOwnerOp;
use calimero_context_client::local_governance::GroupOp;
use eyre::{bail, Result as EyreResult};

use super::context::GroupApplyCtx;
use crate::owner_guard::{advance_owner_op_counter, check_root_proof, GuardedOp};
use crate::{NamespaceRepository, OwnerGuardRefusal};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    inner: &GroupOp,
    proof: &SignedOwnerOp,
) -> EyreResult<()> {
    // The wrapper must carry a guarded kind. `owner_op_kind` is `None` for a
    // wrapper, so this also refuses one wrapper inside another.
    let Some(kind) = inner.owner_op_kind() else {
        bail!(OwnerGuardRefusal::NotAGuardedKind {
            inner: inner.op_kind_label(),
        });
    };
    // A relay never carries one of these (they are not delegable), so a
    // principal here is a wrapper that reached apply by some other door.
    if ctx.acting_principal().is_some() {
        bail!(OwnerGuardRefusal::NotAGuardedKind {
            inner: "a delegated op",
        });
    }
    let Some(account) = ctx.signer_account()? else {
        bail!(OwnerGuardRefusal::SignerUnbound);
    };

    let store = ctx.store();
    let group = *ctx.group_id();
    let guarded = GuardedOp {
        namespace: NamespaceRepository::new(store).resolve(&group)?,
        group,
        kind,
        digest: inner.owner_op_digest()?,
    };
    check_root_proof(store, account, guarded, proof)?;

    super::dispatch_guarded(ctx, inner)?;

    // Only after the op applied, so one that fails or parks for retry leaves its
    // proof usable. A deleted group keeps its counter row, which is what stops a
    // spent proof ever applying to a group recreated under the same id.
    advance_owner_op_counter(store, &group)
}
