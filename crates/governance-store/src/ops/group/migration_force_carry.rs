//! `GroupOp::MigrationForceCarry` apply handler (PR-6c, task 6c.5).
//!
//! Admin force-carry of a departed owner's stale identity-gated entry.
//! When an identity-gated (`User`/`Shared`) entry's owner has departed
//! the namespace, no one can re-sign it as that owner (the migration's
//! owner-driven rewrite path can never run for them) and the admin
//! cannot forge the departed owner's signature nor change an entry's
//! owner — `verify_action_update` rejects both. The only crypto-sound
//! resolution is a **tombstone + rekey**: delete the stale entry and
//! re-create a fresh entity stamped under the ADMIN's OWN key, which
//! the admin signs normally so it verifies at `apply_action` with no
//! new crypto.
//!
//! This governance handler's job is **authorization + recording**, not
//! reaching across into the app's CRDT storage layer (`GroupApplyCtx`
//! only borrows the governance `Store`). It:
//!   1. requires the signer to hold the namespace admin/owner
//!      capability (reusing the same `require_admin` gate `MemberRemoved`
//!      uses), rejecting non-admins before recording anything; and
//!   2. records the force-carry intent by broadcasting
//!      `OpEvent::MigrationForceCarried`, carrying the admin signer as
//!      the new owner.
//!
//! The actual storage delete+create is performed downstream by the
//! `OpEvent::MigrationForceCarried` subscriber on the storage path: a
//! normal admin-signed `Action::Delete(entry_id)` +
//! `Action::Add(new, owner=admin)` sequence that verifies normally at
//! `apply_action`. The governance op never bypasses storage
//! verification and never re-signs as `departed_owner` (carried for
//! audit only).

use super::context::GroupApplyCtx;
use crate::{get_group_for_context, ContextRegistrationError};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    context_id: &[u8; 32],
    entry_id: &[u8; 32],
    departed_owner: &PublicKey,
    target_schema_version: u32,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Authorization: only the namespace admin/owner may force-carry.
    // Reuses the exact gate `MemberRemoved` uses; rejects non-admins
    // with `MembershipError::NotAdmin` before any intent is recorded.
    ctx.permissions().require_admin(signer)?;

    // Context-scoping: this op is an authorization + recording component,
    // so it must bind `context_id` to the op's group. Without this guard
    // an admin of group A could name a `context_id` belonging to a
    // different group B (which they do not administer) — `require_admin`
    // passes on A, and we'd broadcast a tombstone+rekey intent for B's
    // entry. The upstream apply path does not bind context to group; that
    // responsibility lives per-handler. Mirrors `context_metadata_set`.
    if get_group_for_context(store, &ContextId::from(*context_id))? != Some(*group_id) {
        bail!(ContextRegistrationError::NotInGroup {
            group_id: hex::encode(group_id.to_bytes()),
            context_id: format!("{context_id:?}"),
        });
    }

    // Record intent. The new entity is owned by the admin SIGNER — never
    // `departed_owner` — so the downstream storage tombstone+rekey emits
    // an admin-signed `Action::Add` that verifies at `apply_action`
    // (no owner-forge, no owner-change).
    crate::op_events::notify(crate::op_events::OpEvent::MigrationForceCarried {
        group_id: group_id.to_bytes(),
        context_id: *context_id,
        entry_id: *entry_id,
        departed_owner: *departed_owner,
        new_owner: *signer,
        target_schema_version,
    });

    tracing::info!(
        target: "calimero::migration",
        group_id = %hex::encode(group_id.to_bytes()),
        context_id = %hex::encode(context_id),
        entry_id = %hex::encode(entry_id),
        departed_owner = %departed_owner,
        new_owner = %signer,
        target_schema_version,
        "MigrationForceCarry: admin authorized; recorded tombstone+rekey intent"
    );

    Ok(())
}
