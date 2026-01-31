//! Entity-based sync protocols.
//!
//! Implements HashComparison, BloomFilter, SubtreePrefetch, and LevelWise strategies
//! for synchronizing state ENTITIES (not deltas) between peers.
//!
//! These protocols work on the Merkle tree state directly, using entity keys
//! and values rather than DAG deltas.
//!
//! ## Strategy Overview
//!
//! | Strategy | Round Trips | Best For |
//! |----------|-------------|----------|
//! | BloomFilter | 2 | Large tree, small divergence (<10%) |
//! | HashComparison | O(depth * branches) | General purpose |
//! | SubtreePrefetch | 1 + subtrees | Deep trees, localized changes |
//! | LevelWise | O(depth) | Wide shallow trees |
//!
//! ## Instrumentation
//!
//! Each strategy logs a `STRATEGY_SYNC_METRICS` line with:
//! - `strategy`: The sync strategy used
//! - `round_trips`: Number of network round trips
//! - `entities_synced`: Number of entities transferred
//! - `entities_skipped`: Number of entities already in sync
//! - `bytes_received`: Total bytes received
//! - `bytes_sent`: Approximate bytes sent (filter/requests)
//! - `duration_ms`: Total sync duration
//! - Strategy-specific metrics (e.g., `bloom_filter_size`, `nodes_checked`, etc.)
//!
//! ## Merge Behavior
//!
//! When applying remote entities, these protocols use CRDT merge semantics:
//! - If local entity exists, merge local + remote using `WasmMergeCallback`
//! - If local entity doesn't exist, write remote directly
//! - Built-in CRDTs (Counter, Map) use storage-layer merge
//! - Custom types dispatch to WASM via the callback

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{
    InitPayload, MessagePayload, StreamMessage, TreeNode, TreeNodeChild,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::WasmMergeCallback;
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::slice::Slice;
use calimero_store::types::ContextState as ContextStateValue;
use eyre::{bail, Result};
use libp2p::PeerId;
use rand::Rng;
use tracing::{debug, info, trace, warn};

use super::manager::SyncManager;
use super::snapshot::{build_entity_bloom_filter, get_entity_keys};
use super::tracking::SyncProtocol;

impl SyncManager {
    /// Execute bloom filter sync with a peer.
    ///
    /// 1. Get all local entity keys
    /// 2. Build bloom filter from keys
    /// 3. Send filter to peer
    /// 4. Peer checks their entities against filter
    /// 5. Peer sends back entities we're missing
    /// 6. Apply received entities with CRDT merge
    pub(super) async fn bloom_filter_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
        false_positive_rate: f32,
    ) -> Result<SyncProtocol> {
        let start = Instant::now();
        let mut round_trips = 0u32;

        info!(
            %context_id,
            %peer_id,
            false_positive_rate,
            "Starting ENTITY-based bloom filter sync"
        );

        // Get storage handle via context_client
        let store_handle = self.context_client.datastore_handle();

        // Get all local entity keys
        let local_keys = get_entity_keys(&store_handle, context_id)?;
        let local_entity_count = local_keys.len();

        debug!(
            %context_id,
            local_entity_count,
            "Building bloom filter from local entity keys"
        );

        // Build bloom filter
        let bloom_filter = build_entity_bloom_filter(&local_keys, false_positive_rate);
        let bloom_filter_size = bloom_filter.len();
        let bytes_sent = bloom_filter_size as u64;

        // Send bloom filter request
        let request = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::BloomFilterRequest {
                context_id,
                bloom_filter,
                false_positive_rate,
            },
            next_nonce: rand::thread_rng().gen(),
        };

        self.send(stream, &request, None).await?;
        round_trips += 1;

        let response = self.recv(stream, None).await?;

        match response {
            Some(StreamMessage::Message {
                payload:
                    MessagePayload::BloomFilterResponse {
                        missing_entities,
                        matched_count,
                    },
                ..
            }) => {
                let bytes_received = missing_entities.len() as u64;

                // Get merge callback for CRDT-aware entity application
                let merge_callback = self.get_merge_callback();

                // Decode and apply missing entities with merge
                let entities_synced = self.apply_entities_from_bytes(
                    context_id,
                    &missing_entities,
                    Some(merge_callback.as_ref()),
                )?;

                let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

                // Calculate false positive estimate
                // If matched_count > (remote_entities - entities_synced), we had false positives
                let entities_skipped = matched_count as u64;

                // Log structured metrics for analysis
                info!(
                    %context_id,
                    %peer_id,
                    strategy = "bloom_filter",
                    round_trips,
                    entities_synced,
                    entities_skipped,
                    bytes_received,
                    bytes_sent,
                    duration_ms = format!("{:.2}", duration_ms),
                    // Bloom filter specific
                    bloom_filter_size,
                    false_positive_rate,
                    local_entity_count,
                    matched_count,
                    "STRATEGY_SYNC_METRICS"
                );

                // Record metrics
                self.metrics.record_bytes_received(bytes_received);

                Ok(SyncProtocol::BloomFilter)
            }
            Some(StreamMessage::OpaqueError) => {
                warn!(%context_id, "Peer returned error for bloom filter request");
                bail!("Peer returned error during bloom filter sync");
            }
            other => {
                warn!(%context_id, ?other, "Unexpected response to BloomFilterRequest");
                bail!("Unexpected response during bloom filter sync");
            }
        }
    }

    /// Execute recursive hash comparison sync with a peer.
    ///
    /// Algorithm:
    /// 1. Request root tree node
    /// 2. Compare root hashes - if same, done
    /// 3. For each child with different hash, recursively request children
    /// 4. When reaching leaf nodes with different hashes, transfer the entity data
    ///
    /// This is O(depth * differing_branches) round trips.
    pub(super) async fn hash_comparison_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
        local_root_hash: Hash,
        remote_root_hash: Hash,
    ) -> Result<SyncProtocol> {
        let start = Instant::now();

        info!(
            %context_id,
            %peer_id,
            local_hash = %local_root_hash,
            remote_hash = %remote_root_hash,
            "Starting recursive hash comparison sync"
        );

        // If hashes match, no sync needed
        if local_root_hash == remote_root_hash {
            info!(
                %context_id,
                %peer_id,
                strategy = "hash_comparison",
                round_trips = 0,
                entities_synced = 0,
                entities_skipped = 0,
                bytes_received = 0,
                bytes_sent = 0,
                duration_ms = "0.00",
                nodes_checked = 0,
                max_depth_reached = 0,
                hash_matches = 1,
                "STRATEGY_SYNC_METRICS: Root hashes match, no sync needed"
            );
            return Ok(SyncProtocol::None);
        }

        // Track nodes that need to be fetched (BFS traversal)
        let mut nodes_to_check: VecDeque<([u8; 32], u32)> = VecDeque::new(); // (node_id, depth)
        let mut checked_nodes: HashSet<[u8; 32]> = HashSet::new();
        let mut total_entities_synced = 0u64;
        let mut total_bytes_received = 0u64;
        let mut total_bytes_sent = 0u64;

        // Get merge callback for CRDT-aware entity application
        let merge_callback = self.get_merge_callback();
        let mut round_trips = 0u32;
        let mut max_depth_reached = 0u32;
        let mut hash_comparisons = 0u64;

        // Start with root node (empty node_ids = root)
        nodes_to_check.push_back(([0; 32], 0)); // Root sentinel at depth 0

        while let Some((node_id, depth)) = nodes_to_check.pop_front() {
            if checked_nodes.contains(&node_id) {
                continue;
            }
            checked_nodes.insert(node_id);
            max_depth_reached = max_depth_reached.max(depth);

            // Request this node with immediate children
            let request_ids = if node_id == [0; 32] {
                vec![] // Empty = root
            } else {
                vec![node_id]
            };

            // Estimate bytes sent (rough approximation)
            total_bytes_sent += 64 + (request_ids.len() * 32) as u64;

            let request = StreamMessage::Init {
                context_id,
                party_id: our_identity,
                payload: InitPayload::TreeNodeRequest {
                    context_id,
                    node_ids: request_ids,
                    include_children_depth: 1, // Get immediate children hashes
                },
                next_nonce: rand::thread_rng().gen(),
            };

            self.send(stream, &request, None).await?;
            round_trips += 1;
            let response = self.recv(stream, None).await?;

            match response {
                Some(StreamMessage::Message {
                    payload: MessagePayload::TreeNodeResponse { nodes },
                    ..
                }) => {
                    for node in nodes {
                        debug!(
                            %context_id,
                            node_id = ?node.node_id,
                            hash = %node.hash,
                            children_count = node.children.len(),
                            has_leaf_data = node.leaf_data.is_some(),
                            depth,
                            "Received tree node"
                        );

                        // If this is a leaf with data, apply it with merge
                        if let Some(leaf_data) = &node.leaf_data {
                            total_bytes_received += leaf_data.len() as u64;
                            let applied = self.apply_leaf_entity(
                                context_id,
                                leaf_data,
                                Some(merge_callback.as_ref()),
                            )?;
                            if applied {
                                total_entities_synced += 1;
                            }
                        }

                        // Check children for divergence
                        for child in &node.children {
                            hash_comparisons += 1;
                            // Check if we have this child with same hash
                            let need_sync = self
                                .check_local_node_differs(context_id, &child.node_id, &child.hash)
                                .await;

                            if need_sync && !checked_nodes.contains(&child.node_id) {
                                nodes_to_check.push_back((child.node_id, depth + 1));
                            }
                        }
                    }
                }
                Some(StreamMessage::OpaqueError) => {
                    warn!(%context_id, "Peer returned error for tree node request");
                    bail!("Peer returned error during hash comparison sync");
                }
                other => {
                    warn!(%context_id, ?other, "Unexpected response to TreeNodeRequest");
                    bail!("Unexpected response during hash comparison sync");
                }
            }
        }

        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

        // Log structured metrics for analysis
        info!(
            %context_id,
            %peer_id,
            strategy = "hash_comparison",
            round_trips,
            entities_synced = total_entities_synced,
            entities_skipped = 0, // Hash comparison doesn't skip, it compares
            bytes_received = total_bytes_received,
            bytes_sent = total_bytes_sent,
            duration_ms = format!("{:.2}", duration_ms),
            // Hash comparison specific
            nodes_checked = checked_nodes.len(),
            max_depth_reached,
            hash_comparisons,
            "STRATEGY_SYNC_METRICS"
        );

        self.metrics.record_bytes_received(total_bytes_received);

        Ok(SyncProtocol::HashComparison)
    }

    /// Execute subtree prefetch sync with a peer.
    ///
    /// Similar to hash comparison, but when we find a divergent subtree,
    /// we fetch the ENTIRE subtree in one request (include_children_depth = max).
    ///
    /// This is efficient for deep trees with localized changes.
    pub(super) async fn subtree_prefetch_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
        local_root_hash: Hash,
        remote_root_hash: Hash,
        max_depth: Option<usize>,
    ) -> Result<SyncProtocol> {
        let start = Instant::now();
        let mut round_trips = 0u32;
        let mut total_bytes_sent = 0u64;

        info!(
            %context_id,
            %peer_id,
            local_hash = %local_root_hash,
            remote_hash = %remote_root_hash,
            ?max_depth,
            "Starting subtree prefetch sync"
        );

        // If hashes match, no sync needed
        if local_root_hash == remote_root_hash {
            info!(
                %context_id,
                %peer_id,
                strategy = "subtree_prefetch",
                round_trips = 0,
                entities_synced = 0,
                entities_skipped = 0,
                bytes_received = 0,
                bytes_sent = 0,
                duration_ms = "0.00",
                subtrees_fetched = 0,
                divergent_children = 0,
                prefetch_depth = max_depth.unwrap_or(255),
                "STRATEGY_SYNC_METRICS: Root hashes match, no sync needed"
            );
            return Ok(SyncProtocol::None);
        }

        // First, get root node with shallow depth to find divergent subtrees
        total_bytes_sent += 64; // Approximate request size
        let request = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::TreeNodeRequest {
                context_id,
                node_ids: vec![], // Root
                include_children_depth: 1,
            },
            next_nonce: rand::thread_rng().gen(),
        };

        self.send(stream, &request, None).await?;
        round_trips += 1;
        let response = self.recv(stream, None).await?;

        let root_children: Vec<TreeNodeChild> = match response {
            Some(StreamMessage::Message {
                payload: MessagePayload::TreeNodeResponse { nodes },
                ..
            }) => nodes.into_iter().flat_map(|n| n.children).collect(),
            _ => {
                bail!("Failed to get root node for subtree prefetch");
            }
        };

        let total_children = root_children.len();
        let mut divergent_children = 0u32;
        let mut subtrees_fetched = 0u32;
        let mut total_entities_synced = 0u64;
        let mut total_bytes_received = 0u64;

        // Get merge callback for CRDT-aware entity application
        let merge_callback = self.get_merge_callback();

        // For each divergent child, fetch entire subtree
        for child in root_children {
            let need_sync = self
                .check_local_node_differs(context_id, &child.node_id, &child.hash)
                .await;

            if need_sync {
                divergent_children += 1;
                debug!(
                    %context_id,
                    child_id = ?child.node_id,
                    "Fetching divergent subtree"
                );

                // Request full subtree (max depth)
                let prefetch_depth = max_depth.unwrap_or(255) as u8;
                total_bytes_sent += 64 + 32; // Request with one node_id
                let request = StreamMessage::Init {
                    context_id,
                    party_id: our_identity,
                    payload: InitPayload::TreeNodeRequest {
                        context_id,
                        node_ids: vec![child.node_id],
                        include_children_depth: prefetch_depth,
                    },
                    next_nonce: rand::thread_rng().gen(),
                };

                self.send(stream, &request, None).await?;
                round_trips += 1;
                let response = self.recv(stream, None).await?;

                match response {
                    Some(StreamMessage::Message {
                        payload: MessagePayload::TreeNodeResponse { nodes },
                        ..
                    }) => {
                        subtrees_fetched += 1;
                        // Apply all leaf entities from the subtree
                        for node in nodes {
                            if let Some(leaf_data) = &node.leaf_data {
                                total_bytes_received += leaf_data.len() as u64;
                                let applied = self.apply_leaf_entity(
                                    context_id,
                                    leaf_data,
                                    Some(merge_callback.as_ref()),
                                )?;
                                if applied {
                                    total_entities_synced += 1;
                                }
                            }
                        }
                    }
                    _ => {
                        warn!(%context_id, child_id = ?child.node_id, "Failed to fetch subtree");
                    }
                }
            }
        }

        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

        // Log structured metrics for analysis
        info!(
            %context_id,
            %peer_id,
            strategy = "subtree_prefetch",
            round_trips,
            entities_synced = total_entities_synced,
            entities_skipped = (total_children as u32 - divergent_children),
            bytes_received = total_bytes_received,
            bytes_sent = total_bytes_sent,
            duration_ms = format!("{:.2}", duration_ms),
            // Subtree prefetch specific
            subtrees_fetched,
            divergent_children,
            total_children,
            prefetch_depth = max_depth.unwrap_or(255),
            "STRATEGY_SYNC_METRICS"
        );

        self.metrics.record_bytes_received(total_bytes_received);

        Ok(SyncProtocol::SubtreePrefetch)
    }

    /// Execute level-wise breadth-first sync with a peer.
    ///
    /// Syncs one tree level at a time, batching all requests per depth.
    /// Efficient for wide shallow trees where many siblings differ.
    pub(super) async fn level_wise_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
        local_root_hash: Hash,
        remote_root_hash: Hash,
        max_depth: Option<usize>,
    ) -> Result<SyncProtocol> {
        let start = Instant::now();
        let mut round_trips = 0u32;
        let mut total_bytes_sent = 0u64;

        info!(
            %context_id,
            %peer_id,
            local_hash = %local_root_hash,
            remote_hash = %remote_root_hash,
            ?max_depth,
            "Starting level-wise sync"
        );

        // If hashes match, no sync needed
        if local_root_hash == remote_root_hash {
            info!(
                %context_id,
                %peer_id,
                strategy = "level_wise",
                round_trips = 0,
                entities_synced = 0,
                entities_skipped = 0,
                bytes_received = 0,
                bytes_sent = 0,
                duration_ms = "0.00",
                levels_synced = 0,
                max_nodes_per_level = 0,
                total_nodes_checked = 0,
                "STRATEGY_SYNC_METRICS: Root hashes match, no sync needed"
            );
            return Ok(SyncProtocol::None);
        }

        let max_depth = max_depth.unwrap_or(10);
        let mut total_entities_synced = 0u64;
        let mut total_bytes_received = 0u64;
        let mut current_level_ids: Vec<[u8; 32]> = vec![]; // Empty = root
        let mut levels_synced = 0u32;
        let mut max_nodes_per_level = 0usize;
        let mut total_nodes_checked = 0u64;

        // Get merge callback for CRDT-aware entity application
        let merge_callback = self.get_merge_callback();

        for depth in 0..=max_depth {
            // Estimate bytes sent
            total_bytes_sent += 64 + (current_level_ids.len() * 32) as u64;

            // Request all nodes at current level
            let request = StreamMessage::Init {
                context_id,
                party_id: our_identity,
                payload: InitPayload::TreeNodeRequest {
                    context_id,
                    node_ids: current_level_ids.clone(),
                    include_children_depth: 1, // Get immediate children
                },
                next_nonce: rand::thread_rng().gen(),
            };

            self.send(stream, &request, None).await?;
            round_trips += 1;
            let response = self.recv(stream, None).await?;

            let nodes: Vec<TreeNode> = match response {
                Some(StreamMessage::Message {
                    payload: MessagePayload::TreeNodeResponse { nodes },
                    ..
                }) => nodes,
                _ => {
                    warn!(%context_id, depth, "Failed to get level nodes");
                    break;
                }
            };

            total_nodes_checked += nodes.len() as u64;
            max_nodes_per_level = max_nodes_per_level.max(nodes.len());
            levels_synced = depth as u32 + 1;

            debug!(
                %context_id,
                depth,
                nodes_received = nodes.len(),
                "Received level nodes"
            );

            // Collect children for next level
            let mut next_level_ids: Vec<[u8; 32]> = Vec::new();

            for node in nodes {
                // Apply leaf data if present with merge
                if let Some(leaf_data) = &node.leaf_data {
                    total_bytes_received += leaf_data.len() as u64;
                    let applied = self.apply_leaf_entity(
                        context_id,
                        leaf_data,
                        Some(merge_callback.as_ref()),
                    )?;
                    if applied {
                        total_entities_synced += 1;
                    }
                }

                // Collect divergent children for next level
                for child in &node.children {
                    let need_sync = self
                        .check_local_node_differs(context_id, &child.node_id, &child.hash)
                        .await;

                    if need_sync {
                        next_level_ids.push(child.node_id);
                    }
                }
            }

            if next_level_ids.is_empty() {
                debug!(%context_id, depth, "No more divergent nodes at this level");
                break;
            }

            debug!(
                %context_id,
                depth,
                next_level_count = next_level_ids.len(),
                "Moving to next level"
            );

            current_level_ids = next_level_ids;
        }

        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

        // Log structured metrics for analysis
        info!(
            %context_id,
            %peer_id,
            strategy = "level_wise",
            round_trips,
            entities_synced = total_entities_synced,
            entities_skipped = 0,
            bytes_received = total_bytes_received,
            bytes_sent = total_bytes_sent,
            duration_ms = format!("{:.2}", duration_ms),
            // Level-wise specific
            levels_synced,
            max_nodes_per_level,
            total_nodes_checked,
            configured_max_depth = max_depth,
            "STRATEGY_SYNC_METRICS"
        );

        self.metrics.record_bytes_received(total_bytes_received);

        Ok(SyncProtocol::LevelWise)
    }

    // =========================================================================
    // Helper Methods
    // =========================================================================

    /// Apply a single entity with CRDT merge semantics.
    ///
    /// If local entity exists, merges local + remote using the callback.
    /// If local entity doesn't exist, writes remote directly.
    ///
    /// Returns true if entity was written, false if skipped.
    fn apply_entity_with_merge(
        &self,
        context_id: ContextId,
        key: [u8; 32],
        remote_value: Vec<u8>,
        merge_callback: Option<&dyn WasmMergeCallback>,
    ) -> Result<bool> {
        let state_key = ContextStateKey::new(context_id, key);
        let mut store_handle = self.context_client.datastore_handle();

        // Try to read existing local value
        let local_value: Option<Vec<u8>> = store_handle
            .get(&state_key)
            .ok()
            .flatten()
            .map(|v: ContextStateValue| v.as_ref().to_vec());

        let final_value = if let Some(local_data) = local_value {
            // Local exists - need to merge
            if let Some(callback) = merge_callback {
                // Use callback for merge (handles built-in CRDTs and custom types)
                // Note: We don't have full metadata here, so use type_name "unknown"
                // The callback will fall back to LWW for unknown types
                match callback.merge_custom(
                    "unknown", // Type name not available at this level
                    &local_data,
                    &remote_value,
                    0, // Local timestamp not available
                    1, // Remote timestamp (assume newer)
                ) {
                    Ok(merged) => {
                        trace!(
                            %context_id,
                            entity_key = ?key,
                            local_len = local_data.len(),
                            remote_len = remote_value.len(),
                            merged_len = merged.len(),
                            "Merged entity via callback"
                        );
                        merged
                    }
                    Err(e) => {
                        warn!(
                            %context_id,
                            entity_key = ?key,
                            error = %e,
                            "Merge callback failed, using remote (LWW)"
                        );
                        remote_value
                    }
                }
            } else {
                // No callback - use LWW (remote wins)
                trace!(
                    %context_id,
                    entity_key = ?key,
                    "No merge callback, using remote (LWW)"
                );
                remote_value
            }
        } else {
            // No local value - just use remote
            remote_value
        };

        // Write the final value
        let slice: Slice<'_> = final_value.into();
        store_handle.put(&state_key, &ContextStateValue::from(slice))?;

        debug!(
            %context_id,
            entity_key = ?key,
            "Applied entity with merge"
        );

        Ok(true)
    }

    /// Apply entities from serialized bytes (format: key[32] + len[4] + value[len])
    ///
    /// Uses CRDT merge when local entity exists.
    fn apply_entities_from_bytes(
        &self,
        context_id: ContextId,
        data: &[u8],
        merge_callback: Option<&dyn WasmMergeCallback>,
    ) -> Result<u64> {
        let mut entities_applied = 0u64;
        let mut offset = 0;

        while offset + 36 <= data.len() {
            // Read key (32 bytes)
            let mut key = [0u8; 32];
            key.copy_from_slice(&data[offset..offset + 32]);
            offset += 32;

            // Read value length (4 bytes)
            let value_len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;

            if offset + value_len > data.len() {
                warn!(%context_id, "Truncated entity data");
                break;
            }

            let value = data[offset..offset + value_len].to_vec();
            offset += value_len;

            // Apply entity with merge
            match self.apply_entity_with_merge(context_id, key, value, merge_callback) {
                Ok(true) => {
                    entities_applied += 1;
                }
                Ok(false) => {
                    debug!(%context_id, entity_key = ?key, "Entity skipped");
                }
                Err(e) => {
                    warn!(
                        %context_id,
                        entity_key = ?key,
                        error = %e,
                        "Failed to apply entity"
                    );
                }
            }
        }

        Ok(entities_applied)
    }

    /// Apply a single leaf entity from serialized data.
    ///
    /// Expected format: key[32] + value_len[4] + value[value_len]
    /// Uses CRDT merge when local entity exists.
    fn apply_leaf_entity(
        &self,
        context_id: ContextId,
        leaf_data: &[u8],
        merge_callback: Option<&dyn WasmMergeCallback>,
    ) -> Result<bool> {
        if leaf_data.len() < 36 {
            return Ok(false);
        }

        let mut key = [0u8; 32];
        key.copy_from_slice(&leaf_data[0..32]);

        let value_len =
            u32::from_le_bytes([leaf_data[32], leaf_data[33], leaf_data[34], leaf_data[35]])
                as usize;

        if leaf_data.len() < 36 + value_len {
            return Ok(false);
        }

        let value = leaf_data[36..36 + value_len].to_vec();

        self.apply_entity_with_merge(context_id, key, value, merge_callback)
    }

    /// Check if a local node differs from remote (by hash).
    ///
    /// Returns true if we should fetch this node (either we don't have it
    /// or our hash differs).
    async fn check_local_node_differs(
        &self,
        context_id: ContextId,
        node_id: &[u8; 32],
        remote_hash: &Hash,
    ) -> bool {
        // For now, always return true to fetch all nodes.
        // A full implementation would look up the local Merkle tree node
        // and compare hashes.
        //
        // The storage layer doesn't expose per-node hashes directly,
        // so we use a conservative approach: always sync if parent differs.
        //
        // TODO: Implement proper Merkle tree node lookup in storage layer
        let _ = (context_id, node_id, remote_hash);
        true
    }
}
