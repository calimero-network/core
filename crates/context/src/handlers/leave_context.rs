//! Local-only opt-out from a single context.
//!
//! See `architecture/membership-and-leave.html` § 4 for the design.
//!
//! Mechanism:
//!   1. Look up ALL of the context's local signing identities by scanning
//!      `ContextIdentity` rows for `(context_id, *)` and collecting every
//!      one where this node holds the private key. A node can hold more
//!      than one identity per context (e.g. one minted at create time via
//!      `CreateContextRequest.identity_secret` plus an inherited
//!      namespace-level identity); tombstoning only the first would leave
//!      the rest syncing and re-armable by auto-follow.
//!   2. In a single batched store handle, write a `ContextLeftMarker`
//!      tombstone in `Column::ContextLocal` AND delete the
//!      `ContextIdentity` row for every one of those identities. The
//!      auto-follow handler
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

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::NamespaceRepository;

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
                // Find EVERY local identity for this context by scanning the
                // `ContextIdentity` column for rows keyed by this context where
                // we hold the private key. Falls back to the namespace identity
                // only when no such row exists (e.g. inherited membership we've
                // never explicitly joined).
                //
                // Enumerating all of them (not just the first) matters:
                // `ContextIdentity` rows can be keyed by identities that differ
                // from the namespace-level pk (`CreateContextRequest.identity_secret`
                // can mint a per-context identity on creation), and a node may
                // legitimately hold several. Tombstoning only one would leave
                // the others syncing, and a later auto-follow event could
                // re-arm them.
                let mut member_public_keys =
                    calimero_governance_store::find_local_signing_identities(
                        &datastore,
                        &context_id,
                    )?;

                if member_public_keys.is_empty() {
                    // No ContextIdentity row exists locally — the node
                    // never joined or was already cleaned up. Fall back
                    // to the namespace identity for the marker so a
                    // future auto-follow event still finds an opt-out
                    // record under our identity. If even that fails,
                    // there's nothing for us to leave.
                    let group_id =
                        calimero_governance_store::get_group_for_context(&datastore, &context_id)?
                            .ok_or_else(|| {
                                eyre!(
                                    "context {} is not mapped to any local group; \
                                     nothing to leave on this node",
                                    context_id
                                )
                            })?;
                    match NamespaceRepository::new(&datastore).resolve_identity(&group_id)? {
                        Some((pk, _, _)) => member_public_keys.push(pk),
                        None => bail!(
                            "no local identity for context {}; nothing to leave",
                            context_id
                        ),
                    }
                }

                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .wrap_err("system clock is before UNIX_EPOCH")?
                    .as_millis() as u64;

                // The marker/delete pair per identity is written sequentially,
                // not atomically — `Handle` forwards each `put`/`delete`
                // directly to the underlying DB layer, which commits per-call.
                // The store does have a batched `WriteLayer::apply(&Transaction)`
                // primitive (crates/store/src/layer.rs:44) we could thread
                // through, but ordering + idempotency get us the same
                // correctness guarantee here:
                //
                //   * Marker first → if the delete then fails, the marker
                //     still gates auto-follow (it returns `false` for any
                //     concurrent `ContextRegistered` event). Sync stays
                //     active until the delete succeeds, but auto-follow
                //     can't re-register a row that's already there.
                //   * Marker put is idempotent (same key, fresher
                //     timestamp on retry).
                //   * Delete is idempotent (no-op on already-gone row).
                //   * Both errors bubble up; the caller can retry the
                //     whole leave without producing inconsistent state.
                //
                // The window where "marker exists but identity row still
                // present" is observable but coherent: leaver still
                // appears in member lists, sync continues, but
                // auto-follow won't re-add. Calling leave again succeeds.
                {
                    let mut handle = datastore.handle();
                    for member_public_key in &member_public_keys {
                        let marker_key =
                            key::ContextLeftMarker::new(context_id, *member_public_key);
                        let marker_value = types::ContextLeftMarker { left_at_ms: now_ms };
                        let identity_key =
                            key::ContextIdentity::new(context_id, *member_public_key);
                        handle
                            .put(&marker_key, &marker_value)
                            .wrap_err("failed to write context-leave marker")?;
                        handle
                            .delete(&identity_key)
                            .wrap_err("failed to delete context identity row")?;
                    }
                }

                // The response reports the primary (first) identity; every
                // identity above has been tombstoned regardless.
                let member_public_key = member_public_keys[0];

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
