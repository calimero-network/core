//! HashComparison sync protocol responder (CIP §2.3 Rules 3, 7).
//!
//! This module contains the responder side of the HashComparison protocol.
//! The initiator logic is in `hash_comparison_protocol.rs`.
//!
//! # Responder Flow
//!
//! ```text
//! Initiator                              Responder (this module)
//! │                                            │
//! │ ── TreeNodeRequest (root) ───────────────► │
//! │                                            │ handle_tree_node_request
//! │ ◄── TreeNodeResponse (children hashes) ─── │
//! │                                            │
//! │ ── TreeNodeRequest (child) ──────────────► │
//! │ ◄── TreeNodeResponse ─────────────────────│
//! │                                            │
//! │ ...repeat until initiator closes stream... │
//! └────────────────────────────────────────────┘
//! ```

use crate::sync::helpers::{handle_entity_delete_push_locked, handle_entity_push_locked};
use calimero_crypto::Nonce;
use calimero_node_primitives::sync::{
    create_runtime_env, InitPayload, MessagePayload, StreamMessage, SyncTransport,
    TreeNodeResponse, MAX_NODES_PER_RESPONSE,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::env::{with_runtime_env, RuntimeEnv};
use calimero_storage::index::Index;
use calimero_storage::store::MainStorage;
use eyre::Result;
use tracing::{debug, info, trace, warn};

use super::manager::SyncManager;

/// Maximum depth allowed in TreeNodeRequest.
///
/// Prevents malicious peers from requesting expensive deep traversals.
pub const MAX_REQUEST_DEPTH: u8 = 16;

use super::hash_comparison_protocol::MAX_HASH_COMPARISON_REQUESTS;

// =============================================================================
// SyncManager Responder Implementation
// =============================================================================

impl SyncManager {
    /// Handle incoming TreeNodeRequest from a peer.
    ///
    /// This is the responder side of HashComparison sync.
    /// Handles the first request (already parsed) and then loops to handle
    /// subsequent requests until the stream closes.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Context being synchronized
    /// * `first_node_id` - Node ID from the first request (already parsed)
    /// * `first_max_depth` - Max depth from the first request
    /// * `transport` - Transport for sending/receiving messages
    /// * `_nonce` - Reserved for future encrypted sync (currently unused as each
    ///   response generates its own nonce via `generate_nonce()`)
    pub async fn handle_tree_node_request<T: SyncTransport>(
        &self,
        context_id: ContextId,
        first_node_id: [u8; 32],
        first_max_depth: Option<u8>,
        transport: &mut T,
        _nonce: Nonce,
        // The authenticated identity of the initiator (the peer driving this
        // session). Used to gate ingestion of authorless plain-entity pushes:
        // a peer that is no longer an authorized member must not launder a
        // Public write into our store via HC. `None` when unresolvable.
        peer_identity: Option<PublicKey>,
    ) -> Result<()> {
        info!(%context_id, "Starting HashComparison responder");

        // Get our identity for RuntimeEnv - look up from context members
        let identities = self
            .context_client
            .get_context_members(&context_id, Some(true));

        let our_identity = match crate::utils::choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        {
            Some((identity, _)) => identity,
            None => {
                warn!(%context_id, "No owned identity for context, cannot respond to TreeNodeRequest");
                // Send not-found response
                let mut sqx = super::tracking::Sequencer::default();
                let msg = StreamMessage::Message {
                    sequence_id: sqx.next(),
                    payload: MessagePayload::TreeNodeResponse {
                        nodes: vec![],
                        not_found: true,
                    },
                    next_nonce: super::helpers::generate_nonce(),
                };
                transport.send(&msg).await?;
                return Ok(());
            }
        };

        let mut sqx = super::tracking::Sequencer::default();
        let mut requests_handled = 0u64;

        // Create RuntimeEnv once for all requests (optimization: avoids per-request allocation)
        let datastore = self.context_client.datastore_handle().into_inner();
        let runtime_env = create_runtime_env(&datastore, context_id, our_identity);

        // PR-6b Task 6b.7: the responder's loaded-reader schema, stamped onto
        // every leaf it emits so a peer on an older reader can decline+buffer a
        // future-schema leaf. `None` when unresolvable (no group / missing meta).
        let schema_app_key =
            calimero_context::hlc_fence::loaded_reader_app_key(&datastore, &context_id)
                .ok()
                .flatten();

        // Handle the first request (already parsed by handle_sync_request)
        {
            let clamped_depth = first_max_depth.map(|d| d.min(MAX_REQUEST_DEPTH));
            let response = self
                .build_tree_node_response(
                    context_id,
                    &first_node_id,
                    clamped_depth,
                    &runtime_env,
                    schema_app_key,
                )
                .await?;

            let msg = StreamMessage::Message {
                sequence_id: sqx.next(),
                payload: MessagePayload::TreeNodeResponse {
                    nodes: response.nodes,
                    not_found: response.not_found,
                },
                next_nonce: super::helpers::generate_nonce(),
            };
            transport.send(&msg).await?;
            requests_handled += 1;
        }

        // Loop to handle subsequent requests until stream closes
        loop {
            // DoS protection: limit total requests per session
            if requests_handled >= MAX_HASH_COMPARISON_REQUESTS {
                warn!(
                    %context_id,
                    requests_handled,
                    "Request limit reached, closing responder"
                );
                break;
            }

            let Some(request) = transport.recv().await? else {
                debug!(%context_id, requests_handled, "Stream closed, responder done");
                break;
            };

            let StreamMessage::Init { payload, .. } = request else {
                debug!(%context_id, "Received non-Init message, ending responder");
                break;
            };

            match payload {
                InitPayload::TreeNodeRequest {
                    node_id, max_depth, ..
                } => {
                    trace!(
                        %context_id,
                        node_id = %hex::encode(node_id),
                        ?max_depth,
                        "Handling subsequent TreeNodeRequest"
                    );

                    let clamped_depth = max_depth.map(|d| d.min(MAX_REQUEST_DEPTH));
                    let response = self
                        .build_tree_node_response(
                            context_id,
                            &node_id,
                            clamped_depth,
                            &runtime_env,
                            schema_app_key,
                        )
                        .await?;

                    let msg = StreamMessage::Message {
                        sequence_id: sqx.next(),
                        payload: MessagePayload::TreeNodeResponse {
                            nodes: response.nodes,
                            not_found: response.not_found,
                        },
                        next_nonce: super::helpers::generate_nonce(),
                    };
                    transport.send(&msg).await?;
                    requests_handled += 1;
                }

                InitPayload::EntityPush { entities, .. } => {
                    let entity_count = entities.len();
                    trace!(%context_id, entity_count, "Handling EntityPush from initiator");

                    // Apply under the per-context execution lock so this
                    // host-side merge can't interleave with a concurrent
                    // delta merge in the executor (torn-root split-brain).
                    let outcome = handle_entity_push_locked(
                        Some(&self.context_client),
                        &datastore,
                        &runtime_env,
                        context_id,
                        &entities,
                        peer_identity,
                    )
                    .await;
                    let applied = outcome.applied;

                    // Dispatch any deferred root-entity merges through
                    // the WASM module before ACKing the push — keeps
                    // the responder side symmetric with the initiator,
                    // so a bidirectional push of root state still
                    // converges. Identical helper to the one HC /
                    // LevelWise initiators use.
                    if !outcome.deferred_root_merges.is_empty() {
                        super::protocol_selector::dispatch_deferred_root_merges(
                            &self.context_client,
                            &datastore,
                            context_id,
                            our_identity,
                            &outcome.deferred_root_merges,
                        )
                        .await;
                    }

                    let msg = StreamMessage::Message {
                        sequence_id: sqx.next(),
                        payload: MessagePayload::EntityPushAck {
                            applied_count: applied,
                        },
                        next_nonce: super::helpers::generate_nonce(),
                    };
                    transport.send(&msg).await?;
                    requests_handled += 1;

                    info!(
                        %context_id,
                        applied,
                        deferred_root_merges = outcome.deferred_root_merges.len(),
                        total = entity_count,
                        "Applied pushed entities via CRDT merge"
                    );
                }

                InitPayload::EntityDeletePush { deletions, .. } => {
                    let total = deletions.len();
                    trace!(%context_id, total, "Handling EntityDeletePush from initiator");

                    // Apply the tombstones (delete-wins by HLC; signature/nonce
                    // verified for User/Shared) under the per-context execution
                    // lock, same split-brain guard as the EntityPush path.
                    let applied = handle_entity_delete_push_locked(
                        Some(&self.context_client),
                        context_id,
                        &runtime_env,
                        &deletions,
                    )
                    .await;

                    let msg = StreamMessage::Message {
                        sequence_id: sqx.next(),
                        payload: MessagePayload::EntityDeletePushAck {
                            applied_count: applied,
                        },
                        next_nonce: super::helpers::generate_nonce(),
                    };
                    transport.send(&msg).await?;
                    requests_handled += 1;

                    info!(%context_id, applied, total, "Applied pushed tombstones (delete-wins)");
                }

                InitPayload::DagHeadsRequest { .. } => {
                    // End-of-session convergence re-read for the initiator's
                    // post-sync check. Re-read our root NOW — after applying
                    // every leaf/tombstone pushed in this session — so the
                    // initiator compares against our live post-merge state
                    // instead of the root it captured at handshake (which goes
                    // stale the moment either side moves, producing both the
                    // forever-WARN false negative and the divergence-masking
                    // false positive).
                    let current_root = with_runtime_env(runtime_env.clone(), || {
                        Index::<MainStorage>::get_hashes_for(Id::new(*context_id.as_ref()))
                            .ok()
                            .flatten()
                            .map(|(full, _)| full)
                            .unwrap_or([0; 32])
                    });

                    // C0 scope_root shadow: fold our governance projection's ACL +
                    // membership onto the just-re-read entity root, so the initiator
                    // can detect a hash-neutral writer/membership rotation that the
                    // bare `root_hash` hides. `None` (non-group / cold projection) ⇒
                    // the initiator skips the compare. Observe-only.
                    let scope_root =
                        super::helpers::local_scope_root(&datastore, &context_id, current_root)
                            .map(Hash::from);

                    let msg = StreamMessage::Message {
                        sequence_id: sqx.next(),
                        payload: MessagePayload::DagHeadsResponse {
                            dag_heads: Vec::new(),
                            root_hash: Hash::from(current_root),
                            scope_root,
                        },
                        next_nonce: super::helpers::generate_nonce(),
                    };
                    transport.send(&msg).await?;
                    requests_handled += 1;
                }

                _ => {
                    debug!(%context_id, "Received unknown payload, ending responder");
                    break;
                }
            }
        }

        info!(%context_id, requests_handled, "HashComparison responder complete");
        Ok(())
    }

    /// Build TreeNodeResponse for a requested node.
    ///
    /// Uses the real Merkle tree Index via RuntimeEnv bridge.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Context being synchronized
    /// * `node_id` - ID of the node to retrieve
    /// * `max_depth` - Maximum depth to traverse (clamped externally)
    /// * `runtime_env` - Pre-created RuntimeEnv (shared across requests for efficiency)
    async fn build_tree_node_response(
        &self,
        context_id: ContextId,
        node_id: &[u8; 32],
        max_depth: Option<u8>,
        runtime_env: &RuntimeEnv,
        schema_app_key: Option<[u8; 32]>,
    ) -> Result<TreeNodeResponse> {
        // Get context to check if this is a root request
        let context = self.context_client.get_context(&context_id)?;
        let Some(context) = context else {
            debug!(
                %context_id,
                "Context not found for TreeNodeRequest"
            );
            return Ok(TreeNodeResponse::not_found());
        };

        // Determine if this is a root request (node_id matches root_hash)
        let is_root_request = node_id == context.root_hash.as_ref();

        // Get the local node. Delegates to the shared, tombstone-aware builder
        // in `hash_comparison_protocol` so the production responder advertises
        // `deleted_children` exactly like the initiator / trait responder do —
        // see that function's doc and the #3217 regression test below.
        let local_node = with_runtime_env(runtime_env.clone(), || {
            super::hash_comparison_protocol::get_local_tree_node(
                context_id,
                node_id,
                is_root_request,
                schema_app_key,
            )
        })?;

        let Some(node) = local_node else {
            debug!(
                %context_id,
                node_id = %hex::encode(node_id),
                "TreeNodeRequest: node not found"
            );
            return Ok(TreeNodeResponse::not_found());
        };

        let mut nodes = vec![node.clone()];

        // If max_depth > 0 and this is an internal node, include children
        let depth = max_depth.unwrap_or(0);
        if depth > 0 && node.is_internal() {
            // Include child nodes
            for child_id in &node.children {
                let child_node = with_runtime_env(runtime_env.clone(), || {
                    super::hash_comparison_protocol::get_local_tree_node(
                        context_id,
                        child_id,
                        false,
                        schema_app_key,
                    )
                })?;

                if let Some(child) = child_node {
                    nodes.push(child);

                    // Limit to avoid oversized responses
                    if nodes.len() >= MAX_NODES_PER_RESPONSE {
                        break;
                    }
                }
            }
        }

        debug!(
            %context_id,
            node_id = %hex::encode(node_id),
            nodes_count = nodes.len(),
            "TreeNodeRequest: returning nodes"
        );

        Ok(TreeNodeResponse::new(nodes))
    }
}

#[cfg(test)]
mod deleted_children_tests {
    use std::sync::Arc;

    use calimero_node_primitives::sync::create_runtime_env;
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PublicKey;
    use calimero_storage::action::Action;
    use calimero_storage::address::Id;
    use calimero_storage::entities::{ChildInfo, Metadata};
    use calimero_storage::env::{time_now, with_runtime_env};
    use calimero_storage::index::Index;
    use calimero_storage::interface::{ApplyContext, Interface};
    use calimero_storage::store::MainStorage;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use crate::sync::hash_comparison_protocol::get_local_tree_node;

    /// The production HashComparison responder MUST advertise a cleared child's
    /// tombstone in its container's `deleted_children`, so a peer that still
    /// holds the entry live converges to the deletion (delete-wins) even when
    /// that peer is the one initiating the sync. Without this, the holder keeps
    /// redelivering its stale leaf every round while the cleared node silently
    /// stale-drops it — the permanent AuthoredMap redelivery loop (#3217).
    #[test]
    fn responder_ships_deleted_children_for_cleared_container() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let context_id = ContextId::from([7u8; 32]);
        let identity = PublicKey::from([0u8; 32]);
        let env = create_runtime_env(&store, context_id, identity);

        let root_id = Id::new(*context_id.as_ref());
        let container_id = Id::new([1u8; 32]);
        let child_id = Id::new([2u8; 32]);

        // Build: root → container → child, then delete the child so the
        // container is childless-but-tombstoned (the cleared-entry shape).
        with_runtime_env(env.clone(), || {
            let apply = |action| {
                Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
                    .expect("apply_action");
            };

            apply(Action::Update {
                id: root_id,
                data: vec![],
                ancestors: vec![],
                metadata: Metadata::default(),
            });

            let root_hash = Index::<MainStorage>::get_hashes_for(root_id)
                .ok()
                .flatten()
                .map(|(full, _)| full)
                .unwrap_or([0; 32]);
            apply(Action::Update {
                id: container_id,
                data: vec![],
                ancestors: vec![ChildInfo::new(root_id, root_hash, Metadata::default())],
                metadata: Metadata::default(),
            });

            let container_hash = Index::<MainStorage>::get_hashes_for(container_id)
                .ok()
                .flatten()
                .map(|(full, _)| full)
                .unwrap_or([0; 32]);
            apply(Action::Update {
                id: child_id,
                data: b"v1".to_vec(),
                ancestors: vec![ChildInfo::new(
                    container_id,
                    container_hash,
                    Metadata::default(),
                )],
                metadata: Metadata::default(),
            });

            let child_metadata = Index::<MainStorage>::get_index(child_id)
                .ok()
                .flatten()
                .map(|idx| idx.metadata.clone())
                .expect("child index");
            apply(Action::DeleteRef {
                id: child_id,
                deleted_at: time_now(),
                metadata: child_metadata,
            });
        });

        // The responder builds the container node for a peer's TreeNodeRequest.
        let node = with_runtime_env(env, || {
            get_local_tree_node(context_id, container_id.as_bytes(), false, None)
        })
        .expect("build node")
        .expect("container node present");

        assert!(
            node.is_internal(),
            "a childless-but-tombstoned container must be an internal node, not a leaf"
        );
        assert!(
            node.deleted_children
                .iter()
                .any(|d| d.id == *child_id.as_bytes()),
            "HC responder must advertise the cleared child's tombstone in deleted_children; \
             got {} tombstone(s) — the AuthoredMap redelivery loop (#3217)",
            node.deleted_children.len()
        );
    }
}
