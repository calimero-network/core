//! `RootOp::KeyDelivery` apply handler (#2481).
//!
//! The state-mutation arm is intentionally empty — `KeyDelivery`
//! side effects (storing the wrapped group key, seeding the
//! namespace admin row, retrying pending encrypted ops) live in
//! `NamespaceGovernance::apply_signed_op` after the dispatch
//! returns. That ordering is load-bearing: the side effects need
//! access to `self.decrypt_and_apply_group_op`, the retry-collect
//! service, and the outer apply pipeline's divergence outbox —
//! none of which belong on `NamespaceApplyCtx`.

use super::context::NamespaceApplyCtx;
use eyre::Result as EyreResult;

pub(crate) fn apply(_ctx: &NamespaceApplyCtx<'_>) -> EyreResult<()> {
    Ok(())
}
