//! Refusing a revoked device's state deltas on every delta path (core#4070).
//!
//! Every delta path except the gossip entry point authorizes the author at the
//! governance heads the author cites. A device that was offline when it was
//! revoked, and writes before it folds its own revocation, cites heads from
//! before it, so that check alone lets its write in. These paths ask
//! [`is_revoked_signer`] as well, and a delta it answers `true` for is refused
//! through [`refuse_delta`]: never applied here, and the deltas other authors
//! built on it apply without it rather than waiting for a parent this node will
//! only refuse again.
//!
//! The price is deliberate. A write the device genuinely made before it was
//! revoked, arriving after this node folded the revocation, is refused too: from
//! a cited cut alone the two cannot be told apart.
//!
//! **Record a refusal only after the envelope signature has verified.** A
//! delta's id content-addresses its parents, actions and HLC, not its author.
//! Recorded before verification, anyone could get an honest delta's id refused
//! by sending it under a revoked author's name. Even verified, a holder of a
//! revoked key can re-sign an honest delta's content, which is why the DAG does
//! not bar a refused id from applying later: the honest copy passes every path's
//! author check and still lands, so the most that trick costs is the delta's
//! children not waiting for it. The gossip entry and the governance-pending
//! drain check before any signature is verified, so they drop without
//! recording.

use calimero_context_client::client::ContextClient;
use calimero_governance_store::DenyListRepository;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use tracing::warn;

use super::events::execute_cascaded_events;
use super::store_setup::choose_owned_identity;
use crate::delta_store::DeltaStore;

/// Whether `author` signed for a device the namespace owning `context_id` has
/// revoked, with no live binding speaking for the key now.
///
/// A failed lookup answers `false`, and says so. A refusal is remembered by the
/// DAG, so failing closed would turn a transient store error into a permanent
/// refusal of a delta that may be honest; the at-cut check still runs.
pub(crate) fn is_revoked_signer(
    datastore: &calimero_store::Store,
    context_id: &ContextId,
    author: &PublicKey,
) -> bool {
    DenyListRepository::new(datastore)
        .is_revoked_signer_for_context(context_id, author)
        .unwrap_or_else(|err| {
            warn!(
                %context_id,
                %author,
                ?err,
                "revoked-signer lookup failed; leaving the delta to the at-cut check"
            );
            false
        })
}

/// Refuse `delta_id` in `delta_store` and run the event handlers of the deltas
/// the refusal released.
///
/// Call only once the delta's envelope signature has verified (see the module
/// docs).
///
/// `our_identity` is this node's identity in the context when the caller holds
/// it; otherwise one is chosen, and only if something was released.
pub(crate) async fn refuse_delta(
    delta_store: &DeltaStore,
    node_client: &NodeClient,
    context_client: &ContextClient,
    context_id: &ContextId,
    our_identity: Option<PublicKey>,
    delta_id: [u8; 32],
) {
    let released_events = delta_store.refuse_delta(delta_id).await;
    if released_events.is_empty() {
        return;
    }
    let our_identity = match our_identity {
        Some(identity) => identity,
        None => match choose_owned_identity(context_client, context_id).await {
            Ok(identity) => identity,
            Err(err) => {
                warn!(
                    %context_id,
                    delta_id = %Hash::from(delta_id),
                    %err,
                    "no owned identity to run released deltas' handlers; they replay on next init"
                );
                return;
            }
        },
    };
    // Same log-and-continue policy as every other cascade: the DAG has already
    // moved, so a handler failure must not unwind the caller.
    if let Err(err) = execute_cascaded_events(
        &released_events,
        node_client,
        context_client,
        context_id,
        &our_identity,
        "refused-parent release",
        None,
        delta_store,
    )
    .await
    {
        warn!(
            %context_id,
            delta_id = %Hash::from(delta_id),
            ?err,
            "handlers of deltas released by a refusal failed; events stay in DB for next init"
        );
    }
}
