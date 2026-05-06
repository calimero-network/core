//! Local-only opt-out from a single context.
//!
//! See `architecture/membership-and-leave.html` § 4 for the design.
//!
//! Mechanism:
//!   1. Look up the context's local signing identity by scanning
//!      `ContextIdentity` rows for `(context_id, *)` and finding the one
//!      where this node holds the private key. This handles cases where
//!      a context was created with a per-context identity that differs
//!      from the namespace-level identity (e.g. via
//!      `CreateContextRequest.identity_secret`).
//!   2. In a single batched store handle, write the `ContextLeftMarker`
//!      tombstone in `Column::ContextLocal` AND delete the
//!      `ContextIdentity` row. The auto-follow handler
//!      (`crate::auto_follow::has_left_context`) checks the marker
//!      before re-joining; the deleted identity row stops sync.
//!   3. Unsubscribe from the context's gossipsub topic so the node
//!      stops receiving traffic for it.
//!
//! No governance op is published. Peers never observe the leave.
//! Reversal is a regular `JoinContextRequest`, which clears the marker
//! as a side effect.
//!
//! # Why a separate column instead of a flag on `ContextIdentity`?
//!
//! `ContextIdentity` is part of the synced membership shape; storing a
//! "node-local opt-out" on it would conflate replicated and node-local
//! state. A dedicated `Column::ContextLocal` makes the node-local-ness
//! explicit at the storage layer. (Reviewed-and-decided choice — the
//! design doc § 4 reflects this.)

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
        let node_client = self.node_client.clone();

        ActorResponse::r#async(
            async move {
                // Find this node's actual context-specific identity by scanning
                // the `ContextIdentity` column for a row keyed by this context
                // where we hold the private key. Falls back to the namespace
                // identity only when no such row exists (e.g. inherited
                // membership we've never explicitly joined).
                //
                // This is the right resolution because `ContextIdentity` rows
                // can be keyed by an identity that differs from the
                // namespace-level pk (`CreateContextRequest.identity_secret`
                // can mint a per-context identity on creation). Resolving
                // through `find_local_signing_identity` deletes the row that
                // actually exists on disk.
                let member_public_key = match group_store::find_local_signing_identity(
                    &datastore,
                    &context_id,
                )? {
                    Some(pk) => pk,
                    None => {
                        // No ContextIdentity row exists locally — the node
                        // never joined or was already cleaned up. Fall back
                        // to the namespace identity for the marker so a
                        // future auto-follow event still finds an opt-out
                        // record under our identity. If even that fails,
                        // there's nothing for us to leave.
                        let group_id =
                            group_store::get_group_for_context(&datastore, &context_id)?
                                .ok_or_else(|| {
                                    eyre!(
                                        "context {} is not mapped to any local group; \
                                         nothing to leave on this node",
                                        context_id
                                    )
                                })?;
                        match group_store::resolve_namespace_identity(&datastore, &group_id)? {
                            Some((pk, _, _)) => pk,
                            None => bail!(
                                "no local identity for context {}; nothing to leave",
                                context_id
                            ),
                        }
                    }
                };

                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .wrap_err("system clock is before UNIX_EPOCH")?
                    .as_millis() as u64;

                let marker_key = key::ContextLeftMarker::new(context_id, member_public_key);
                let marker_value = types::ContextLeftMarker { left_at_ms: now_ms };
                let identity_key = key::ContextIdentity::new(context_id, member_public_key);

                // Write the marker AND delete the identity row through the
                // same handle so they land in the same batched commit. This
                // closes the window where a crash between the two writes
                // would leave a dangling marker (or worse, no marker but
                // the identity row already gone — which would let
                // auto-follow re-add freely on next event).
                {
                    let mut handle = datastore.handle();
                    handle
                        .put(&marker_key, &marker_value)
                        .wrap_err("failed to write context-leave marker")?;
                    handle
                        .delete(&identity_key)
                        .wrap_err("failed to delete context identity row")?;
                }

                // Stop receiving gossipsub traffic for this context. The
                // inverse of `node_client.subscribe(context_id)` that
                // `join_context` calls. If the unsubscribe fails the node
                // is in a slightly leaky state (still receiving messages)
                // but the membership is already gone — log and proceed.
                if let Err(err) = node_client.unsubscribe(&context_id).await {
                    warn!(
                        %context_id,
                        ?err,
                        "leave_context: unsubscribe from gossipsub failed; \
                         node may still receive context messages until restart"
                    );
                }

                info!(
                    %context_id,
                    %member_public_key,
                    "leave_context: opted out locally — sync stopped, auto-follow disarmed, gossipsub unsubscribed"
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
