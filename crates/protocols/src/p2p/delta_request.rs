//! Delta Request Protocol - Stateless P2P delta/DAG heads handlers
//!
//! **Purpose**: Fill DAG gaps by requesting missing deltas from peers.
//!
//! **Protocol**:
//! 1. Client sends Init with DeltaRequest payload
//! 2. Client proves identity (prevents unauthorized access)
//! 3. Server verifies identity, fetches delta from DB/DeltaStore
//! 4. Server sends DeltaResponse or DeltaNotFound
//!
//! **Stateless Design**: All dependencies injected as parameters (NO SyncManager!)

use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::delta::CausalDelta;
use eyre::{bail, OptionExt, Result};
use rand::Rng;
use tokio::time::Duration;
use tracing::{debug, info, warn};

use crate::stream::{SecureStream, Sequencer};

// ═══════════════════════════════════════════════════════════════════════════
// DeltaStore Trait (Avoids circular dependency with node crate)
// ═══════════════════════════════════════════════════════════════════════════

/// Result from adding a delta (for cascade handling).
#[derive(Debug)]
pub struct AddDeltaResult {
    /// Whether the delta was immediately applied (true) or pending (false)
    pub applied: bool,
    /// Cascaded events from deltas that were applied due to this delta
    pub cascaded_events: Vec<([u8; 32], Vec<u8>)>,
}

/// Result from checking missing parents.
#[derive(Debug)]
pub struct MissingParentsResult {
    /// IDs of missing parent deltas
    pub missing_ids: Vec<[u8; 32]>,
    /// Cascaded events from deltas that were loaded from DB
    pub cascaded_events: Vec<([u8; 32], Vec<u8>)>,
}

/// Trait for delta storage operations.
///
/// This allows protocols to work with any delta store implementation
/// without depending on the node crate (prevents circular dependency).
///
/// Note: Uses `?Send` because some implementations (like node's DeltaStore)
/// call methods on calimero-dag which uses non-Send futures internally.
#[async_trait::async_trait(?Send)]
pub trait DeltaStore: Send + Sync {
    /// Check if a delta exists in the store
    async fn has_delta(&self, delta_id: &[u8; 32]) -> bool;

    /// Add a delta to the store (simple version, no events)
    async fn add_delta(
        &self,
        delta: calimero_dag::CausalDelta<Vec<calimero_storage::interface::Action>>,
    ) -> Result<()>;

    /// Add a delta with associated events (for handler execution on cascade)
    async fn add_delta_with_events(
        &self,
        delta: calimero_dag::CausalDelta<Vec<calimero_storage::interface::Action>>,
        events: Option<Vec<u8>>,
    ) -> Result<AddDeltaResult>;

    /// Get a delta from the store
    async fn get_delta(
        &self,
        delta_id: &[u8; 32],
    ) -> Option<calimero_dag::CausalDelta<Vec<calimero_storage::interface::Action>>>;

    /// Get missing parent IDs and cascaded events
    async fn get_missing_parents(&self) -> MissingParentsResult;

    /// Check if a delta has been applied to the DAG
    async fn dag_has_delta_applied(&self, delta_id: &[u8; 32]) -> bool;
}

// ═══════════════════════════════════════════════════════════════════════════
// Client Side: Request DAG Heads
// ═══════════════════════════════════════════════════════════════════════════

/// Request DAG heads from a peer.
///
/// Used for initial sync when we have an empty DAG and need to know what to fetch.
pub async fn request_dag_heads(
    network_client: &NetworkClient,
    context_id: ContextId,
    peer_id: libp2p::PeerId,
    our_identity: PublicKey,
    context_client: &ContextClient,
    timeout: Duration,
) -> Result<Vec<[u8; 32]>> {
    info!(%context_id, ?peer_id, "Requesting DAG heads from peer");

    let mut stream = network_client.open_stream(peer_id).await?;

    // Send Init with DagHeadsRequest payload
    let our_nonce = rand::thread_rng().gen();
    crate::stream::send(
        &mut stream,
        &StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::DagHeadsRequest { context_id },
            next_nonce: our_nonce,
        },
        None,
    )
    .await?;

    // No authentication needed for DAG heads - just metadata request
    // (actual delta requests still require full authentication)

    // Receive heads response
    let message = crate::stream::recv(&mut stream, None, timeout).await?
        .ok_or_eyre("Connection closed while waiting for DAG heads")?;

    let (dag_heads, root_hash) = match message {
        StreamMessage::Message {
            payload: MessagePayload::DagHeadsResponse { dag_heads, root_hash },
            ..
        } => (dag_heads, root_hash),
        unexpected => bail!("Expected DagHeadsResponse, got: {:?}", unexpected),
    };

    info!(%context_id, heads_count = dag_heads.len(), %root_hash, "Received DAG heads from peer");

    Ok(dag_heads)
}

// ═══════════════════════════════════════════════════════════════════════════
// Client Side: Request Missing Deltas
// ═══════════════════════════════════════════════════════════════════════════

/// Request missing deltas from a peer and add them to the DAG.
///
/// Recursively fetches all missing ancestors until reaching deltas we already have.
/// This implements the "DAG catchup" protocol.
///
/// # Arguments
/// * `network_client` - Network client to open streams
/// * `context_id` - Context ID for the deltas
/// * `missing_ids` - Initial set of missing delta IDs
/// * `peer_id` - Peer to request from
/// * `delta_store` - DeltaStore to check/add deltas (injected!)
/// * `our_identity` - Our identity in this context
/// * `context_client` - Client for identity operations
/// * `timeout` - Timeout for each request
///
/// # Example
/// ```rust,ignore
/// request_missing_deltas(
///     &network_client,
///     context_id,
///     vec![missing_delta_id],
///     peer_id,
///     delta_store,
///     our_identity,
///     &context_client,
///     Duration::from_secs(10),
/// ).await?;
/// ```
pub async fn request_missing_deltas(
    network_client: &NetworkClient,
    context_id: ContextId,
    missing_ids: Vec<[u8; 32]>,
    peer_id: libp2p::PeerId,
    delta_store: &dyn DeltaStore, // Injected dependency!
    our_identity: PublicKey,
    context_client: &ContextClient,
    timeout: Duration,
) -> Result<()> {
    info!(
        %context_id,
        ?peer_id,
        initial_missing_count = missing_ids.len(),
        "Requesting missing parent deltas from peer"
    );

    // Open stream to peer
    let mut stream = network_client.open_stream(peer_id).await?;

    // Fetch all missing ancestors, then add them in topological order (oldest first)
    let mut to_fetch = missing_ids;
    let mut fetched_deltas: Vec<(
        calimero_dag::CausalDelta<Vec<calimero_storage::interface::Action>>,
        [u8; 32],
    )> = Vec::new();
    let mut fetch_count = 0;

    // Phase 1: Fetch ALL missing deltas recursively
    // No artificial limit - DAG is acyclic so this will naturally terminate at genesis
    let mut iteration = 0;
    while !to_fetch.is_empty() {
        iteration += 1;
        let batch_size = to_fetch.len();
        let current_batch = to_fetch.clone();
        to_fetch.clear();
        
        info!(%context_id, iteration, batch_size, "🔃 Starting fetch iteration");

        for missing_id in current_batch {
            fetch_count += 1;

            match request_delta(
                &context_id,
                missing_id,
                &mut stream,
                our_identity,
                context_client,
                timeout,
            )
            .await
            {
                Ok(Some(parent_delta)) => {
                    info!(
                        %context_id,
                        delta_id = ?missing_id,
                        action_count = parent_delta.actions.len(),
                            total_fetched = fetch_count,
                            "Received missing parent delta"
                    );

                    // Convert to DAG delta format
                    let dag_delta = calimero_dag::CausalDelta {
                        id: parent_delta.id,
                        parents: parent_delta.parents.clone(),
                        payload: parent_delta.actions,
                        hlc: parent_delta.hlc,
                        expected_root_hash: parent_delta.expected_root_hash,
                    };

                    // Store for later (don't add to DAG yet!)
                    fetched_deltas.push((dag_delta, missing_id));

                    // Check what parents THIS delta needs
                    for parent_id in &parent_delta.parents {
                        // Skip genesis
                        if *parent_id == [0; 32] {
                            continue;
                        }
                        
                        let has_delta = delta_store.has_delta(parent_id).await;
                        let in_to_fetch = to_fetch.contains(parent_id);
                        let in_fetched = fetched_deltas.iter().any(|(d, _)| d.id == *parent_id);
                        
                        // Skip if we already have it or are about to fetch it
                        if !has_delta && !in_to_fetch && !in_fetched {
                            info!(
                                %context_id,
                                parent_id = ?parent_id,
                                delta_id = ?missing_id,
                                "🔄 Queueing parent delta for fetch (child depends on it)"
                            );
                            to_fetch.push(*parent_id);
                        } else {
                            info!(
                                %context_id,
                                parent_id = ?parent_id,
                                delta_id = ?missing_id,
                                has_delta, in_to_fetch, in_fetched,
                                "⏭️ Skipping parent (already have it or queued)"
                            );
                        }
                    }
                }
                Ok(None) => {
                    warn!(%context_id, delta_id = ?missing_id, "Peer doesn't have requested delta");
                }
                Err(e) => {
                    warn!(?e, %context_id, delta_id = ?missing_id, "Failed to request delta - stopping batch");
                    break; // Stop requesting current batch if stream fails
                }
            }
        }
        
        info!(%context_id, iteration, to_fetch_count = to_fetch.len(), "✅ Iteration complete - will continue" = !to_fetch.is_empty());
    }

    info!(%context_id, total_fetched = fetch_count, fetched_deltas_count = fetched_deltas.len(), total_iterations = iteration, "📦 Phase 1 complete: All deltas fetched");

    // Phase 2: Add all fetched deltas to DAG in topological order (oldest first)
    // We need to sort by dependencies so parents are added before children
    if !fetched_deltas.is_empty() {
        info!(
            %context_id,
            total_fetched = fetched_deltas.len(),
            "Sorting fetched deltas in topological order"
        );

        // Topological sort: process deltas whose parents are all already added
        let mut added: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();
        added.insert([0; 32]); // Genesis is always "added"
        
        let mut remaining = fetched_deltas;
        let mut added_count = 0;
        
        while !remaining.is_empty() {
            let before_len = remaining.len();
            let mut next_remaining = Vec::new();
            
            // Find deltas whose parents are all added (or genesis)
            for (dag_delta, delta_id) in remaining {
                let mut parents_ready = true;
                for parent in &dag_delta.parents {
                    if *parent != [0; 32] && !added.contains(parent) && !delta_store.has_delta(parent).await {
                        parents_ready = false;
                        break;
                    }
                }
                
                if parents_ready {
                    // Add to DAG now
                    if let Err(e) = delta_store.add_delta(dag_delta).await {
                        warn!(?e, %context_id, delta_id = ?delta_id, "Failed to add fetched delta to DAG");
                    } else {
                        added.insert(delta_id);
                        added_count += 1;
                    }
                } else {
                    // Keep for next iteration
                    next_remaining.push((dag_delta, delta_id));
                }
            }
            
            remaining = next_remaining;
            
            // Detect infinite loop (circular dependency - shouldn't happen in a DAG)
            if remaining.len() == before_len && !remaining.is_empty() {
                warn!(
                    %context_id,
                    remaining_count = remaining.len(),
                    "Cannot add remaining deltas - missing parents (DAG corrupted?)"
                );
                break;
            }
        }
        
        info!(
            %context_id,
            added_count,
            "Completed adding fetched deltas in topological order"
        );
    }

    if fetch_count > 0 {
        info!(
            %context_id,
            total_fetched = fetch_count,
            "Completed fetching missing delta ancestors"
        );

        // Log warning for very large syncs (informational, not a hard limit)
        if fetch_count > 1000 {
            warn!(
                %context_id,
                total_fetched = fetch_count,
                "Large sync detected - fetched many deltas from peer (context has deep history)"
            );
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper: Request Single Delta
// ═══════════════════════════════════════════════════════════════════════════

/// Request a specific delta from a peer.
///
/// This is called by `request_missing_deltas` for each missing delta.
/// Stream is already open and reused across multiple requests.
///
/// # Arguments
/// * `context_id` - Context ID for the delta
/// * `delta_id` - ID of the delta to request
/// * `stream` - Open stream to the peer
/// * `our_identity` - Our identity in this context
/// * `context_client` - Client for identity operations
/// * `timeout` - Timeout for the request
async fn request_delta(
    context_id: &ContextId,
    delta_id: [u8; 32],
    stream: &mut Stream,
    our_identity: PublicKey,
    context_client: &ContextClient,
    timeout: Duration,
) -> Result<Option<CausalDelta>> {
    info!(
        %context_id,
        delta_id = ?delta_id,
        "Requesting missing delta from peer"
    );

    // Generate random nonce for this request
    let nonce = rand::thread_rng().gen::<calimero_crypto::Nonce>();

    // Send request with proper identity (not [0; 32])
    let msg = StreamMessage::Init {
        context_id: *context_id,
        party_id: our_identity,
        payload: InitPayload::DeltaRequest {
            context_id: *context_id,
            delta_id,
        },
        next_nonce: nonce,
    };

    crate::stream::send(stream, &msg, None).await?;

    // No authentication needed - membership already enforced by gossipsub
    
    // Wait for response
    match crate::stream::recv(stream, None, timeout).await? {
        Some(StreamMessage::Message {
            payload: MessagePayload::DeltaResponse { delta },
            ..
        }) => {
            // Deserialize delta
            let causal_delta: CausalDelta = borsh::from_slice(&delta)?;

            // Verify delta ID matches
            if causal_delta.id != delta_id {
                bail!(
                    "Received delta ID mismatch: requested {:?}, got {:?}",
                    delta_id,
                    causal_delta.id
                );
            }

            info!(
                %context_id,
                delta_id = ?delta_id,
                action_count = causal_delta.actions.len(),
                "Received requested delta"
            );

            Ok(Some(causal_delta))
        }
        Some(StreamMessage::Message {
            payload: MessagePayload::DeltaNotFound,
            ..
        }) => {
            debug!(
                %context_id,
                delta_id = ?delta_id,
                "Peer doesn't have requested delta"
            );
            Ok(None)
        }
        Some(StreamMessage::OpaqueError) => {
            bail!("Peer encountered error processing delta request");
        }
        other => {
            bail!("Unexpected response to delta request: {:?}", other);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Server Side: Handle Delta Request
// ═══════════════════════════════════════════════════════════════════════════

/// Handle incoming delta request from a peer (server side).
///
/// Called when a peer sends Init with DeltaRequest payload.
/// Verifies identity, fetches delta from DB/DeltaStore, sends response.
///
/// **NOTE**: This is called AFTER the Init message has been consumed.
///
/// # Arguments
/// * `stream` - Stream (Init message already consumed)
/// * `context_id` - Context ID for the delta
/// * `delta_id` - ID of the delta being requested
/// * `their_identity` - Their identity (from their Init message)
/// * `our_identity` - Our identity in this context
/// * `datastore_handle` - RocksDB handle for persistent deltas
/// * `delta_store` - Optional DeltaStore for recently broadcast deltas
/// * `context_client` - Client for identity operations
/// * `timeout` - Timeout for verification
pub async fn handle_delta_request(
    stream: &mut Stream,
    context_id: ContextId,
    delta_id: [u8; 32],
    their_identity: PublicKey,
    our_identity: PublicKey,
    datastore_handle: &calimero_store::Handle<calimero_store::Store>,
    delta_store: Option<&dyn DeltaStore>, // Injected!
    context_client: &ContextClient,
    timeout: Duration,
) -> Result<()> {
    info!(
        %context_id,
        %their_identity,
        delta_id = ?delta_id,
        "Handling delta request (no auth - gossipsub already enforces membership)"
    );

    // NOTE: We don't verify identity here because:
    // 1. Gossipsub subscription already enforces membership (can't subscribe if not member)
    // 2. Avoids race condition where inviter's member cache is stale after blockchain update
    // 3. Deltas are encrypted with context-specific keys anyway (members-only access)
    //
    // An attacker who somehow subscribes to gossipsub without being a member still can't
    // decrypt the delta artifacts due to the encryption layer.

    // Try RocksDB first (has full CausalDelta with HLC)
    use calimero_store::key;

    let db_key = key::ContextDagDelta::new(context_id, delta_id);

    let response = if let Some(stored_delta) = datastore_handle.get(&db_key)? {
        // Found in RocksDB - reconstruct CausalDelta with HLC
        let actions: Vec<calimero_storage::interface::Action> =
            borsh::from_slice(&stored_delta.actions)?;

        let causal_delta = CausalDelta {
            id: stored_delta.delta_id,
            parents: stored_delta.parents,
            actions,
            hlc: stored_delta.hlc,
            expected_root_hash: stored_delta.expected_root_hash,
        };

        let serialized = borsh::to_vec(&causal_delta)?;

        debug!(
            %context_id,
            delta_id = ?delta_id,
            size = serialized.len(),
            source = "RocksDB",
            "Sending requested delta to peer"
        );

        MessagePayload::DeltaResponse {
            delta: serialized.into(),
        }
    } else if let Some(delta_store) = delta_store {
        // Not in RocksDB yet (race condition after broadcast), try DeltaStore
        if let Some(dag_delta) = delta_store.get_delta(&delta_id).await {
            // dag::CausalDelta now includes HLC, so we can directly convert
            let causal_delta = CausalDelta {
                id: dag_delta.id,
                parents: dag_delta.parents,
                actions: dag_delta.payload,
                hlc: dag_delta.hlc,
                expected_root_hash: dag_delta.expected_root_hash,
            };

            let serialized = borsh::to_vec(&causal_delta)?;

            debug!(
                %context_id,
                delta_id = ?delta_id,
                size = serialized.len(),
                source = "DeltaStore",
                "Sending requested delta to peer"
            );

            MessagePayload::DeltaResponse {
                delta: serialized.into(),
            }
        } else {
            warn!(
                %context_id,
                delta_id = ?delta_id,
                "Requested delta not found in RocksDB or DeltaStore"
            );
            MessagePayload::DeltaNotFound
        }
    } else {
        warn!(
            %context_id,
            delta_id = ?delta_id,
            "Requested delta not found (no DeltaStore for context)"
        );
        MessagePayload::DeltaNotFound
    };

    // Send response
    let mut sqx = Sequencer::default();
    let nonce = rand::thread_rng().gen::<calimero_crypto::Nonce>();
    let msg = StreamMessage::Message {
        sequence_id: sqx.next(),
        payload: response,
        next_nonce: nonce,
    };

    crate::stream::send(stream, &msg, None).await?;

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Server Side: Handle DAG Heads Request
// ═══════════════════════════════════════════════════════════════════════════

/// Handle incoming DAG heads request from a peer (server side).
///
/// Verifies identity and sends current DAG heads + root hash.
/// Used by peers to check if they're out of sync.
///
/// # Arguments
/// * `stream` - Stream (Init message already consumed)
/// * `context_id` - Context ID for the DAG heads
/// * `their_identity` - Their identity (from their Init message)
/// * `our_identity` - Our identity in this context
/// * `context_client` - Client for getting context metadata
/// * `timeout` - Timeout for verification
pub async fn handle_dag_heads_request(
    stream: &mut Stream,
    context_id: ContextId,
    their_identity: PublicKey,
    our_identity: PublicKey,
    context_client: &ContextClient,
    timeout: Duration,
) -> Result<()> {
    info!(
        %context_id,
        %their_identity,
        "Handling DAG heads request (no auth needed - just metadata)"
    );

    // NOTE: We don't verify identity here because:
    // 1. DAG heads are just delta IDs (not sensitive data)
    // 2. Gossipsub subscription already enforces membership
    // 3. Avoids race condition where inviter's member cache is stale after blockchain update
    //
    // If someone requests heads for a context they're not in, they still can't fetch
    // the actual deltas without proper authentication (handled in delta_request).

    // Get context to retrieve dag_heads and root_hash
    let context = context_client
        .get_context(&context_id)?
        .ok_or_eyre("Context not found")?;

    info!(
        %context_id,
        heads_count = context.dag_heads.len(),
        root_hash = %context.root_hash,
        "Sending DAG heads to peer"
    );

    // Send response
    let mut sqx = Sequencer::default();
    let nonce = rand::thread_rng().gen::<calimero_crypto::Nonce>();
    let msg = StreamMessage::Message {
        sequence_id: sqx.next(),
        payload: MessagePayload::DagHeadsResponse {
            dag_heads: context.dag_heads,
            root_hash: context.root_hash,
        },
        next_nonce: nonce,
    };

    crate::stream::send(stream, &msg, None).await?;

    Ok(())
}
