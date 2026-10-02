//! A member's signed state (`BroadcastMessage::StateBeacon`): what tells
//! tombstone GC how far that member has caught up with this node.
//!
//! The beacon counts only once it verifies under the key it names and that key
//! is a device the sync admission accepts for the context, the same set GC
//! waits on ([`crate::tombstone_stability::other_member_devices`]). A beacon
//! from anyone else could release tombstones a stale replica still needs.

use calimero_node_primitives::sync::delta_auth::verify_state_beacon;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use tracing::debug;

use crate::tombstone_stability::{beacon_heads_fit, now_nanos};
use crate::NodeManager;

pub(super) fn handle_state_beacon(
    manager: &NodeManager,
    source: libp2p::PeerId,
    context_id: ContextId,
    signer: PublicKey,
    root_hash: Hash,
    dag_heads: &[[u8; 32]],
    signature: &[u8; 64],
) {
    if !beacon_heads_fit(dag_heads) {
        debug!(%context_id, %source, %signer, heads = dag_heads.len(), "dropping a state beacon whose heads are unsorted or too many");
        return;
    }
    if let Err(err) = verify_state_beacon(context_id, signer, *root_hash, dag_heads, signature) {
        debug!(%context_id, %source, %signer, %err, "dropping a state beacon that does not verify");
        return;
    }
    let context_client = &manager.clients.context;
    let store = context_client.datastore();
    let admitted = match calimero_governance_store::is_admitted_to_context(
        store,
        &context_id,
        &signer,
    ) {
        Ok(Some(admitted)) => admitted,
        // A context in no group: its own member set decides, as the sync
        // admission's does.
        Ok(None) => context_client
            .has_member(&context_id, &signer, None)
            .unwrap_or(false),
        Err(err) => {
            debug!(%context_id, %signer, %err, "dropping a state beacon: membership unreadable");
            return;
        }
    };
    if !admitted {
        debug!(%context_id, %source, %signer, "dropping a state beacon from a non-member");
        return;
    }

    let stability = &manager.state.tombstone_stability;
    // Record this node's state as of now, so a beacon equal to it matches even
    // between the heartbeats that record it otherwise.
    if let Ok(Some(context)) = context_client.get_context(&context_id) {
        stability.record_own(
            context_id,
            context.dag_heads.clone(),
            *context.root_hash,
            now_nanos(),
        );
    }
    let _matched = stability.observe(context_id, signer, dag_heads, *root_hash);
}
