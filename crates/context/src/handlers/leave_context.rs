//! Local-only opt-out from a single context.
//!
//! See `architecture/membership-and-leave.html` § leave_context for the full
//! design. Summary:
//!
//! 1. Resolve the calling member's `(context_id, public_key)` pair.
//! 2. Delete the local `ContextIdentity` row, which stops sync (the sync layer
//!    iterates `ContextIdentity` rows when deciding what to replicate).
//! 3. Write a `ContextLeftMarker` tombstone in `Column::ContextLocal`. The
//!    auto-follow handler (`crate::auto_follow::has_left_context`) checks
//!    this marker before re-joining on a `ContextRegistered` event.
//!
//! No governance op is published. Peers never observe the leave. Reversal is
//! a regular `JoinContextRequest`, which clears the marker as a side effect.
//!
//! # Why a separate column instead of a flag on `ContextIdentity`?
//!
//! `ContextIdentity` is part of the synced membership shape; storing a
//! "node-local opt-out" on it would conflate replicated and node-local state
//! and force every receiver to ignore the field. A dedicated
//! `Column::ContextLocal` makes the node-local-ness explicit at the storage
//! layer.

use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{LeaveContextRequest, LeaveContextResponse};
use calimero_store::{key, types};
use eyre::{bail, eyre, WrapErr};
use tracing::{info, warn};

use crate::group_store;
use crate::ContextManager;

impl Handler<LeaveContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <LeaveContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        LeaveContextRequest { context_id }: LeaveContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let datastore = self.datastore.clone();

        ActorResponse::r#async(
            async move {
                // 1. Resolve the calling member's public key for the group owning
                //    this context. If the context isn't mapped to a group locally,
                //    there's nothing to leave — the node never joined.
                let group_id = group_store::get_group_for_context(&datastore, &context_id)?
                    .ok_or_else(|| {
                        eyre!(
                            "context {} is not mapped to any local group; \
                             nothing to leave on this node",
                            context_id
                        )
                    })?;

                let member_public_key =
                    match group_store::resolve_namespace_identity(&datastore, &group_id)? {
                        Some((pk, _, _)) => pk,
                        None => {
                            bail!(
                                "no identity stored for group {} on this node; \
                                 cannot leave context {}",
                                hex::encode(group_id.to_bytes()),
                                context_id
                            )
                        }
                    };

                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);

                // 2. Write the leave marker. We do this BEFORE deleting the
                //    identity row so that any concurrent auto-follow handler
                //    that reads the identity row first would still find the
                //    marker on its leave-check and skip re-joining.
                let marker_key = key::ContextLeftMarker::new(context_id, member_public_key);
                let marker_value = types::ContextLeftMarker { left_at_ms: now_ms };

                datastore
                    .handle()
                    .put(&marker_key, &marker_value)
                    .wrap_err("failed to write context-leave marker")?;

                // 3. Delete the identity row. With this row gone, sync stops
                //    replicating this context to/from this node.
                let identity_key = key::ContextIdentity::new(context_id, member_public_key);
                if let Err(err) = datastore.handle().delete(&identity_key) {
                    // The marker is already in place, so even if delete fails
                    // we won't auto-rejoin. Log and proceed; sync will cleanly
                    // ignore the row at most until a future flush.
                    warn!(
                        %context_id,
                        ?err,
                        "leave_context: marker written but identity-row delete failed; \
                         sync will retry on next opportunity"
                    );
                }

                info!(
                    %context_id,
                    %member_public_key,
                    "leave_context: opted out locally — sync stopped, auto-follow disarmed"
                );

                Ok(LeaveContextResponse {
                    context_id,
                    member_public_key,
                })
            }
            .into_actor(self),
        )
    }
}
