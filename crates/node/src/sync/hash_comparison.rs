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
    InitPayload, MAX_NODES_PER_RESPONSE, MessagePayload, StreamMessage, SyncTransport,
    TreeNodeResponse, create_runtime_env, get_local_tree_node,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::env::with_runtime_env;
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
    /// * `nonce` - Initial nonce (unused; each response generates its own nonce)
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

        // Handle the first request (already parsed by handle_sync_request)
        {
            let clamped_depth = first_max_depth.map(|d| d.min(MAX_REQUEST_DEPTH));
            let response = self
                .build_tree_node_response(context_id, &first_node_id, clamped_depth, our_identity)
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
                .build_tree_node_response(context_id, &node_id, clamped_depth, our_identity)
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
    async fn build_tree_node_response(
        &self,
        context_id: ContextId,
        node_id: &[u8; 32],
        max_depth: Option<u8>,
        our_identity: PublicKey,
    ) -> Result<TreeNodeResponse> {
        // Set up storage bridge using shared helper
        let datastore = self.context_client.datastore_handle().into_inner();
        let runtime_env = create_runtime_env(&datastore, context_id, our_identity);

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

        // Get the local node using the shared helper
        let local_node = with_runtime_env(runtime_env.clone(), || {
            get_local_tree_node(context_id, node_id, is_root_request)
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
                    get_local_tree_node(context_id, child_id, false)
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
