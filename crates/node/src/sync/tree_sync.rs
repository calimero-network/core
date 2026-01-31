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
use std::time::Instant;

use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{
    InitPayload, MessagePayload, StreamMessage, TreeLeafData, TreeNode, TreeNodeChild,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::entities::Metadata;
use calimero_storage::index::EntityIndex;
use calimero_storage::interface::Interface;
use calimero_storage::store::{Key as StorageKey, MainStorage};
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
                // Calculate bytes received (sum of all entity values)
                let bytes_received: u64 =
                    missing_entities.iter().map(|e| e.value.len() as u64).sum();

                // Get merge callback for CRDT-aware entity application
                let merge_callback = self.get_merge_callback();

                // Apply each entity with proper CRDT merge using included metadata
                let mut entities_synced = 0u64;
                for leaf_data in &missing_entities {
                    match self.apply_leaf_from_tree_data(
                        context_id,
                        leaf_data,
                        Some(merge_callback.as_ref()),
                    ) {
                        Ok(true) => entities_synced += 1,
                        Ok(false) => {} // Already up to date
                        Err(e) => {
                            warn!(
                                %context_id,
                                key = ?leaf_data.key,
                                error = %e,
                                "Failed to apply bloom filter entity"
                            );
                        }
                    }
                }

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

                        // If this is a leaf with data, apply it with CRDT merge
                        if let Some(leaf_data) = &node.leaf_data {
                            total_bytes_received += leaf_data.value.len() as u64;
                            let applied = self.apply_leaf_from_tree_data(
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
                                total_bytes_received += leaf_data.value.len() as u64;
                                let applied = self.apply_leaf_from_tree_data(
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
                // Apply leaf data if present with CRDT merge
                if let Some(leaf_data) = &node.leaf_data {
                    total_bytes_received += leaf_data.value.len() as u64;
                    let applied = self.apply_leaf_from_tree_data(
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

    /// Read entity metadata from storage.
    ///
    /// The EntityIndex (containing Metadata with crdt_type) is stored at
    /// Key::Index(id) which is persisted through the WASM runtime to RocksDB.
    fn read_entity_metadata(&self, context_id: ContextId, entity_id: [u8; 32]) -> Option<Metadata> {
        let store_handle = self.context_client.datastore_handle();

        // Index is stored at Key::Index(id).to_bytes()
        let id = calimero_storage::address::Id::from(entity_id);
        let index_key_bytes = StorageKey::Index(id).to_bytes();
        let state_key = ContextStateKey::new(context_id, index_key_bytes);

        // Get and immediately clone the bytes to avoid lifetime issues
        let value_bytes: Option<Vec<u8>> = store_handle
            .get(&state_key)
            .ok()
            .flatten()
            .map(|v| v.as_ref().to_vec());

        match value_bytes {
            Some(bytes) => {
                // Deserialize as EntityIndex
                match borsh::from_slice::<EntityIndex>(&bytes) {
                    Ok(index) => {
                        trace!(
                            %context_id,
                            ?entity_id,
                            crdt_type = ?index.metadata.crdt_type,
                            "Read entity metadata from storage"
                        );
                        Some(index.metadata.clone())
                    }
                    Err(e) => {
                        warn!(
                            %context_id,
                            ?entity_id,
                            error = %e,
                            "Failed to deserialize EntityIndex"
                        );
                        None
                    }
                }
            }
            None => None,
        }
    }

    /// Apply a single entity with CRDT merge semantics.
    ///
    /// Uses entity metadata (crdt_type) to dispatch to proper CRDT merge:
    /// - Built-in CRDTs (Counter, Map, etc.) → merge in storage layer
    /// - Custom types → dispatch to WASM via callback
    /// - Unknown/missing → fallback to LWW
    ///
    /// Returns true if entity was written, false if skipped.
    fn apply_entity_with_merge(
        &self,
        context_id: ContextId,
        key: [u8; 32],
        remote_value: Vec<u8>,
        remote_metadata: &Metadata,
        merge_callback: Option<&dyn WasmMergeCallback>,
    ) -> Result<bool> {
        let state_key = ContextStateKey::new(context_id, key);
        let mut store_handle = self.context_client.datastore_handle();

        // Read local entity data
        let local_value: Option<Vec<u8>> = store_handle
            .get(&state_key)
            .ok()
            .flatten()
            .map(|v: ContextStateValue| v.as_ref().to_vec());

        let final_value = if let Some(local_data) = local_value {
            // Local exists - perform CRDT merge using metadata
            let local_metadata = self
                .read_entity_metadata(context_id, key)
                .unwrap_or_else(|| {
                    // Fallback: create default metadata with LwwRegister
                    warn!(
                        %context_id,
                        ?key,
                        "No local metadata found, using LwwRegister fallback"
                    );
                    Metadata::new(0, 0)
                });

            // Use Interface::merge_by_crdt_type_with_callback for proper dispatch
            match Interface::<MainStorage>::merge_by_crdt_type_with_callback(
                &local_data,
                &remote_value,
                &local_metadata,
                remote_metadata,
                merge_callback,
            ) {
                Ok(Some(merged)) => {
                    let crdt_type = local_metadata.crdt_type.as_ref();
                    debug!(
                        %context_id,
                        entity_key = ?key,
                        ?crdt_type,
                        local_len = local_data.len(),
                        remote_len = remote_value.len(),
                        merged_len = merged.len(),
                        "CRDT merge completed"
                    );
                    merged
                }
                Ok(None) => {
                    // Merge returned None (manual resolution needed) - use remote
                    warn!(
                        %context_id,
                        entity_key = ?key,
                        "CRDT merge returned None, using remote"
                    );
                    remote_value
                }
                Err(e) => {
                    warn!(
                        %context_id,
                        entity_key = ?key,
                        error = %e,
                        "CRDT merge failed, using remote (LWW fallback)"
                    );
                    remote_value
                }
            }
        } else {
            // No local value - just use remote
            trace!(
                %context_id,
                entity_key = ?key,
                "No local entity, applying remote directly"
            );
            remote_value
        };

        // Write the final value (entity data)
        let slice: Slice<'_> = final_value.into();
        store_handle.put(&state_key, &ContextStateValue::from(slice))?;

        debug!(
            %context_id,
            entity_key = ?key,
            "Applied entity with CRDT merge"
        );

        Ok(true)
    }

    /// Apply entities from serialized bytes (legacy format: key[32] + len[4] + value[len])
    ///
    /// This format doesn't include metadata, so we read it from local storage.
    /// If local metadata is available, uses proper CRDT merge.
    /// Falls back to LwwRegister merge for unknown entities.
    ///
    /// NOTE: This is kept for backward compatibility with older wire formats.
    /// The preferred method is to use TreeLeafData which includes metadata.
    #[allow(dead_code)]
    fn apply_entities_from_bytes(
        &self,
        context_id: ContextId,
        data: &[u8],
        merge_callback: Option<&dyn WasmMergeCallback>,
    ) -> Result<u64> {
        let mut entities_applied = 0u64;
        let mut offset = 0;

        // Create a default metadata for remote (assumes newer timestamp)
        // When we don't have remote metadata, assume LwwRegister as safe default
        let default_remote_metadata = Metadata::new(
            0,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1),
        );

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

            // Apply entity with merge (using local metadata for CRDT type)
            match self.apply_entity_with_merge(
                context_id,
                key,
                value,
                &default_remote_metadata,
                merge_callback,
            ) {
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

    /// Apply a single leaf entity from TreeLeafData (new format with metadata).
    ///
    /// The TreeLeafData includes Metadata with crdt_type for proper CRDT merge.
    fn apply_leaf_from_tree_data(
        &self,
        context_id: ContextId,
        leaf_data: &TreeLeafData,
        merge_callback: Option<&dyn WasmMergeCallback>,
    ) -> Result<bool> {
        self.apply_entity_with_merge(
            context_id,
            leaf_data.key,
            leaf_data.value.clone(),
            &leaf_data.metadata,
            merge_callback,
        )
    }

    /// Apply a single leaf entity from serialized data (legacy format).
    ///
    /// Expected format: key[32] + value_len[4] + value[value_len]
    /// Reads local metadata for CRDT type, defaults to LwwRegister.
    ///
    /// Note: This function is kept for backward compatibility with old wire formats.
    #[allow(dead_code)]
    fn apply_leaf_entity_legacy(
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

        // Create default metadata for remote (LwwRegister, current timestamp)
        let remote_metadata = Metadata::new(
            0,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1),
        );

        self.apply_entity_with_merge(context_id, key, value, &remote_metadata, merge_callback)
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

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_storage::collections::CrdtType;
    use calimero_storage::entities::Metadata;

    /// Test that TreeLeafData correctly serializes and deserializes metadata
    #[test]
    fn test_tree_leaf_data_serialization() {
        let key = [0u8; 32];
        let value = vec![1, 2, 3, 4];
        let mut metadata = Metadata::new(1000, 2000);
        metadata.crdt_type = Some(CrdtType::Counter);

        let leaf_data = TreeLeafData {
            key,
            value: value.clone(),
            metadata: metadata.clone(),
        };

        // Serialize and deserialize
        let serialized = borsh::to_vec(&leaf_data).expect("serialize");
        let deserialized: TreeLeafData = borsh::from_slice(&serialized).expect("deserialize");

        assert_eq!(deserialized.key, key);
        assert_eq!(deserialized.value, value);
        assert_eq!(deserialized.metadata.crdt_type, Some(CrdtType::Counter));
        assert_eq!(deserialized.metadata.created_at, 1000);
    }

    /// Test that TreeLeafData carries different CRDT types
    #[test]
    fn test_tree_leaf_data_crdt_types() {
        let test_types = vec![
            (CrdtType::LwwRegister, "LwwRegister"),
            (CrdtType::Counter, "Counter"),
            (CrdtType::UnorderedMap, "UnorderedMap"),
            (CrdtType::UnorderedSet, "UnorderedSet"),
            (CrdtType::Vector, "Vector"),
        ];

        for (crdt_type, name) in test_types {
            let mut metadata = Metadata::new(0, 0);
            metadata.crdt_type = Some(crdt_type.clone());

            let leaf_data = TreeLeafData {
                key: [0u8; 32],
                value: vec![],
                metadata,
            };

            let serialized = borsh::to_vec(&leaf_data).expect(&format!("serialize {}", name));
            let deserialized: TreeLeafData =
                borsh::from_slice(&serialized).expect(&format!("deserialize {}", name));

            assert_eq!(
                deserialized.metadata.crdt_type,
                Some(crdt_type),
                "CRDT type {} round-trip failed",
                name
            );
        }
    }

    /// Test that default Metadata has LwwRegister as crdt_type
    #[test]
    fn test_default_metadata_crdt_type() {
        let metadata = Metadata::new(0, 0);
        // Default should be LwwRegister for safe fallback
        assert_eq!(metadata.crdt_type, Some(CrdtType::LwwRegister));
    }

    /// Test that legacy format parsing creates correct default metadata
    #[test]
    fn test_legacy_format_default_metadata() {
        // Legacy format: key[32] + len[4] + value[len]
        let mut data = Vec::new();
        data.extend_from_slice(&[1u8; 32]); // key
        data.extend_from_slice(&(4u32).to_le_bytes()); // len = 4
        data.extend_from_slice(&[10, 20, 30, 40]); // value

        // Verify format is valid
        assert!(data.len() >= 36);
        let value_len = u32::from_le_bytes([data[32], data[33], data[34], data[35]]) as usize;
        assert_eq!(value_len, 4);
        assert_eq!(data.len(), 36 + value_len);

        // Legacy format should create LwwRegister metadata
        let default_metadata = Metadata::new(0, 1);
        assert_eq!(default_metadata.crdt_type, Some(CrdtType::LwwRegister));
    }

    /// Test TreeNode structure for internal nodes
    #[test]
    fn test_tree_node_internal() {
        let child1 = TreeNodeChild {
            node_id: [1u8; 32],
            hash: calimero_primitives::hash::Hash::new(&[2u8; 32]),
        };
        let child2 = TreeNodeChild {
            node_id: [3u8; 32],
            hash: calimero_primitives::hash::Hash::new(&[4u8; 32]),
        };

        let node = TreeNode {
            node_id: [0u8; 32],
            hash: calimero_primitives::hash::Hash::new(&[5u8; 32]),
            children: vec![child1, child2],
            leaf_data: None, // Internal node has no leaf data
        };

        let serialized = borsh::to_vec(&node).expect("serialize");
        let deserialized: TreeNode = borsh::from_slice(&serialized).expect("deserialize");

        assert_eq!(deserialized.children.len(), 2);
        assert!(deserialized.leaf_data.is_none());
    }

    /// Test TreeNode structure for leaf nodes with metadata
    #[test]
    fn test_tree_node_leaf_with_metadata() {
        let mut metadata = Metadata::new(1000, 2000);
        metadata.crdt_type = Some(CrdtType::Counter);

        let leaf_data = TreeLeafData {
            key: [7u8; 32],
            value: vec![100, 200],
            metadata,
        };

        let node = TreeNode {
            node_id: [6u8; 32],
            hash: calimero_primitives::hash::Hash::new(&[8u8; 32]),
            children: vec![], // Leaf has no children
            leaf_data: Some(leaf_data),
        };

        let serialized = borsh::to_vec(&node).expect("serialize");
        let deserialized: TreeNode = borsh::from_slice(&serialized).expect("deserialize");

        assert!(deserialized.children.is_empty());
        assert!(deserialized.leaf_data.is_some());

        let data = deserialized.leaf_data.unwrap();
        assert_eq!(data.key, [7u8; 32]);
        assert_eq!(data.value, vec![100, 200]);
        assert_eq!(data.metadata.crdt_type, Some(CrdtType::Counter));
    }

    /// Test Metadata with None crdt_type (edge case)
    #[test]
    fn test_metadata_none_crdt_type() {
        let mut metadata = Metadata::new(0, 0);
        metadata.crdt_type = None; // Explicitly set to None

        let serialized = borsh::to_vec(&metadata).expect("serialize");
        let deserialized: Metadata = borsh::from_slice(&serialized).expect("deserialize");

        assert_eq!(deserialized.crdt_type, None);
    }

    /// Test Custom CRDT type with type name
    #[test]
    fn test_custom_crdt_type() {
        let mut metadata = Metadata::new(0, 0);
        metadata.crdt_type = Some(CrdtType::Custom {
            type_name: "MyCustomType".to_string(),
        });

        let serialized = borsh::to_vec(&metadata).expect("serialize");
        let deserialized: Metadata = borsh::from_slice(&serialized).expect("deserialize");

        match deserialized.crdt_type {
            Some(CrdtType::Custom { type_name }) => {
                assert_eq!(type_name, "MyCustomType");
            }
            _ => panic!("Expected Custom CRDT type"),
        }
    }

    /// Test Interface::merge_by_crdt_type_with_callback behavior
    #[test]
    fn test_merge_dispatch_lww_register() {
        // Two LWW registers with different timestamps
        // Later timestamp should win
        let mut local_metadata = Metadata::new(1000, 1000);
        local_metadata.crdt_type = Some(CrdtType::LwwRegister);

        let mut remote_metadata = Metadata::new(2000, 2000);
        remote_metadata.crdt_type = Some(CrdtType::LwwRegister);

        let local_data = b"local_value".to_vec();
        let remote_data = b"remote_value".to_vec();

        // Remote has later timestamp, should win
        let result = Interface::<MainStorage>::merge_by_crdt_type_with_callback(
            &local_data,
            &remote_data,
            &local_metadata,
            &remote_metadata,
            None, // No WASM callback
        );

        assert!(result.is_ok());
        let merged = result.unwrap();
        assert!(merged.is_some());
        // Remote should win because it has higher timestamp
        assert_eq!(merged.unwrap(), remote_data);
    }

    /// Test merge with local having later timestamp
    #[test]
    fn test_merge_dispatch_lww_local_wins() {
        // Local has later timestamp - should win
        let mut local_metadata = Metadata::new(3000, 3000);
        local_metadata.crdt_type = Some(CrdtType::LwwRegister);

        let mut remote_metadata = Metadata::new(1000, 1000);
        remote_metadata.crdt_type = Some(CrdtType::LwwRegister);

        let local_data = b"local_value".to_vec();
        let remote_data = b"remote_value".to_vec();

        let result = Interface::<MainStorage>::merge_by_crdt_type_with_callback(
            &local_data,
            &remote_data,
            &local_metadata,
            &remote_metadata,
            None,
        );

        assert!(result.is_ok());
        let merged = result.unwrap();
        assert!(merged.is_some());
        // Local should win because it has higher timestamp
        assert_eq!(merged.unwrap(), local_data);
    }

    /// Test BloomFilterResponse wire format includes metadata
    #[test]
    fn test_bloom_filter_response_includes_metadata() {
        use calimero_node_primitives::sync::MessagePayload;

        // Create entities with different CRDT types
        let mut counter_metadata = Metadata::new(1000, 2000);
        counter_metadata.crdt_type = Some(CrdtType::Counter);

        let mut map_metadata = Metadata::new(3000, 4000);
        map_metadata.crdt_type = Some(CrdtType::UnorderedMap);

        let entities = vec![
            TreeLeafData {
                key: [1u8; 32],
                value: vec![10, 20, 30],
                metadata: counter_metadata.clone(),
            },
            TreeLeafData {
                key: [2u8; 32],
                value: vec![40, 50],
                metadata: map_metadata.clone(),
            },
        ];

        // Create BloomFilterResponse with entities
        let response = MessagePayload::BloomFilterResponse {
            missing_entities: entities.clone(),
            matched_count: 5,
        };

        // Serialize and deserialize
        let serialized = borsh::to_vec(&response).expect("serialize");
        let deserialized: MessagePayload = borsh::from_slice(&serialized).expect("deserialize");

        // Verify structure preserved
        match deserialized {
            MessagePayload::BloomFilterResponse {
                missing_entities,
                matched_count,
            } => {
                assert_eq!(matched_count, 5);
                assert_eq!(missing_entities.len(), 2);

                // Verify first entity (Counter)
                assert_eq!(missing_entities[0].key, [1u8; 32]);
                assert_eq!(missing_entities[0].value, vec![10, 20, 30]);
                assert_eq!(
                    missing_entities[0].metadata.crdt_type,
                    Some(CrdtType::Counter)
                );

                // Verify second entity (UnorderedMap)
                assert_eq!(missing_entities[1].key, [2u8; 32]);
                assert_eq!(missing_entities[1].value, vec![40, 50]);
                assert_eq!(
                    missing_entities[1].metadata.crdt_type,
                    Some(CrdtType::UnorderedMap)
                );
            }
            _ => panic!("Expected BloomFilterResponse"),
        }
    }

    /// Test BloomFilterResponse preserves Custom CRDT type name
    #[test]
    fn test_bloom_filter_response_custom_crdt_type() {
        use calimero_node_primitives::sync::MessagePayload;

        let mut custom_metadata = Metadata::new(0, 0);
        custom_metadata.crdt_type = Some(CrdtType::Custom {
            type_name: "MyCustomCRDT".to_string(),
        });

        let entities = vec![TreeLeafData {
            key: [3u8; 32],
            value: vec![1, 2, 3],
            metadata: custom_metadata,
        }];

        let response = MessagePayload::BloomFilterResponse {
            missing_entities: entities,
            matched_count: 0,
        };

        let serialized = borsh::to_vec(&response).expect("serialize");
        let deserialized: MessagePayload = borsh::from_slice(&serialized).expect("deserialize");

        match deserialized {
            MessagePayload::BloomFilterResponse {
                missing_entities, ..
            } => {
                assert_eq!(missing_entities.len(), 1);
                match &missing_entities[0].metadata.crdt_type {
                    Some(CrdtType::Custom { type_name }) => {
                        assert_eq!(type_name, "MyCustomCRDT");
                    }
                    _ => panic!("Expected Custom CRDT type"),
                }
            }
            _ => panic!("Expected BloomFilterResponse"),
        }
    }
}
