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
    create_runtime_env, InitPayload, LeafMetadata, MessagePayload, StreamMessage, SyncTransport,
    TreeLeafData, TreeNode, TreeNodeResponse, MAX_NODES_PER_RESPONSE,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::CrdtType;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::env::{with_runtime_env, RuntimeEnv};
use calimero_storage::index::Index;
use calimero_storage::interface::Interface;
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

                    let msg = StreamMessage::Message {
                        sequence_id: sqx.next(),
                        payload: MessagePayload::DagHeadsResponse {
                            dag_heads: Vec::new(),
                            root_hash: Hash::from(current_root),
                        },
                        next_nonce: super::helpers::generate_nonce(),
                    };
                    transport.send(&msg).await?;
                    requests_handled += 1;
                }

                InitPayload::RotationLogSyncRequest { logs, .. } => {
                    // End-of-session rotation-log reconciliation (core#2716/#2703).
                    // A writer-set rotation is hash-neutral, so HC never carries
                    // it; union the initiator's Shared rotation logs into ours and
                    // reply with our own so the initiator unions them too — one
                    // round-trip reconciles both directions.
                    let applied = with_runtime_env(runtime_env.clone(), || {
                        super::hash_comparison_protocol::union_received_rotation_logs(&logs)
                    });
                    let local_logs = with_runtime_env(runtime_env.clone(), || {
                        super::hash_comparison_protocol::collect_local_shared_rotation_logs(
                            context_id,
                        )
                    });

                    let msg = StreamMessage::Message {
                        sequence_id: sqx.next(),
                        payload: MessagePayload::RotationLogSyncResponse { logs: local_logs },
                        next_nonce: super::helpers::generate_nonce(),
                    };
                    transport.send(&msg).await?;
                    requests_handled += 1;

                    if applied > 0 {
                        info!(%context_id, applied, "rotation-log sync: unioned initiator's Shared rotation logs");
                    }
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

        // Get the local node
        let local_node = with_runtime_env(runtime_env.clone(), || {
            self.get_local_tree_node_from_index(
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
                    self.get_local_tree_node_from_index(context_id, child_id, false, schema_app_key)
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

    /// Get local tree node from the real Merkle tree Index.
    ///
    /// Must be called within `with_runtime_env` context.
    fn get_local_tree_node_from_index(
        &self,
        context_id: ContextId,
        node_id: &[u8; 32],
        is_root_request: bool,
        schema_app_key: Option<[u8; 32]>,
    ) -> Result<Option<TreeNode>> {
        // Determine the entity ID to look up
        let entity_id = if is_root_request {
            // For root request, look up Id::root() (which equals context_id)
            Id::new(*context_id.as_ref())
        } else {
            // For child requests, node_id IS the entity ID
            Id::new(*node_id)
        };

        // Get the entity's index from the Merkle tree
        let index = match Index::<MainStorage>::get_index(entity_id) {
            Ok(Some(idx)) => idx,
            Ok(None) => return Ok(None),
            Err(e) => {
                warn!(
                    %context_id,
                    entity_id = %entity_id,
                    error = %e,
                    "Failed to get index for entity"
                );
                return Ok(None);
            }
        };

        // Get hashes from the index
        let full_hash = index.full_hash();

        // Get children from the index
        let children_ids: Vec<[u8; 32]> = index
            .children()
            .map(|children| {
                children
                    .iter()
                    .map(|child| *child.id().as_bytes())
                    .collect()
            })
            .unwrap_or_default();

        // Determine if this is a leaf or internal node
        if children_ids.is_empty() {
            // Leaf node - try to get entity data
            if let Some(entry_data) = Interface::<MainStorage>::find_by_id_raw(entity_id) {
                let crdt_type = index.metadata.crdt_type.clone().unwrap_or_else(|| {
                    // No CRDT type ("opaque" leaf — e.g. the `Root<T>` state entry).
                    // Emit a real *leaf* (not a malformed empty `internal` node, which
                    // the peer's `TreeNode::is_valid()` rejects) carrying a synthetic
                    // LWW wire type — merge-equivalent to `None` and Merkle-hash-neutral.
                    // Same Model-S fix as `hash_comparison_protocol.rs` (see
                    // `OPAQUE_LEAF_CRDT_TYPE_NAME` there + opaque-leaf-sync design spec).
                    trace!(%entity_id, "opaque leaf, synthesised LWW wire type for sync");
                    CrdtType::lww_register(
                        super::hash_comparison_protocol::OPAQUE_LEAF_CRDT_TYPE_NAME,
                    )
                });
                // Carry the leaf's Merkle parent_id on the wire so the
                // initiator can reconstruct the entity at its proper Merkle
                // position. Pre-fix this field was unconditionally `None`
                // and the initiator's apply path fell back to "direct child
                // of context root" — corrupting the topology for any nested-
                // collection entity (every `Root<T>`-wrapped app, which is
                // ~all of them). See the design spec for the wire-format
                // analysis: the field already exists on `LeafMetadata`, just
                // wasn't populated.
                let mut metadata =
                    LeafMetadata::new(crdt_type, index.metadata.updated_at(), [0u8; 32])
                        .with_created_at(index.metadata.created_at());
                if let Some(parent_id) = index.parent_id() {
                    metadata = metadata.with_parent(*parent_id.as_bytes());
                }
                // Carry the full ancestor chain so the receiver places the
                // entity at its exact Merkle position. A non-root entity
                // shipped without its chain forces the receiver's
                // `apply_leaf_with_crdt_merge` empty-ancestors fallback,
                // which `add_root`s the missing ancestors — wrong tree
                // position → divergent Merkle root that HashComparison
                // cannot heal (the same-DAG-heads / different-root
                // split-brain). Surface the error loudly instead of
                // silently shipping a leaf we couldn't resolve a chain for;
                // the receiver-side guard then declines to guess its
                // position rather than misplacing it.
                match Index::<MainStorage>::get_ancestors_of(entity_id) {
                    Ok(ancestors) => {
                        metadata = metadata.with_ancestors(ancestors);
                    }
                    Err(err) => {
                        warn!(
                            %entity_id,
                            ?err,
                            "HC sender: could not resolve ancestor chain for leaf; \
                             shipping with parent_id only (receiver will not guess \
                             its tree position)"
                        );
                    }
                }
                if let Some(auth) = crate::sync::helpers::wire_authorization_for(&index.metadata) {
                    metadata = metadata.with_authorization(auth);
                }
                // PR-6b Task 6b.7: stamp the responder's loaded-reader schema so
                // a receiver on an older reader can decline+buffer this leaf if
                // it's future-schema.
                if let Some(schema) = schema_app_key {
                    metadata = metadata.with_schema_app_key(schema);
                }

                let leaf_data = TreeLeafData::new(*entity_id.as_bytes(), entry_data, metadata);

                Ok(Some(TreeNode::leaf(
                    *entity_id.as_bytes(),
                    full_hash,
                    leaf_data,
                )))
            } else {
                // Index exists but no entry data - treat as internal node with no children
                // This can happen for collection containers
                Ok(Some(TreeNode::internal(
                    *entity_id.as_bytes(),
                    full_hash,
                    vec![],
                )))
            }
        } else {
            // Internal node with children
            Ok(Some(TreeNode::internal(
                *entity_id.as_bytes(),
                full_hash,
                children_ids,
            )))
        }
    }
}
