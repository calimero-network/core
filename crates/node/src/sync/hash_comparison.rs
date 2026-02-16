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

use calimero_crypto::Nonce;
use calimero_node_primitives::sync::{
    create_runtime_env, InitPayload, LeafMetadata, MessagePayload, StreamMessage, SyncTransport,
    TreeLeafData, TreeNode, TreeNodeResponse, MAX_NODES_PER_RESPONSE,
};
use calimero_primitives::context::ContextId;
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

        // Handle the first request (already parsed by handle_sync_request)
        {
            let clamped_depth = first_max_depth.map(|d| d.min(MAX_REQUEST_DEPTH));
            let response = self
                .build_tree_node_response(context_id, &first_node_id, clamped_depth, &runtime_env)
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
            let Some(request) = transport.recv().await? else {
                debug!(%context_id, requests_handled, "Stream closed, responder done");
                break;
            };

            // Expect Init messages with TreeNodeRequest
            let StreamMessage::Init { payload, .. } = request else {
                debug!(%context_id, "Received non-Init message, ending responder");
                break;
            };

            let InitPayload::TreeNodeRequest {
                node_id, max_depth, ..
            } = payload
            else {
                debug!(%context_id, "Received non-TreeNodeRequest, ending responder");
                break;
            };

            trace!(
                %context_id,
                node_id = %hex::encode(node_id),
                ?max_depth,
                "Handling subsequent TreeNodeRequest"
            );

            let clamped_depth = max_depth.map(|d| d.min(MAX_REQUEST_DEPTH));
            let response = self
                .build_tree_node_response(context_id, &node_id, clamped_depth, &runtime_env)
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
            self.get_local_tree_node_from_index(context_id, node_id, is_root_request)
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
                    self.get_local_tree_node_from_index(context_id, child_id, false)
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
                let crdt_type = index.metadata.crdt_type.clone().ok_or_else(|| {
                    eyre::eyre!(
                        "Missing CRDT type metadata for leaf entity {}: data integrity issue",
                        entity_id
                    )
                })?;
                let metadata = LeafMetadata::new(
                    crdt_type,
                    index.metadata.updated_at(),
                    // Collection ID - use parent if available
                    [0u8; 32],
                );

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
