//! Namespace sync flows for [`SyncManager`]: governance catch-up + backfill,
//! and namespace / open-subgroup join (request and initiate sides).
//!
//! Extracted from the manager god-file as a self-contained `impl SyncManager`
//! block, driven by the stream dispatcher and the `SyncDriverDispatch` trait.
//! Methods that stay in `mod.rs` remain reachable here via ancestor privacy.

use calimero_context::group_store::{
    CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository, NamespaceRepository,
};
use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::{NamespaceJoinParams, OpenSubgroupJoinParams};
use calimero_node_primitives::join_bundle::JoinBundle;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use libp2p::PeerId;
use rand::Rng;
use tokio::time;
use tracing::{debug, info, warn};

use super::SyncManager;

impl SyncManager {
    /// Actively request governance catch-up from a specific peer whose
    /// identity we don't yet recognize as a context member.
    ///
    /// Scenario: a peer opens a sync stream to us, but their identity isn't
    /// in our local governance DAG yet because fire-and-forget `MemberAdded`
    /// gossip (issue #2237) hasn't reached us. The legacy path waited 2 s
    /// for gossip and then closed the stream, stalling the initiator for
    /// up to 30 s (`NamespaceStateHeartbeat` cadence). Instead, open a
    /// separate stream back to the peer with `NamespaceBackfillRequest`
    /// (empty `delta_ids` = "send everything you have for this namespace"),
    /// apply every op they return, and let the caller re-check membership.
    ///
    /// Best-effort: any failure (no group resolved, stream open fails,
    /// peer returns no ops, ops fail to apply) is logged at debug and the
    /// caller proceeds to close the stream as before. The real fix is the
    /// three-phase contract in #2237; this is a responder-side bandaid
    /// that turns a 30 s stall into at worst a second round-trip.
    pub(super) async fn request_governance_catchup_from_peer(
        &self,
        peer_id: PeerId,
        context_id: &ContextId,
        their_identity: &PublicKey,
    ) {
        let store = self.context_client.datastore();
        let namespace_id =
            match calimero_context::group_store::get_group_for_context(store, context_id) {
                Ok(Some(group_id)) => match NamespaceRepository::new(store).resolve(&group_id) {
                    Ok(ns) => ns.to_bytes(),
                    Err(err) => {
                        debug!(
                            %context_id,
                            %their_identity,
                            %err,
                            "failed to resolve namespace for governance catch-up"
                        );
                        return;
                    }
                },
                Ok(None) => {
                    debug!(
                        %context_id,
                        %their_identity,
                        "context not in a group — no namespace to request catch-up from"
                    );
                    return;
                }
                Err(err) => {
                    debug!(
                        %context_id,
                        %their_identity,
                        %err,
                        "failed to resolve group for governance catch-up"
                    );
                    return;
                }
            };

        let mut stream = match self.sync_network.open_stream(peer_id).await {
            Ok(s) => s,
            Err(err) => {
                debug!(
                    %context_id,
                    %their_identity,
                    %peer_id,
                    %err,
                    "failed to open catch-up stream to peer"
                );
                return;
            }
        };

        let msg = StreamMessage::Init {
            context_id: ContextId::from([0u8; 32]),
            party_id: PublicKey::from([0u8; 32]),
            payload: InitPayload::NamespaceBackfillRequest {
                namespace_id,
                delta_ids: Vec::new(),
            },
            next_nonce: rand::thread_rng().gen(),
        };

        if let Err(err) = crate::sync::stream::send(&mut stream, &msg, None).await {
            debug!(
                %context_id,
                %their_identity,
                %peer_id,
                %err,
                "failed to send NamespaceBackfillRequest during catch-up"
            );
            return;
        }

        let response =
            match crate::sync::stream::recv(&mut stream, None, self.sync_config.timeout).await {
                Ok(Some(StreamMessage::Message {
                    payload: MessagePayload::NamespaceBackfillResponse { deltas },
                    ..
                })) => deltas,
                Ok(_) => {
                    debug!(
                        %context_id,
                        %their_identity,
                        %peer_id,
                        "unexpected response to NamespaceBackfillRequest during catch-up"
                    );
                    return;
                }
                Err(err) => {
                    debug!(
                        %context_id,
                        %their_identity,
                        %peer_id,
                        %err,
                        "catch-up NamespaceBackfillRequest timed out or failed"
                    );
                    return;
                }
            };

        if response.is_empty() {
            debug!(
                %context_id,
                %their_identity,
                %peer_id,
                "peer returned no namespace ops for catch-up"
            );
            return;
        }

        use calimero_context_client::messages::NamespaceApplyOutcome;
        let ops_count = response.len();
        let mut applied = 0usize;
        let mut newly_applied = 0usize;
        for (_delta_id, op_bytes) in response {
            let op = match borsh::from_slice::<
                calimero_context_client::local_governance::SignedNamespaceOp,
            >(&op_bytes)
            {
                Ok(o) => o,
                Err(err) => {
                    debug!(
                        %context_id,
                        %their_identity,
                        %err,
                        "failed to decode catch-up op"
                    );
                    continue;
                }
            };
            match self.context_client.apply_signed_namespace_op(op).await {
                Ok(NamespaceApplyOutcome::Applied { .. }) => {
                    applied += 1;
                    newly_applied += 1;
                }
                Ok(_) => {
                    applied += 1;
                }
                Err(err) => {
                    debug!(
                        %context_id,
                        %their_identity,
                        %err,
                        "failed to apply catch-up op"
                    );
                    continue;
                }
            }
        }

        // Single FSM notification after the batch when we actually
        // advanced the local applied_through. `Pending` (parents missing)
        // and `Duplicate` outcomes are no-progress from the FSM's POV,
        // so we skip the mailbox hop in those cases. Mirrors the gate
        // used at `network_event/namespace.rs:120`.
        if newly_applied > 0 {
            self.node_client.notify_namespace_op_applied(namespace_id);
        }

        // Parity with the gossip apply path: a governance op we just learned
        // may unblock a state delta buffered as `Unknown`. Run whenever this
        // catch-up returned ops, not only on a fresh apply — see
        // `drain_governance_pending_after_sync`.
        if ops_count > 0 {
            self.drain_governance_pending_after_sync().await;
        }

        debug!(
            %context_id,
            %their_identity,
            %peer_id,
            ops_received = ops_count,
            ops_applied = applied,
            "governance catch-up complete"
        );
    }

    /// Release any state deltas parked in the governance-pending buffer after
    /// a governance-sync path applied (or re-confirmed) ops.
    ///
    /// The gossip apply path (`network_event/namespace.rs`) already drains the
    /// governance-pending buffer when a namespace op applies, but the
    /// **sync/backfill** apply paths here did not — a parity gap. A late
    /// joiner's first post-join state delta is buffered as an incomplete-cut
    /// (the projection can't yet resolve membership) until the local node
    /// learns the joiner's membership op; when that op arrives via sync (beacon-triggered
    /// governance sync or catch-up backfill) rather than gossip, nothing
    /// re-evaluated the buffer, so the delta sat there forever and the two
    /// nodes' context root hashes never reconverged.
    ///
    /// Deliberately *not* gated on a fresh `Applied` outcome: the awaited op
    /// may already be present locally (e.g. deduplicated on read, #2327) yet
    /// no drain has ever fired for it. Re-evaluating membership is the correct
    /// trigger, and the call is cheap — `drain_all_governance_pending` returns
    /// immediately when no context holds buffered deltas.
    async fn drain_governance_pending_after_sync(&self) {
        let drain_input = crate::handlers::state_delta::StateDeltaContext {
            node_clients: crate::state::NodeClients {
                context: self.context_client.clone(),
                node: self.node_client.clone(),
            },
            node_state: self.node_state.clone(),
            network_client: self.network_client.clone(),
            sync_timeout: self.sync_config.timeout,
        };
        crate::handlers::state_delta::drain_all_governance_pending(&drain_input).await;
        // PR-6b Task 6b.5: a node offline across a migration window reconnects,
        // syncs, and lazily advances its binary on first execute. Sync settle
        // is the node-side observation point for that advance — drain any
        // absorbed straggler deltas whose schema the now-loaded reader can read,
        // replaying their original signed bytes verbatim.
        crate::handlers::state_delta::drain_all_absorbed(&drain_input).await;
    }

    /// #2625: when `context_id` has state deltas parked in the
    /// governance-pending buffer, proactively pull its namespace governance
    /// DAG so the missing governance op lands and the buffered deltas drain.
    ///
    /// This closes the gap left by #2589: that fix drains the buffer *when a
    /// governance op is applied* via sync, but here the op is never delivered
    /// to us at all. The only local record that the op exists is the buffered
    /// delta's `governance_position`; our governance DAG has no missing-parent
    /// entry for it, so `resolve_namespace_pending` (which gates on
    /// `namespace_has_pending`) is a no-op and never requests it. Actively
    /// pulling the namespace DAG is what fetches the op; `sync_namespace_from_peer`
    /// then calls `drain_governance_pending_after_sync` once any ops arrive.
    ///
    /// Peer selection matters: the missing op is almost always an *encrypted
    /// group op*, and only a group **member** stores it as a full
    /// `StoredNamespaceEntry::Signed` (a non-member namespace subscriber holds
    /// only the `Opaque` skeleton and serves nothing for it). So we target the
    /// peers that actually delivered the stuck deltas first — they satisfied
    /// the delta's governance position at send time, hence hold the `Signed`
    /// op — and only fall back to an arbitrary mesh peer if that didn't drain
    /// the buffer (e.g. the delta was relayed by a non-member).
    ///
    /// Gated on a non-empty buffer (a cheap `DashMap` length read), so the
    /// steady-state cost on every interval tick is one map lookup.
    pub(super) async fn backfill_governance_for_pending_deltas(&self, context_id: ContextId) {
        if !should_backfill_governance(self.node_state.governance_pending_len(&context_id)) {
            return;
        }
        let store = self.context_client.datastore_handle().into_inner();
        let Some(namespace_id) = resolve_namespace_id(&store, &context_id) else {
            debug!(
                %context_id,
                "governance-pending backfill: could not resolve namespace id; skipping (#2625)"
            );
            return;
        };
        drop(store);
        debug!(
            %context_id,
            namespace_id = %hex::encode(namespace_id),
            pending = self.node_state.governance_pending_len(&context_id),
            "governance-pending backfill: pulling namespace governance DAG to release buffered deltas (#2625)"
        );

        // Prefer the peers that delivered the stuck deltas (likely group
        // members holding the full `Signed` op). Stop as soon as the buffer
        // drains so we don't open redundant streams.
        for peer in self.node_state.governance_pending_source_peers(&context_id) {
            if !should_backfill_governance(self.node_state.governance_pending_len(&context_id)) {
                return;
            }
            self.sync_namespace_from_peer(namespace_id, Some(peer))
                .await;
        }

        // Fallback: a non-member relay may have delivered the delta, so its
        // source peer couldn't serve the op. Try the namespace mesh — but
        // anyone can subscribe to the `ns/<id>` topic without being a member,
        // so prefer trusted ANCHORS (peers we've observed signing applied
        // messages with an Owner/Admin/ReadOnlyTee identity) over arbitrary
        // subscribers, exactly like the regular context-sync partner picker.
        //
        // This is a *liveness* defense, not a safety one: a malicious or
        // non-member subscriber cannot corrupt our governance state — every
        // op is signature-verified in `apply_signed_op` before any mutation,
        // is content-hash idempotent, and is nonce/DAG-ordered. The worst a
        // bad peer can do is serve nothing or stale ops; anchor-first ordering
        // just avoids wasting backfill rounds on such peers.
        if should_backfill_governance(self.node_state.governance_pending_len(&context_id)) {
            let topic =
                libp2p::gossipsub::TopicHash::from_raw(format!("ns/{}", hex::encode(namespace_id)));
            let mut peers = self.sync_network.subscribed_peers(topic).await;
            let _anchor_count = crate::sync::peers::partition_peers_anchor_first(
                &mut peers,
                &*self.state_access,
                &self.anchor_identities_for_context(&context_id),
            );
            for peer in peers {
                if !should_backfill_governance(self.node_state.governance_pending_len(&context_id))
                {
                    break;
                }
                self.sync_namespace_from_peer(namespace_id, Some(peer))
                    .await;
            }
        }
    }

    /// Handle a namespace backfill request: look up full `SignedNamespaceOp`
    /// payloads for the requested delta IDs and send them back.
    ///
    /// We scan the namespace governance op store for matching delta IDs.
    /// For each requested delta, if we have the full op (stored when we were
    /// a member at apply time), we include it in the response.
    pub(super) async fn handle_namespace_backfill_request(
        &self,
        namespace_id: [u8; 32],
        delta_ids: &[[u8; 32]],
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        let store = self.context_client.datastore_handle().into_inner();
        let handle = store.handle();
        let mut found = Vec::new();

        /// Maximum ops returned in a single backfill response to prevent
        /// memory exhaustion from large namespace governance DAGs.
        const MAX_BACKFILL_OPS: usize = 500;

        if delta_ids.is_empty() {
            // Empty request = "give me everything for this namespace".
            let start = calimero_store::key::NamespaceGovOp::new(namespace_id, [0u8; 32]);
            let mut iter = handle.iter::<calimero_store::key::NamespaceGovOp>()?;
            let first = iter.seek(start).transpose();

            for entry in first.into_iter().chain(iter.keys()) {
                let key = match entry {
                    Ok(k) => k,
                    Err(_) => break,
                };
                if key.namespace_id() != namespace_id {
                    break;
                }
                if let Ok(Some(value)) = handle.get(&key) {
                    if let Some(signed_bytes) =
                        crate::sync::helpers::extract_signed_op_bytes(&value.skeleton_bytes)
                    {
                        found.push((key.delta_id(), signed_bytes));
                        if found.len() >= MAX_BACKFILL_OPS {
                            break;
                        }
                    }
                }
            }
        } else {
            for delta_id in delta_ids.iter().take(MAX_BACKFILL_OPS) {
                let key = calimero_store::key::NamespaceGovOp::new(namespace_id, *delta_id);
                if let Ok(Some(value)) = handle.get(&key) {
                    if let Some(signed_bytes) =
                        crate::sync::helpers::extract_signed_op_bytes(&value.skeleton_bytes)
                    {
                        found.push((*delta_id, signed_bytes));
                    }
                }
            }
        }

        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::NamespaceBackfillResponse { deltas: found },
            next_nonce: nonce,
        };
        crate::sync::stream::send(stream, &msg, None).await?;
        Ok(())
    }

    /// Handle an incoming NamespaceJoinRequest on the responder side.
    ///
    /// Validates the invitation, wraps the group key for the joiner,
    /// enumerates contexts, and collects governance ops.
    pub(super) async fn handle_namespace_join_request(
        &self,
        namespace_id: [u8; 32],
        invitation_bytes: &[u8],
        joiner_public_key: PublicKey,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        use calimero_context::group_store::enumerate_group_contexts;
        use calimero_context::group_store::NamespaceMembershipService;
        use calimero_context_config::types::ContextGroupId;
        use calimero_context_config::types::SignedGroupOpenInvitation;

        let invitation: SignedGroupOpenInvitation = match borsh::from_slice(invitation_bytes) {
            Ok(inv) => inv,
            Err(err) => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::NamespaceJoinRejected {
                        reason: format!("invalid invitation: {err}"),
                    },
                    next_nonce: nonce,
                };
                crate::sync::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
        };

        let group_id = ContextGroupId::from(namespace_id);
        let store = self.context_client.datastore_handle().into_inner();

        let meta = match MetaRepository::new(&store).load(&group_id)? {
            Some(m) => m,
            None => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::NamespaceJoinRejected {
                        reason: "group not found".to_owned(),
                    },
                    next_nonce: nonce,
                };
                crate::sync::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
        };

        // Validate the invitation against this responder's local clock
        // before releasing the group key or pre-registering the joiner.
        // A wall-clock check is sound here because key delivery is
        // point-to-point, not folded governance state, so responders
        // disagreeing cannot diverge membership.
        let now_secs = calimero_context::group_store::now_secs();
        if let Err(err) = NamespaceMembershipService::new(&store, namespace_id)
            .validate_open_invitation(&invitation, now_secs)
        {
            let msg = StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::NamespaceJoinRejected {
                    reason: format!("invitation rejected: {err}"),
                },
                next_nonce: nonce,
            };
            crate::sync::stream::send(stream, &msg, None).await?;
            return Ok(());
        }

        let key_envelope_bytes = match GroupKeyring::new(&store, group_id).load_current_key()? {
            Some((_key_id, group_key)) => {
                let ns_identity =
                    NamespaceRepository::new(&store).resolve_identity_record(&group_id)?;
                match ns_identity {
                    Some(record) => {
                        let sender_sk =
                            calimero_primitives::identity::PrivateKey::from(record.private_key);
                        match GroupKeyring::wrap_for_member(
                            &sender_sk,
                            &joiner_public_key,
                            &group_key,
                        ) {
                            Ok(envelope) => borsh::to_vec(&envelope).unwrap_or_default(),
                            Err(err) => {
                                warn!(
                                    namespace_id = %hex::encode(namespace_id),
                                    %err,
                                    "failed to wrap group key for joiner"
                                );
                                Vec::new()
                            }
                        }
                    }
                    None => {
                        warn!(
                            namespace_id = %hex::encode(namespace_id),
                            "no namespace identity found, cannot wrap key"
                        );
                        Vec::new()
                    }
                }
            }
            None => Vec::new(),
        };

        // Pre-register the joiner as a group member and write ContextIdentity
        // entries so that when the joiner opens a sync stream, this node's
        // membership check (has_member) passes immediately.
        if let Err(e) = MembershipRepository::new(&store).add_member(
            &group_id,
            &joiner_public_key,
            calimero_primitives::context::GroupMemberRole::Member,
        ) {
            warn!(%e, "failed to pre-register joiner as group member");
        }

        let context_ids = enumerate_group_contexts(&store, &group_id, 0, usize::MAX)?;
        let application_id: [u8; 32] = *meta.target_application_id.as_ref();

        for ctx_id in &context_ids {
            let ci_key = calimero_store::key::ContextIdentity::new(*ctx_id, joiner_public_key);
            let mut handle = store.handle();
            if !handle.has(&ci_key).unwrap_or(false) {
                let _ = handle.put(
                    &ci_key,
                    &calimero_store::types::ContextIdentity {
                        private_key: None,
                        sender_key: None,
                    },
                );
            }
        }

        let governance_ops = self.collect_namespace_governance_ops(namespace_id)?;

        // Issue #2256: the namespace's default-capabilities value travels
        // with the bundle so the joiner doesn't need to fall back to a
        // hard-coded constant. Read whatever the responder currently
        // believes (already reflects any admin-issued
        // `DefaultCapabilitiesSet` ops because the local store is
        // updated as those ops apply). `unwrap_or(0)` matches the
        // pre-existing semantics for "default key absent."
        let default_capabilities = CapabilitiesRepository::new(&store)
            .default_capabilities(&group_id)?
            .unwrap_or(0);

        debug!(
            namespace_id = %hex::encode(namespace_id),
            has_key = !key_envelope_bytes.is_empty(),
            context_count = context_ids.len(),
            app_id = %hex::encode(application_id),
            governance_ops_count = governance_ops.len(),
            default_capabilities,
            "Sending NamespaceJoinResponse"
        );

        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::NamespaceJoinResponse {
                key_envelope_bytes,
                context_ids,
                application_id,
                governance_ops,
                default_capabilities,
            },
            next_nonce: nonce,
        };
        crate::sync::stream::send(stream, &msg, None).await?;
        Ok(())
    }

    /// Handle an incoming `OpenSubgroupJoinRequest` (issue #2357) on the
    /// responder side. Validates that the joiner has
    /// `MembershipPath::Inherited` to the requested subgroup, wraps the
    /// local subgroup key for the joiner via ECDH, and replies with the
    /// envelope. Mirrors `handle_namespace_join_request` for the
    /// inherited self-join path.
    pub(super) async fn handle_open_subgroup_join_request(
        &self,
        namespace_id: [u8; 32],
        subgroup_id: [u8; 32],
        joiner_public_key: PublicKey,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        use calimero_context::group_store::MembershipPath;
        use calimero_context_config::types::ContextGroupId;

        let subgroup_gid = ContextGroupId::from(subgroup_id);
        let store = self.context_client.datastore_handle().into_inner();

        // Cross-namespace pin: the requested subgroup must belong to the
        // namespace the joiner named, otherwise an attacker on namespace
        // A could elicit a key for a subgroup of namespace B.
        match NamespaceRepository::new(&store).resolve(&subgroup_gid) {
            Ok(ns) if ns.to_bytes() == namespace_id => {}
            Ok(other_ns) => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::OpenSubgroupJoinRejected {
                        reason: format!(
                            "subgroup belongs to namespace {} not {}",
                            hex::encode(other_ns.to_bytes()),
                            hex::encode(namespace_id),
                        ),
                    },
                    next_nonce: nonce,
                };
                crate::sync::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
            Err(err) => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::OpenSubgroupJoinRejected {
                        reason: format!("resolve namespace: {err}"),
                    },
                    next_nonce: nonce,
                };
                crate::sync::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
        }

        if MetaRepository::new(&store).load(&subgroup_gid)?.is_none() {
            let msg = StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::OpenSubgroupJoinRejected {
                    reason: "subgroup not found locally".to_owned(),
                },
                next_nonce: nonce,
            };
            crate::sync::stream::send(stream, &msg, None).await?;
            return Ok(());
        }

        // Authorisation check: the joiner must reach the subgroup via the
        // Open-chain inheritance walk. `MembershipPath::Inherited`
        // implies every intermediate ancestor was Open (see
        // `membership.rs:267`), so this is the proof of authorisation.
        match MembershipRepository::new(&store).check_path(&subgroup_gid, &joiner_public_key)? {
            MembershipPath::Inherited { .. } | MembershipPath::Direct => {}
            MembershipPath::None => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::OpenSubgroupJoinRejected {
                        reason: "joiner has no membership path to subgroup".to_owned(),
                    },
                    next_nonce: nonce,
                };
                crate::sync::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
        }

        let key_envelope_bytes = match GroupKeyring::new(&store, subgroup_gid).load_current_key()? {
            Some((_key_id, group_key)) => {
                let ns_gid = ContextGroupId::from(namespace_id);
                match NamespaceRepository::new(&store).resolve_identity_record(&ns_gid)? {
                    Some(record) => {
                        let sender_sk =
                            calimero_primitives::identity::PrivateKey::from(record.private_key);
                        match GroupKeyring::wrap_for_member(
                            &sender_sk,
                            &joiner_public_key,
                            &group_key,
                        ) {
                            Ok(envelope) => borsh::to_vec(&envelope).unwrap_or_default(),
                            Err(err) => {
                                warn!(
                                    namespace_id = %hex::encode(namespace_id),
                                    subgroup_id = %hex::encode(subgroup_id),
                                    %err,
                                    "failed to wrap subgroup key for joiner"
                                );
                                Vec::new()
                            }
                        }
                    }
                    None => {
                        warn!(
                            namespace_id = %hex::encode(namespace_id),
                            "no namespace identity, cannot wrap subgroup key"
                        );
                        Vec::new()
                    }
                }
            }
            None => Vec::new(),
        };

        debug!(
            namespace_id = %hex::encode(namespace_id),
            subgroup_id = %hex::encode(subgroup_id),
            has_key = !key_envelope_bytes.is_empty(),
            "Sending OpenSubgroupJoinResponse"
        );

        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::OpenSubgroupJoinResponse { key_envelope_bytes },
            next_nonce: nonce,
        };
        crate::sync::stream::send(stream, &msg, None).await?;
        Ok(())
    }

    /// Initiator side for `request_open_subgroup_join`. Picks a mesh peer
    /// on the namespace topic, opens a stream, sends the request, and
    /// returns the wrapped key envelope. Same peer-discovery retry loop
    /// as `initiate_namespace_join`.
    pub(super) async fn initiate_open_subgroup_join(
        &self,
        params: OpenSubgroupJoinParams,
    ) -> eyre::Result<Vec<u8>> {
        let topic = libp2p::gossipsub::TopicHash::from_raw(format!(
            "ns/{}",
            hex::encode(params.namespace_id)
        ));

        let mut peers = Vec::new();
        for attempt in 1..=crate::sync::config::DEFAULT_MESH_RETRIES_UNINITIALIZED {
            peers = self.sync_network.subscribed_peers(topic.clone()).await;
            if !peers.is_empty() {
                break;
            }
            if attempt < crate::sync::config::DEFAULT_MESH_RETRIES_UNINITIALIZED {
                debug!(
                    namespace_id = %hex::encode(params.namespace_id),
                    subgroup_id = %hex::encode(params.subgroup_id),
                    attempt,
                    "No namespace mesh peers yet for open-subgroup join, retrying..."
                );
                time::sleep(std::time::Duration::from_millis(
                    crate::sync::config::DEFAULT_MESH_RETRY_DELAY_MS_UNINITIALIZED,
                ))
                .await;
            }
        }

        if peers.is_empty() {
            eyre::bail!(
                "no mesh peers for namespace {} (open-subgroup join)",
                hex::encode(params.namespace_id)
            );
        }

        // Try every mesh peer, not just the first. Only peers that
        // already hold the subgroup key can serve the request — for an
        // `Open` subgroup that is the creator plus anyone who has
        // already inherited in. A freshly-joined namespace member
        // (which is also on the `ns/<hex>` topic) replies with an empty
        // envelope ("responder did not hold the subgroup key"); picking
        // `peers.first()` would fail the whole join whenever that peer
        // happened to be key-less. Walk the list: return on the first
        // peer that yields a key, skip key-less peers, and remember the
        // last authorization rejection so it surfaces if NO peer
        // accepts (a rejection from one peer can be a stale cold-start
        // view while another peer accepts).
        let mut last_rejection: Option<String> = None;
        let mut keyless_peers = 0usize;
        let mut transport_errors = 0usize;

        for peer in &peers {
            let mut stream = match self.sync_network.open_stream(*peer).await {
                Ok(s) => s,
                Err(e) => {
                    debug!(
                        peer = %peer,
                        subgroup_id = %hex::encode(params.subgroup_id),
                        error = %e,
                        "open-subgroup join: failed to open stream, trying next peer"
                    );
                    transport_errors += 1;
                    continue;
                }
            };

            let msg = StreamMessage::Init {
                context_id: calimero_primitives::context::ContextId::from([0u8; 32]),
                party_id: params.joiner_public_key,
                payload: InitPayload::OpenSubgroupJoinRequest {
                    namespace_id: params.namespace_id,
                    subgroup_id: params.subgroup_id,
                    joiner_public_key: params.joiner_public_key,
                },
                next_nonce: rand::thread_rng().gen(),
            };

            if let Err(e) = crate::sync::stream::send(&mut stream, &msg, None).await {
                debug!(
                    peer = %peer,
                    error = %e,
                    "open-subgroup join: send failed, trying next peer"
                );
                transport_errors += 1;
                continue;
            }

            match crate::sync::stream::recv(&mut stream, None, self.sync_config.timeout).await {
                Ok(Some(StreamMessage::Message {
                    payload: MessagePayload::OpenSubgroupJoinResponse { key_envelope_bytes },
                    ..
                })) => {
                    if key_envelope_bytes.is_empty() {
                        // Peer is on the namespace topic but doesn't
                        // hold the subgroup key — try the next one.
                        keyless_peers += 1;
                        continue;
                    }
                    return Ok(key_envelope_bytes);
                }
                Ok(Some(StreamMessage::Message {
                    payload: MessagePayload::OpenSubgroupJoinRejected { reason },
                    ..
                })) => {
                    // A rejection may be a stale cold-start view on this
                    // peer; keep trying others before surfacing it.
                    debug!(
                        peer = %peer,
                        reason = %reason,
                        "open-subgroup join: peer rejected, trying next peer"
                    );
                    last_rejection = Some(reason);
                    continue;
                }
                Ok(other) => {
                    debug!(
                        peer = %peer,
                        "open-subgroup join: unexpected response {:?}, trying next peer",
                        other.as_ref().map(std::mem::discriminant)
                    );
                    transport_errors += 1;
                    continue;
                }
                Err(e) => {
                    debug!(
                        peer = %peer,
                        error = %e,
                        "open-subgroup join: recv failed, trying next peer"
                    );
                    transport_errors += 1;
                    continue;
                }
            }
        }

        // No peer yielded the key. Surface the most informative cause,
        // always including the full per-peer tally so a mixed failure
        // (some peers key-less, one peer rejecting, some transport
        // errors) is fully diagnosable from a single line.
        let tally = format!(
            "{} peer(s): {} key-less, {} transport error(s)",
            peers.len(),
            keyless_peers,
            transport_errors
        );
        if let Some(reason) = last_rejection {
            eyre::bail!(
                "open-subgroup join for {} served by no peer — last rejection: {} [{}]",
                hex::encode(params.subgroup_id),
                reason,
                tally
            );
        }
        eyre::bail!(
            "no mesh peer held the subgroup key for {} [{}]",
            hex::encode(params.subgroup_id),
            tally
        );
    }

    /// Collect all governance ops for a namespace (reused by the join responder).
    ///
    /// Returns bare `SignedNamespaceOp` bytes (not `StoredNamespaceEntry` wrapped)
    /// so recipients can `borsh::from_slice::<SignedNamespaceOp>` directly.
    fn collect_namespace_governance_ops(
        &self,
        namespace_id: [u8; 32],
    ) -> eyre::Result<Vec<Vec<u8>>> {
        let store = self.context_client.datastore_handle().into_inner();
        let handle = store.handle();
        let mut ops = Vec::new();

        let start = calimero_store::key::NamespaceGovOp::new(namespace_id, [0u8; 32]);
        let mut iter = handle.iter::<calimero_store::key::NamespaceGovOp>()?;
        let first = iter.seek(start).transpose();

        for entry in first.into_iter().chain(iter.keys()) {
            let key = match entry {
                Ok(k) => k,
                Err(_) => break,
            };
            if key.namespace_id() != namespace_id {
                break;
            }
            if let Ok(Some(value)) = handle.get(&key) {
                if let Some(bytes) =
                    crate::sync::helpers::extract_signed_op_bytes(&value.skeleton_bytes)
                {
                    ops.push(bytes);
                }
            }
        }

        Ok(ops)
    }

    /// Initiator side: open a stream to a mesh peer and perform the
    /// NamespaceJoinRequest / NamespaceJoinResponse exchange.
    pub(super) async fn initiate_namespace_join(
        &self,
        params: NamespaceJoinParams,
    ) -> eyre::Result<JoinBundle> {
        // Connect-loop logic (shuffled-peer retry, per-peer timeout,
        // outer deadline) lives in `super::namespace_join::open_namespace_join_stream`
        // so it can be unit-tested against `MockSyncNetwork` without
        // standing up a full `SyncManager`. See that module for the
        // design rationale (mesh-formation latency, stale-transport
        // fallback, deadline budgeting under large meshes).
        //
        // Outer loop retries the entire connect-and-exchange when the
        // chosen peer returns `NamespaceJoinRejected` or fails the
        // post-open send/recv. A peer can be in the gossipsub mesh
        // and reachable on transport but not yet have processed the
        // namespace governance DAG far enough to serve the join —
        // rejecting that peer must not fail the whole join when
        // another mesh peer is in a position to answer. Mirrors the
        // pattern `initiate_open_subgroup_join` uses for the same
        // mesh-cold-peer race.
        //
        // Rejected peers feed back into `open_namespace_join_stream`
        // via `excluded_peers` so the next round skips them at the
        // connect layer rather than re-opening a transport just to
        // get rejected again.
        let mut rejected_peers: std::collections::HashSet<libp2p::PeerId> =
            std::collections::HashSet::new();
        let mut last_rejection: Option<String> = None;
        let mut last_connect_err: Option<String> = None;
        // Cap on protocol-level retries. The connect loop already
        // handles transport failure across peers; this cap bounds the
        // total post-open exchanges so a small mesh full of stale
        // peers can't deadlock the join indefinitely. Sized to cover
        // typical 1–3 mesh peers plus headroom.
        const MAX_PROTOCOL_RETRIES: usize = 5;

        for protocol_attempt in 1..=MAX_PROTOCOL_RETRIES {
            let (mut stream, peer) = match super::namespace_join::open_namespace_join_stream(
                &*self.sync_network,
                params.namespace_id,
                self.sync_config.open_stream_timeout,
                crate::sync::config::DEFAULT_MESH_RETRIES_UNINITIALIZED,
                std::time::Duration::from_millis(
                    crate::sync::config::DEFAULT_MESH_RETRY_DELAY_MS_UNINITIALIZED,
                ),
                &rejected_peers,
            )
            .await
            {
                Ok(opened) => opened,
                Err(open_err) => {
                    if last_rejection.is_none() {
                        // First attempt's connect loop exhausted with
                        // no prior protocol-level success. The
                        // connect loop has its own mesh-retry budget;
                        // re-running it immediately would repeat the
                        // same exhaustion with no state change.
                        // Surface the connect_err directly.
                        return Err(open_err);
                    }
                    // Connect failure *after* at least one peer has
                    // rejected: do not bail. The mesh may surface a
                    // fresh peer on a later protocol attempt that
                    // wasn't visible during this one (mesh-formation
                    // delay, peer just finished processing the
                    // namespace governance DAG, etc.). Record the err
                    // for the exhaustion diagnostic and let the loop
                    // continue.
                    debug!(
                        namespace_id = %hex::encode(params.namespace_id),
                        attempt = protocol_attempt,
                        error = %open_err,
                        "namespace join: connect failed after prior rejection, will retry"
                    );
                    last_connect_err = Some(open_err.to_string());
                    continue;
                }
            };

            let msg = StreamMessage::Init {
                context_id: calimero_primitives::context::ContextId::from([0u8; 32]),
                party_id: params.joiner_public_key,
                payload: InitPayload::NamespaceJoinRequest {
                    namespace_id: params.namespace_id,
                    invitation_bytes: params.invitation_bytes.clone(),
                    joiner_public_key: params.joiner_public_key,
                },
                next_nonce: rand::thread_rng().gen(),
            };

            if let Err(send_err) = crate::sync::stream::send(&mut stream, &msg, None).await {
                debug!(
                    namespace_id = %hex::encode(params.namespace_id),
                    %peer,
                    error = %send_err,
                    "namespace join: send failed, marking peer rejected, trying next peer"
                );
                rejected_peers.insert(peer);
                continue;
            }

            match crate::sync::stream::recv(&mut stream, None, self.sync_config.timeout).await {
                Ok(Some(StreamMessage::Message {
                    payload:
                        MessagePayload::NamespaceJoinResponse {
                            key_envelope_bytes,
                            context_ids,
                            application_id,
                            governance_ops,
                            default_capabilities,
                        },
                    ..
                })) => {
                    return Ok(JoinBundle {
                        key_envelope_bytes,
                        context_ids,
                        application_id: application_id.into(),
                        governance_ops,
                        default_capabilities,
                    });
                }
                Ok(Some(StreamMessage::Message {
                    payload: MessagePayload::NamespaceJoinRejected { reason },
                    ..
                })) => {
                    debug!(
                        namespace_id = %hex::encode(params.namespace_id),
                        %peer,
                        %reason,
                        attempt = protocol_attempt,
                        "namespace join: peer rejected, trying next peer"
                    );
                    rejected_peers.insert(peer);
                    last_rejection = Some(reason);
                    continue;
                }
                Ok(other) => {
                    let detail = format!(
                        "unexpected response variant: {:?}",
                        other.as_ref().map(std::mem::discriminant)
                    );
                    debug!(
                        namespace_id = %hex::encode(params.namespace_id),
                        %peer,
                        %detail,
                        "namespace join: unexpected response, marking peer rejected"
                    );
                    rejected_peers.insert(peer);
                    // Carry the unexpected-response detail into
                    // `last_rejection` so the exhaustion error keeps
                    // diagnostic context if every retry hits this arm.
                    last_rejection = Some(detail);
                    continue;
                }
                Err(recv_err) => {
                    let detail = format!("recv failed: {recv_err}");
                    debug!(
                        namespace_id = %hex::encode(params.namespace_id),
                        %peer,
                        %detail,
                        "namespace join: recv failed, marking peer rejected, trying next peer"
                    );
                    rejected_peers.insert(peer);
                    // Same rationale as the `Ok(other)` arm above —
                    // carry the recv failure into `last_rejection` so
                    // the exhaustion error remains informative.
                    last_rejection = Some(detail);
                    continue;
                }
            }
        }

        eyre::bail!(
            "namespace join exhausted {} protocol attempts (last rejection: {:?}, \
             last connect_err: {:?}, {} peer(s) rejected)",
            MAX_PROTOCOL_RETRIES,
            last_rejection,
            last_connect_err,
            rejected_peers.len()
        )
    }

    /// Pull all namespace governance ops from a peer.
    ///
    /// `peer = Some(p)` targets `p` explicitly; `None` picks the first mesh
    /// peer subscribed to the namespace topic (the legacy behaviour). Callers
    /// that know a group **member** should target it: only members store the
    /// full [`StoredNamespaceEntry::Signed`] op (carrying the encrypted group
    /// payload), so a non-member namespace subscriber holds only the
    /// [`StoredNamespaceEntry::Opaque`] skeleton and `extract_signed_op`
    /// returns `None` for it — backfilling from such a peer yields nothing for
    /// group ops and would never release a governance-pending delta.
    pub(super) async fn sync_namespace_from_peer(
        &self,
        namespace_id: [u8; 32],
        peer: Option<PeerId>,
    ) {
        use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

        let peer = match peer {
            Some(p) => p,
            None => {
                let topic = libp2p::gossipsub::TopicHash::from_raw(format!(
                    "ns/{}",
                    hex::encode(namespace_id)
                ));
                let peers = self.sync_network.subscribed_peers(topic).await;
                let Some(p) = peers.first().copied() else {
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        "no mesh peers for namespace sync"
                    );
                    return;
                };
                p
            }
        };

        let Ok(mut stream) = self.sync_network.open_stream(peer).await else {
            debug!("failed to open stream for namespace sync");
            return;
        };

        let msg = StreamMessage::Init {
            context_id: calimero_primitives::context::ContextId::from([0u8; 32]),
            party_id: calimero_primitives::identity::PublicKey::from([0u8; 32]),
            payload: InitPayload::NamespaceBackfillRequest {
                namespace_id,
                delta_ids: vec![],
            },
            next_nonce: {
                use rand::Rng;
                rand::thread_rng().gen()
            },
        };

        if let Err(err) = crate::sync::stream::send(&mut stream, &msg, None).await {
            debug!(%err, "failed to send NamespaceBackfillRequest");
            return;
        }

        match crate::sync::stream::recv(&mut stream, None, self.sync_config.timeout).await {
            Ok(Some(StreamMessage::Message {
                payload: MessagePayload::NamespaceBackfillResponse { deltas },
                ..
            })) => {
                let ops_received = deltas.len();
                info!(
                    namespace_id = %hex::encode(namespace_id),
                    ops = ops_received,
                    "received namespace governance ops from peer"
                );
                use calimero_context_client::messages::NamespaceApplyOutcome;
                let mut newly_applied = false;
                // Collect divergence reports surfaced by `MemberRemoved` /
                // `MemberLeft` ops arriving via the namespace-backfill
                // path. Same reasoning as the gossip-receive path: once
                // the DAG marks an op `Applied`, any later gossipsub
                // arrival of the same op becomes `Duplicate` and the
                // apply work — including the post-apply hash check —
                // is skipped. If a `MemberRemoved` op arrives first via
                // backfill and divergence is dropped here, no later
                // path will re-surface it. Fire reconcile after the
                // batch loop so we don't hold `&mut` borrows across an
                // await on `self`.
                let mut pending_divergences: Vec<
                    calimero_context_client::messages::DivergenceReport,
                > = Vec::new();
                for (delta_id, op_bytes) in deltas {
                    match borsh::from_slice::<
                        calimero_context_client::local_governance::SignedNamespaceOp,
                    >(&op_bytes)
                    {
                        Ok(op) => {
                            match self
                                .context_client
                                .apply_signed_namespace_op(op.clone())
                                .await
                            {
                                Err(err) => {
                                    // Capture enough context to diagnose codec/schema
                                    // mismatches (observed as "Unexpected length of
                                    // input" from the inner GroupOp decode when a
                                    // variant's binary layout has drifted). The
                                    // op-type tag + byte-length give us a fingerprint
                                    // without logging potentially sensitive payload.
                                    let op_kind = match &op.op {
                                        calimero_context_client::local_governance::NamespaceOp::Root(r) => {
                                            format!("Root::{r:?}").split('{').next().unwrap_or("Root").trim().to_owned()
                                        }
                                        calimero_context_client::local_governance::NamespaceOp::Group { .. } => {
                                            "Group".to_owned()
                                        }
                                    };
                                    warn!(
                                        namespace_id = %hex::encode(namespace_id),
                                        delta_id = %hex::encode(delta_id),
                                        op_kind = %op_kind,
                                        signer = %op.signer,
                                        nonce = op.nonce,
                                        op_bytes_len = op_bytes.len(),
                                        ?err,
                                        "failed to apply namespace governance op from backfill"
                                    );
                                }
                                Ok(NamespaceApplyOutcome::Applied { divergence }) => {
                                    newly_applied = true;
                                    if let Some(report) = divergence {
                                        pending_divergences.push(report);
                                    }
                                    // Group-key delivery is no longer pushed
                                    // from the apply path (the one-shot
                                    // receiver-side push was the #2613
                                    // defect). The joiner pulls any key it
                                    // lacks at the end of this sync round
                                    // (see `recover_missing_group_keys`);
                                    // admin-initiated pushes still come from
                                    // `add_group_members`/`admit_tee_node`.
                                }
                                Ok(_) => {}
                            }
                        }
                        Err(err) => {
                            warn!(
                                namespace_id = %hex::encode(namespace_id),
                                delta_id = %hex::encode(delta_id),
                                op_bytes_len = op_bytes.len(),
                                op_bytes_prefix = %hex::encode(&op_bytes[..op_bytes.len().min(64)]),
                                %err,
                                "failed to decode namespace governance op from backfill"
                            );
                        }
                    }
                }
                // FSM notify after the batch — gated on at least one
                // `Applied` outcome (Pending/Duplicate are no-progress).
                // See the governance-catch-up notify above for rationale.
                if newly_applied {
                    self.node_client.notify_namespace_op_applied(namespace_id);
                }

                // Route any divergence reports surfaced during the
                // backfill apply loop to the reconcile-via-anchor path.
                // Run sequentially after the batch finishes; we're
                // already in an async method on `&self` so no spawn
                // is needed here (the gossip-receive path uses
                // `actix::spawn` because it runs inside an actor's
                // mailbox slot; this method is invoked by the sync
                // tick which has no such constraint).
                for report in pending_divergences {
                    self.reconcile_after_divergence(report).await;
                }

                // Parity with the gossip apply path: releasing buffered
                // state deltas waiting on a membership op we just backfilled.
                // This is the path the late-joiner reverse-sync hit — the
                // joiner's first post-join write was buffered as `Unknown`
                // and the membership op that unblocks it arrived here, via
                // backfill, never via gossip, so nothing drained the buffer.
                if ops_received > 0 {
                    self.drain_governance_pending_after_sync().await;
                }

                // Pull-based group-key recovery (#2613). Having just synced
                // the namespace DAG with this peer, ask it for the key to any
                // group we hold buffered-but-undecryptable ops for. The
                // durable replacement for the removed one-shot receiver-side
                // push: retried every sync round (and on the interval tick /
                // gossip receipt) so a member that missed a delivery is never
                // permanently locked out of group decryption.
                self.recover_missing_group_keys(namespace_id, Some(peer))
                    .await;
            }
            _ => {
                debug!("unexpected response to namespace sync request");
            }
        }
    }

    /// Joiner side of direct key delivery (#2613). For each group in
    /// `namespace_id` that we hold buffered-but-undecryptable ops for,
    /// request the key and apply any wrapped key a peer returns.
    ///
    /// Tries `preferred_peer` first (the peer we just synced with), then
    /// namespace-mesh peers, stopping at the first peer that serves each
    /// group's key. A keyless peer answers with an empty envelope, so trying
    /// several peers in one round means a single keyless peer doesn't cost a
    /// whole interval.
    ///
    /// **Durability (the #2613 fix):** runs at the end of a namespace sync,
    /// on every interval tick (`perform_interval_sync`), and on gossip receipt
    /// of a namespace op. Namespace sync is otherwise edge-triggered, so
    /// without these a member that missed its key at join time would never
    /// retry. Best-effort: every error path is `debug!`/continue.
    pub(crate) async fn recover_missing_group_keys(
        &self,
        namespace_id: [u8; 32],
        preferred_peer: Option<PeerId>,
    ) {
        let store = self.context_client.datastore_handle().into_inner();
        let ns_gid = calimero_context_config::types::ContextGroupId::from(namespace_id);

        // Our namespace identity is the member we request a key for and the
        // ECDH recipient. No identity in this namespace ⇒ nothing to recover.
        let requester_public_key = match NamespaceRepository::new(&store).identity_record(&ns_gid) {
            Ok(Some(record)) => {
                calimero_primitives::identity::PrivateKey::from(record.private_key).public_key()
            }
            Ok(None) => return,
            Err(err) => {
                debug!(%err, "failed to resolve namespace identity for key recovery");
                return;
            }
        };

        let awaiting = match calimero_context::group_store::namespace_groups_awaiting_key(
            &store,
            namespace_id,
        ) {
            Ok(groups) => groups,
            Err(err) => {
                debug!(%err, "failed to enumerate groups awaiting key");
                return;
            }
        };
        drop(store);
        if awaiting.is_empty() {
            return;
        }

        // Candidate key-holders: the peer we just synced with first (a
        // confirmed, connected member), then namespace-mesh subscribers.
        let topic =
            libp2p::gossipsub::TopicHash::from_raw(format!("ns/{}", hex::encode(namespace_id)));
        let mesh = self.sync_network.subscribed_peers(topic).await;
        let mut candidates: Vec<PeerId> = Vec::new();
        if let Some(p) = preferred_peer {
            candidates.push(p);
        }
        for p in mesh {
            if !candidates.contains(&p) {
                candidates.push(p);
            }
        }
        if candidates.is_empty() {
            return;
        }

        for group_id in awaiting {
            for peer in &candidates {
                let Some((envelope_bytes, responder_identity)) = self
                    .request_group_key_from_peer(
                        *peer,
                        namespace_id,
                        group_id,
                        requester_public_key,
                    )
                    .await
                else {
                    continue;
                };
                if envelope_bytes.is_empty() {
                    // This peer doesn't hold the key — try the next one.
                    continue;
                }
                let store = self.context_client.datastore_handle().into_inner();
                let outcome = calimero_context::group_store::apply_received_group_key(
                    &store,
                    namespace_id,
                    group_id,
                    &envelope_bytes,
                    responder_identity,
                );
                drop(store);
                match outcome {
                    Ok(divergence) => {
                        info!(
                            namespace_id = %hex::encode(namespace_id),
                            group_id = %hex::encode(group_id),
                            "recovered group key via direct delivery"
                        );
                        if let Some(report) = divergence {
                            self.reconcile_after_divergence(report).await;
                        }

                        // The arrived key may have made governance ops that were
                        // applied (and frozen as `Noop` in the unified op-store)
                        // before the key landed now decode to their real payload.
                        // Refresh the op-store from the governance DAG with the key
                        // present so its reconstruction matches the projection — the
                        // C2.2c fix for the read-flip's late-decrypted-membership gap.
                        // BEFORE the drain: the projection backs onto the op-store
                        // now (C2.2b), so the drain's membership re-checks must see the
                        // corrected ops, not the stale Noop.
                        let store = self.context_client.datastore_handle().into_inner();
                        calimero_context::scope_projection::ScopeProjections::repersist_namespace_ops(
                            &store,
                            namespace_id,
                        );
                        drop(store);

                        self.drain_governance_pending_after_sync().await;
                    }
                    Err(err) => {
                        warn!(
                            group_id = %hex::encode(group_id),
                            %err,
                            "failed to apply recovered group key"
                        );
                    }
                }
                // Got this group's key (or logged an apply error) — stop
                // trying peers for it.
                break;
            }
        }
    }

    /// Open a one-shot stream to `peer`, send a `GroupKeyRequest`, and return
    /// `(envelope_bytes, responder_identity)` from its `GroupKeyResponse`
    /// (empty envelope ⇒ peer holds no key). `None` on any transport error or
    /// unexpected reply.
    async fn request_group_key_from_peer(
        &self,
        peer: PeerId,
        namespace_id: [u8; 32],
        group_id: [u8; 32],
        requester_public_key: PublicKey,
    ) -> Option<(Vec<u8>, PublicKey)> {
        use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

        let mut stream = match self.sync_network.open_stream(peer).await {
            Ok(s) => s,
            Err(err) => {
                debug!(%err, "failed to open stream for group-key request");
                return None;
            }
        };

        let msg = StreamMessage::Init {
            context_id: calimero_primitives::context::ContextId::from([0u8; 32]),
            party_id: requester_public_key,
            payload: InitPayload::GroupKeyRequest {
                namespace_id,
                group_id,
                requester_public_key,
            },
            next_nonce: {
                use rand::Rng;
                rand::thread_rng().gen()
            },
        };

        if let Err(err) = crate::sync::stream::send(&mut stream, &msg, None).await {
            debug!(%err, "failed to send GroupKeyRequest");
            return None;
        }

        match crate::sync::stream::recv(&mut stream, None, self.sync_config.timeout).await {
            Ok(Some(StreamMessage::Message {
                payload:
                    MessagePayload::GroupKeyResponse {
                        key_envelope_bytes,
                        responder_identity,
                    },
                ..
            })) => Some((key_envelope_bytes, responder_identity)),
            Ok(other) => {
                debug!(
                    "unexpected response to GroupKeyRequest: {:?}",
                    other.as_ref().map(std::mem::discriminant)
                );
                None
            }
            Err(err) => {
                debug!(%err, "GroupKeyRequest recv failed");
                None
            }
        }
    }

    /// Responder for `InitPayload::GroupKeyRequest` — the pull-based
    /// counterpart to the admin push. A member that lacks a group key asks
    /// for it here; we authorise by current membership + cross-namespace pin,
    /// ECDH-wrap the key (`build_group_key_delivery`), and reply. Every
    /// non-deliverable case replies with an empty envelope (the requester
    /// tries another peer; no membership oracle leak).
    pub(super) async fn handle_group_key_request(
        &self,
        namespace_id: [u8; 32],
        group_id: [u8; 32],
        requester_public_key: PublicKey,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        use calimero_node_primitives::sync::{MessagePayload, StreamMessage};

        let store = self.context_client.datastore_handle().into_inner();
        let (key_envelope_bytes, responder_identity) =
            match calimero_context::group_store::build_group_key_delivery(
                &store,
                namespace_id,
                group_id,
                requester_public_key,
            ) {
                Ok(pair) => pair,
                Err(err) => {
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        group_id = %hex::encode(group_id),
                        %err,
                        "failed to build group-key delivery"
                    );
                    (Vec::new(), requester_public_key)
                }
            };
        drop(store);

        debug!(
            namespace_id = %hex::encode(namespace_id),
            group_id = %hex::encode(group_id),
            has_key = !key_envelope_bytes.is_empty(),
            "Sending GroupKeyResponse"
        );

        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::GroupKeyResponse {
                key_envelope_bytes,
                responder_identity,
            },
            next_nonce: nonce,
        };
        crate::sync::stream::send(stream, &msg, None).await?;
        Ok(())
    }
}

/// Pure trigger predicate for the #2625 governance-pending backfill: the
/// interval sync should pull the namespace governance DAG iff the context
/// has at least one delta parked in the governance-pending buffer.
///
/// Extracted as a free function so the trigger condition is unit-testable
/// without standing up a `SyncManager` + network stack — the regression we
/// guard against is silently dropping the trigger (e.g. inverting the
/// comparison), which would let a cross-DAG-buffered delta wedge a context
/// into permanent split-brain again.
pub(super) const fn should_backfill_governance(pending_len: usize) -> bool {
    pending_len > 0
}

/// Resolve the namespace-root id (bytes) that owns `context_id`, walking from
/// the context's immediate owning group up to the namespace root. Returns
/// `None` for non-group (legacy) contexts whose `ContextGroupRef` is absent,
/// or on a namespace-resolution error.
///
/// Mirrors `ContextClient::get_context_group_id` (reads `ContextGroupRef`)
/// followed by `NamespaceRepository::resolve`, but as a free function over
/// `&Store` so it is unit-testable. Unlike the interval-sync fallback-topic
/// closure it does NOT best-effort fall back to the immediate group id: the
/// #2625 backfill must pull the *correct* namespace DAG, and a wrong id would
/// silently fail to converge rather than fetch the missing governance op.
pub(super) fn resolve_namespace_id(
    store: &calimero_store::Store,
    context_id: &ContextId,
) -> Option<[u8; 32]> {
    let handle = store.handle();
    let group_id: [u8; 32] = handle
        .get(&calimero_store::key::ContextGroupRef::new(*context_id))
        .ok()??;
    NamespaceRepository::new(store)
        .resolve(&calimero_context_config::types::ContextGroupId::from(
            group_id,
        ))
        .map(|id| id.to_bytes())
        .ok()
}
