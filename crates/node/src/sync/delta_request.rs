//! Delta request protocol for DAG gap filling
//!
//! When a node receives a delta with missing parents, it uses this protocol
//! to request the missing deltas from peers.
use calimero_context::group_store::NamespaceRepository;
use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::delta::CausalDelta;
use eyre::{bail, OptionExt, Result};
use tracing::{debug, error, info, warn};

use super::manager::SyncManager;
use super::tracking::Sequencer;

/// Maximum number of deltas to fetch recursively in a single sync operation.
/// This prevents OOM where a peer sends a delta with an deep chain with many deltas.
/// TODO: adjust this number after the benchmarks.
const MAX_DELTA_FETCH_LIMIT: usize = 3000;
const DELTA_WARN_LIMIT: usize = 1000;
const GENESIS_DELTA_ID: [u8; 32] = [0u8; 32];

/// Sentinel `author_id` the DAG-catchup responder uses to serve the
/// genesis delta on the wire. `create_context` persists the genesis
/// row with `author_id: None` (genesis predates any governance op so
/// there's no real author to verify), but the wire format requires
/// `author_id: PublicKey`. The all-zeros pubkey is never a valid
/// signing key, so it can't collide with a real author claim;
/// receivers detect it via [`is_genesis_author_sentinel`] and skip
/// every author-keyed check (signature verify, `is_read_only`,
/// `membership_status_at`, `GroupIdCheck`) that doesn't apply to
/// genesis. Without this carve-out late joiners stall on missing
/// genesis ancestors.
pub(crate) const GENESIS_AUTHOR_SENTINEL: [u8; 32] = [0u8; 32];

/// Construct the genesis sentinel as a `PublicKey` for the responder.
pub(crate) fn genesis_author_sentinel() -> PublicKey {
    PublicKey::from(GENESIS_AUTHOR_SENTINEL)
}

/// Receivers use this to bail out of author-keyed checks before
/// running them — a positive result means the delta on the wire is
/// the genesis ancestor and the checks below either don't apply (no
/// author to verify) or would always reject (sentinel isn't a real
/// member of any group).
pub(crate) fn is_genesis_author_sentinel(author: &PublicKey) -> bool {
    let bytes: &[u8; 32] = author.as_ref();
    bytes == &GENESIS_AUTHOR_SENTINEL
}

/// What `request_delta` returns when the peer had the delta: the
/// payload plus the envelope metadata the caller needs to run the same
/// anti-impersonation + cross-DAG membership check that the head-pull
/// path in `request_dag_heads_and_sync` runs.
///
/// Fields mirror `MessagePayload::DeltaResponse` exactly; the only
/// difference is `governance_position_blob` is owned (`Vec<u8>`)
/// rather than `Cow<'_, [u8]>` so it can cross the await boundary
/// without holding onto the stream's buffer.
pub(crate) struct FetchedDelta {
    pub delta: CausalDelta,
    pub author_id: PublicKey,
    pub governance_position_blob: Option<Vec<u8>>,
    pub delta_signature: Option<[u8; 64]>,
}

/// Outcome of verifying a parent delta pulled in Phase 2 of DAG-catchup.
///
/// Mirrors the chain at `manager/mod.rs:1681` (head-pull) — decode the
/// governance position, verify the envelope signature, check membership
/// at the cited cut. Each per-delta rejection is a `Skip`, NOT a fatal
/// error: one malformed or malicious response from a peer can't poison
/// the rest of the catchup batch.
enum VerifiedParent {
    /// Verified. Persist the delta with the decoded position (when
    /// present) and the wire-received author + signature.
    Apply {
        position: Option<calimero_context_config::types::GovernancePosition>,
    },
    /// Rejected — drop this delta and continue with the next one.
    Skip,
}

/// Run the same chain as `request_dag_heads_and_sync`'s head-pull
/// verify block: decode position → verify signature → check
/// membership_status_at. Per-delta rejections are `Skip`; a stream-
/// fatal error returns `Err` (none of the inner checks are
/// stream-fatal, so this path only returns `Ok(...)` in practice).
fn verify_fetched_parent(
    context_id: &ContextId,
    delta_id: [u8; 32],
    fetched: &FetchedDelta,
    datastore: &calimero_store::Store,
) -> VerifiedParent {
    use calimero_context::group_store::{membership_status_at, MembershipStatus};
    use calimero_context_config::types::GovernancePosition;

    // Genesis carve-out: the responder serves the genesis delta with
    // the all-zeros sentinel `author_id` because the wire requires an
    // author but genesis predates any governance op. Skip every
    // author-keyed check — none of them apply to genesis.
    if is_genesis_author_sentinel(&fetched.author_id) {
        debug!(
            %context_id,
            delta_id = ?delta_id,
            "DAG-catchup parent-pull: accepting genesis delta via author sentinel"
        );
        return VerifiedParent::Apply { position: None };
    }

    let pos = match fetched
        .governance_position_blob
        .as_deref()
        .map(borsh::from_slice::<GovernancePosition>)
        .transpose()
    {
        Ok(p) => p,
        Err(e) => {
            warn!(
                %context_id,
                author = %fetched.author_id,
                delta_id = ?delta_id,
                %e,
                "DAG-catchup parent-pull: failed to decode governance_position; \
                 skipping this delta and continuing"
            );
            return VerifiedParent::Skip;
        }
    };

    if let Some(ref sig) = fetched.delta_signature {
        if let Err(err) = calimero_node_primitives::sync::delta_auth::verify_delta_signature(
            *context_id,
            delta_id,
            fetched.author_id,
            pos.as_ref(),
            sig,
        ) {
            warn!(
                %context_id,
                author = %fetched.author_id,
                delta_id = ?delta_id,
                %err,
                "DAG-catchup parent-pull: rejecting delta — envelope signature \
                 verification failed"
            );
            return VerifiedParent::Skip;
        }
    }

    // Anti-bypass parity (same as the head-pull path in
    // `request_dag_heads_and_sync`): confirm the claimed position's
    // group matches the context's owning group, or that no position is
    // claimed for a non-group context. Catches the
    // `GroupContextNoPosition`, `NonGroupContextWithPosition`, and
    // `Mismatch` bypasses described on `GroupIdCheck`.
    {
        use crate::handlers::state_delta::{
            verify_position_group_id_matches_context, GroupIdCheck,
        };
        match verify_position_group_id_matches_context(
            datastore,
            context_id,
            pos.as_ref().map(|p| p.group_id),
        ) {
            GroupIdCheck::Match | GroupIdCheck::NonGroupOk => {}
            GroupIdCheck::GroupContextNoPosition { owning } => {
                warn!(
                    %context_id,
                    author = %fetched.author_id,
                    delta_id = ?delta_id,
                    owning_group = ?owning,
                    "DAG-catchup parent-pull: rejecting delta — context is owned by a \
                     group but delta carries no governance_position"
                );
                return VerifiedParent::Skip;
            }
            GroupIdCheck::NonGroupContextWithPosition { claimed } => {
                warn!(
                    %context_id,
                    author = %fetched.author_id,
                    delta_id = ?delta_id,
                    claimed_group = ?claimed,
                    "DAG-catchup parent-pull: rejecting delta — delta claims a \
                     governance position but context is not in any group"
                );
                return VerifiedParent::Skip;
            }
            GroupIdCheck::Mismatch { owning, claimed } => {
                warn!(
                    %context_id,
                    author = %fetched.author_id,
                    delta_id = ?delta_id,
                    owning_group = ?owning,
                    claimed_group = ?claimed,
                    "DAG-catchup parent-pull: rejecting delta — governance_position \
                     references a different group than the context's owning group"
                );
                return VerifiedParent::Skip;
            }
            GroupIdCheck::LookupError(err) => {
                warn!(
                    %context_id,
                    author = %fetched.author_id,
                    delta_id = ?delta_id,
                    %err,
                    "DAG-catchup parent-pull: skipping delta — group lookup failed \
                     during anti-bypass check"
                );
                return VerifiedParent::Skip;
            }
        }
    }

    // ReadOnly check — parity with the gossip apply path.
    // `membership_status_at` treats ReadOnly as `Member(ReadOnly)`,
    // so a ReadOnly identity's delta would slip past the cross-DAG
    // check on the catchup path even though gossip rejects it.
    // Mirror the gate `apply_authorized_state_delta` uses.
    if NamespaceRepository::new(datastore)
        .is_read_only_for_context(&context_id, &fetched.author_id)
        .unwrap_or(false)
    {
        warn!(
            %context_id,
            author = %fetched.author_id,
            delta_id = ?delta_id,
            "DAG-catchup parent-pull: rejecting delta from ReadOnly member"
        );
        return VerifiedParent::Skip;
    }

    if let Some(ref pos_ref) = pos {
        match membership_status_at(datastore, &fetched.author_id, pos_ref) {
            Ok(MembershipStatus::Member(_)) => {
                // Authorized at the cited cut — proceed.
            }
            Ok(MembershipStatus::Removed { last_role }) => {
                warn!(
                    %context_id,
                    author = %fetched.author_id,
                    delta_id = ?delta_id,
                    last_role = ?last_role,
                    "DAG-catchup parent-pull: rejecting delta — author was removed \
                     at the cited governance cut"
                );
                return VerifiedParent::Skip;
            }
            Ok(MembershipStatus::NeverMember) => {
                warn!(
                    %context_id,
                    author = %fetched.author_id,
                    delta_id = ?delta_id,
                    "DAG-catchup parent-pull: rejecting delta — author was never \
                     a member at the cited governance cut"
                );
                return VerifiedParent::Skip;
            }
            Ok(MembershipStatus::Unknown { needed }) => {
                warn!(
                    %context_id,
                    author = %fetched.author_id,
                    delta_id = ?delta_id,
                    needed = ?needed,
                    "DAG-catchup parent-pull: skipping delta — governance cut not \
                     locally known; will re-attempt on next sync tick"
                );
                return VerifiedParent::Skip;
            }
            Err(e) => {
                warn!(
                    %context_id,
                    author = %fetched.author_id,
                    delta_id = ?delta_id,
                    error = %e,
                    "DAG-catchup parent-pull: skipping delta — membership_status_at \
                     failed"
                );
                return VerifiedParent::Skip;
            }
        }
    }

    VerifiedParent::Apply { position: pos }
}

/// Register one chunk of fetched, already-verified deltas into the DAG via the
/// batch API. Mirrors the single-delta path's warn-and-continue: a failed
/// commit leaves the chunk unpersisted and the next sync re-fetches it.
async fn flush_delta_batch(
    delta_store: &crate::delta_store::DeltaStore,
    context_id: &ContextId,
    batch: Vec<crate::delta_store::BatchDeltaInput>,
) {
    if batch.is_empty() {
        return;
    }
    let batch_count = batch.len();
    if let Err(e) = delta_store.add_deltas_batch(batch).await {
        warn!(
            ?e,
            %context_id,
            batch_count,
            "Failed to persist fetched delta batch to DAG"
        );
    }
}

impl SyncManager {
    /// Request missing deltas from a peer and add them to the DAG
    ///
    /// Recursively fetches all missing ancestors until reaching deltas we already have.
    pub async fn request_missing_deltas(
        &self,
        context_id: ContextId,
        missing_ids: Vec<[u8; 32]>,
        source: libp2p::PeerId,
        delta_store: crate::delta_store::DeltaStore,
        our_identity: PublicKey,
    ) -> Result<()> {
        info!(
            %context_id,
            ?source,
            initial_missing_count = missing_ids.len(),
            "Requesting missing parent deltas from peer"
        );

        // Open stream to peer
        let mut stream = self.sync_network.open_stream(source).await?;

        // Fetch all missing ancestors, then add them in topological order (oldest first)
        let mut to_fetch = missing_ids.clone();
        let mut fetch_count = 0;

        // Track visited IDs to prevent cycles/loops from malicious peers
        let mut visited_ids = std::collections::HashSet::new();
        // Initialize visited IDs with the starting set to ensure we don't re-queue them if they appear as parents.
        for id in &missing_ids {
            visited_ids.insert(*id);
        }

        // Verified deltas waiting to be registered into the DAG. Accumulated
        // across fetches and flushed in `DELTA_BATCH_MAX` chunks so N deltas
        // take one DAG write-lock scope + one atomic persist instead of N.
        // Bounding the buffer also keeps memory in check (we never hold more
        // than one chunk's payloads beyond what's already in flight).
        let mut delta_batch: Vec<crate::delta_store::BatchDeltaInput> = Vec::new();

        // Phase 1: Fetch ALL missing deltas recursively
        // No artificial limit - DAG is acyclic so this will naturally terminate at genesis
        while !to_fetch.is_empty() {
            // Drain the current batch so we don't hold it in memory while fetching new ones
            let current_batch = std::mem::take(&mut to_fetch);

            for missing_id in current_batch {
                // Enforce hard limit on fetched deltas count
                if fetch_count >= MAX_DELTA_FETCH_LIMIT {
                    warn!(
                        %context_id,
                        fetch_count,
                        limit = MAX_DELTA_FETCH_LIMIT,
                        "Exceeded maximum delta fetch limit. The sync gap is too large."
                    );

                    // Flush what we've buffered so far before bailing — those
                    // deltas are verified and shouldn't be dropped just because
                    // the gap is too large to finish.
                    flush_delta_batch(&delta_store, &context_id, std::mem::take(&mut delta_batch))
                        .await;

                    // Stop syncing. Progress so far is saved in DeltaStore (Pending).
                    return Ok(());
                }

                fetch_count += 1;

                match self
                    .request_delta(&context_id, missing_id, &mut stream, our_identity)
                    .await
                {
                    Ok(Some(fetched)) => {
                        info!(
                            %context_id,
                            delta_id = ?missing_id,
                            action_count = fetched.delta.actions.len(),
                            total_fetched = fetch_count,
                            "Received missing parent delta"
                        );

                        // Anti-bypass parity with the head-pull path:
                        // decode the governance position, verify the
                        // envelope signature, and check membership at
                        // the cited cut BEFORE persisting. Without this,
                        // parent-pull was a back door for revoked-author
                        // deltas to reach the DAG.
                        let datastore = self.context_client.datastore_handle().into_inner();
                        let position = match verify_fetched_parent(
                            &context_id,
                            missing_id,
                            &fetched,
                            &datastore,
                        ) {
                            VerifiedParent::Apply { position } => position,
                            VerifiedParent::Skip => continue,
                        };

                        // Check what parents THIS delta needs (identify grandparents).
                        // We also check local storage to avoid re-fetching known deltas.
                        for parent_id in &fetched.delta.parents {
                            // Skip genesis
                            if *parent_id == GENESIS_DELTA_ID {
                                continue;
                            }

                            // Cycle & Duplicate Detection
                            // We attempt to insert into `visited`.
                            // If insert returns true, it's a NEW ID we haven't processed or queued yet.
                            // Then, verify and add to the queue only if we don't have it in Delta
                            // Store (should be stored on disk in future).
                            if visited_ids.insert(*parent_id)
                                && !delta_store.has_delta(parent_id).await
                            {
                                to_fetch.push(*parent_id);
                            }
                        }

                        // Convert to DAG delta format
                        let dag_delta = calimero_dag::CausalDelta {
                            id: fetched.delta.id,
                            parents: fetched.delta.parents.clone(),
                            payload: fetched.delta.actions.clone(),
                            hlc: fetched.delta.hlc,
                            expected_root_hash: fetched.delta.expected_root_hash,
                            kind: calimero_dag::DeltaKind::Regular,
                        };

                        // Persist with the verified envelope so subsequent
                        // DAG-catchup serves from this node carry the
                        // same metadata downstream peers will check.
                        // Re-serialise the position from the typed value
                        // we just decoded — guarantees the stored blob
                        // matches what `membership_status_at` was run
                        // against (and what `verify_delta_signature`
                        // verified against).
                        let governance_position_blob =
                            position.as_ref().and_then(|gp| borsh::to_vec(gp).ok());
                        delta_batch.push(crate::delta_store::BatchDeltaInput {
                            delta: dag_delta,
                            events: None,
                            author_id: Some(fetched.author_id),
                            governance_position_blob,
                            delta_signature: fetched.delta_signature,
                        });
                        if delta_batch.len() >= crate::delta_store::DELTA_BATCH_MAX {
                            flush_delta_batch(
                                &delta_store,
                                &context_id,
                                std::mem::take(&mut delta_batch),
                            )
                            .await;
                        }
                    }
                    Ok(None) => {
                        warn!(%context_id, delta_id = ?missing_id, "Peer doesn't have requested delta");
                    }
                    Err(e) => {
                        error!(?e, %context_id, delta_id = ?missing_id, "Failed to request delta");

                        // Stop requesting if stream fails
                        // TODO: in future, this might also mean that the `stream` has some
                        // critical error and, perhaps, we need to set a limit of failures for the
                        // specific peer (stream).
                        break;
                    }
                }
            }
        }

        // Register any deltas left in the buffer below the chunk threshold.
        flush_delta_batch(&delta_store, &context_id, std::mem::take(&mut delta_batch)).await;

        if fetch_count > 0 {
            info!(
                %context_id,
                total_fetched = fetch_count,
                "Completed fetching missing delta ancestors"
            );

            // Log warning for very large syncs (informational, not a hard limit)
            if fetch_count > DELTA_WARN_LIMIT {
                warn!(
                    %context_id,
                    total_fetched = fetch_count,
                    "Large sync detected - fetched many deltas from peer (context has deep history)"
                );
            }
        }

        Ok(())
    }

    /// Request a specific delta from a peer
    ///
    /// Returns the full envelope (delta + author + governance position +
    /// envelope signature) so the caller can run the same membership /
    /// signature checks the head-pull path in
    /// `request_dag_heads_and_sync` runs. Without that, the parent-pull
    /// path would bypass the anti-impersonation and revoked-author
    /// gates the head-pull path enforces.
    pub(crate) async fn request_delta(
        &self,
        context_id: &ContextId,
        delta_id: [u8; 32],
        stream: &mut Stream,
        our_identity: PublicKey,
    ) -> Result<Option<FetchedDelta>> {
        info!(
            %context_id,
            delta_id = ?delta_id,
            "Requesting missing delta from peer"
        );

        // Send request with proper identity (not [0; 32])
        let msg = StreamMessage::Init {
            context_id: *context_id,
            party_id: our_identity,
            payload: InitPayload::DeltaRequest {
                context_id: *context_id,
                delta_id,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;

        // Wait for response
        let timeout_budget = self.sync_config.timeout;

        match super::stream::recv(stream, None, timeout_budget).await? {
            Some(StreamMessage::Message {
                payload:
                    MessagePayload::DeltaResponse {
                        delta,
                        author_id,
                        governance_position_blob,
                        delta_signature,
                    },
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

                Ok(Some(FetchedDelta {
                    delta: causal_delta,
                    author_id,
                    governance_position_blob: governance_position_blob.map(|cow| cow.into_owned()),
                    delta_signature,
                }))
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

    /// Handle incoming delta request from a peer
    pub async fn handle_delta_request(
        &self,
        context_id: ContextId,
        delta_id: [u8; 32],
        stream: &mut Stream,
    ) -> Result<()> {
        info!(
            %context_id,
            delta_id = ?delta_id,
            "Handling delta request from peer"
        );

        // Try RocksDB first (has full CausalDelta with HLC)
        use calimero_store::key;

        let handle = self.context_client.datastore_handle();
        let db_key = key::ContextDagDelta::new(context_id, delta_id);

        let response = if let Some(stored_delta) = handle.get(&db_key)? {
            // Found in RocksDB. If the stored row lacks an author
            // claim (snapshot checkpoints, race-path persists that
            // didn't carry author info), refuse to serve via this
            // path — the initiator's check requires an author, and
            // we won't bypass it. The initiator's fallback chain
            // (DAG-catchup-None → snapshot) handles recovery.
            // Genesis carve-out: `create_context` persists the
            // genesis delta with `author_id: None` (it predates any
            // governance op so there's no real author to verify). The
            // wire requires an author, so we serve genesis with the
            // sentinel `PublicKey::from([0; 32])` and the initiator
            // recognizes the sentinel via `is_genesis_author_sentinel`
            // and skips the membership / signature / ReadOnly checks
            // that don't apply to genesis. Without this carve-out a
            // late-joining peer that needed to backfill the genesis
            // delta via DAG-catchup would get `DeltaNotFound` and
            // stall — the same failure mode any other ancestor pull
            // hits without an author.
            let is_genesis_delta = stored_delta.parents == vec![[0u8; 32]];
            let effective_author = match (stored_delta.author_id, is_genesis_delta) {
                (Some(a), _) => Some(a),
                (None, true) => Some(genesis_author_sentinel()),
                (None, false) => None,
            };
            match effective_author {
                None => {
                    debug!(
                        %context_id,
                        delta_id = ?delta_id,
                        "Delta found but stored without an author claim (likely a snapshot \
                         checkpoint or pre-author-tracking row) — returning DeltaNotFound \
                         so the initiator falls back to a verifiable path"
                    );
                    MessagePayload::DeltaNotFound
                }
                Some(author_id) => {
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
                        governance_position_present =
                            stored_delta.governance_position_blob.is_some(),
                        "Sending requested delta to peer"
                    );

                    MessagePayload::DeltaResponse {
                        delta: serialized.into(),
                        author_id,
                        governance_position_blob: stored_delta
                            .governance_position_blob
                            .map(Into::into),
                        delta_signature: stored_delta.delta_signature,
                    }
                }
            }
        } else if let Some(delta_store) = self.state_access.delta_store(&context_id) {
            // Not in RocksDB yet (race condition after broadcast). The
            // in-memory `DeltaStore` doesn't carry author info, so we
            // can't serve a verifiable response from there. Return
            // DeltaNotFound and let the initiator re-fetch once the
            // post-apply persist has settled (next sync tick).
            if delta_store.get_delta(&delta_id).await.is_some() {
                debug!(
                    %context_id,
                    delta_id = ?delta_id,
                    "Delta in in-memory DeltaStore but not yet persisted with author info — \
                     returning DeltaNotFound; initiator will re-fetch after persist settles"
                );
                MessagePayload::DeltaNotFound
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
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: response,
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;

        Ok(())
    }

    /// Handle incoming DAG heads request from a peer
    pub async fn handle_dag_heads_request(
        &self,
        context_id: ContextId,
        stream: &mut Stream,
        _nonce: Nonce,
    ) -> Result<()> {
        info!(
            %context_id,
            "Handling DAG heads request from peer"
        );

        // Get context to retrieve dag_heads and root_hash
        let context = self
            .context_client
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
        let msg = StreamMessage::Message {
            sequence_id: sqx.next(),
            payload: MessagePayload::DagHeadsResponse {
                dag_heads: context.dag_heads,
                root_hash: context.root_hash,
            },
            next_nonce: super::helpers::generate_nonce(),
        };

        super::stream::send(stream, &msg, None).await?;

        Ok(())
    }
}
