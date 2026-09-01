//! Namespace sync flows for [`SyncManager`]: governance catch-up + backfill,
//! and namespace / open-subgroup join (request and initiate sides).
//!
//! Extracted from the manager god-file as a self-contained `impl SyncManager`
//! block, driven by the stream dispatcher and the `SyncDriverDispatch` trait.
//! Methods that stay in `mod.rs` remain reachable here via ancestor privacy.

use calimero_crypto::Nonce;
use calimero_governance_store::{
    CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository, NamespaceRepository,
};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::{NamespaceJoinParams, OpenSubgroupJoinParams};
use calimero_node_primitives::join_bundle::JoinBundle;
use calimero_node_primitives::sync::{InitPayload, InitProof, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use libp2p::PeerId;
use rand::Rng;
use tokio::time;
use tracing::{debug, info, warn};

use super::SyncManager;
use crate::sync::MAX_BACKFILL_OPS;

/// The op kinds in a backfill response, for logging.
///
/// Decodes only far enough to name each op — an undecodable entry is reported
/// as such rather than dropped, since "the responder sent something this build
/// cannot read" is itself the answer when a backfill looks complete but leaves
/// the receiver missing an op.
fn backfill_op_kinds(deltas: &[([u8; 32], Vec<u8>)]) -> String {
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};

    let mut kinds = Vec::with_capacity(deltas.len());
    for (_delta_id, bytes) in deltas {
        kinds.push(match borsh::from_slice::<SignedNamespaceOp>(bytes) {
            Ok(op) => match op.op {
                NamespaceOp::Root(root) => {
                    let named = format!("{root:?}");
                    named
                        .split(|c: char| c == '{' || c == '(' || c.is_whitespace())
                        .next()
                        .unwrap_or("Root")
                        .to_owned()
                }
                // Encrypted; the inner kind is not readable without the key,
                // which is frequently the very thing that is missing.
                NamespaceOp::Group { .. } => "Group(encrypted)".to_owned(),
                _ => "Unknown".to_owned(),
            },
            Err(_) => "undecodable".to_owned(),
        });
    }
    kinds.join(",")
}

/// What one walk over the mesh learned about who holds a subgroup key.
///
/// The two failure variants exist to keep a distinction the old single-pass code
/// collapsed: whether the round's picture of "who holds the key" is COMPLETE.
/// A key-less reply and a rejection are answers, and re-asking yields the same
/// ones. A peer that never answered leaves a gap that a retry can close — and
/// with a single key holder, that gap is the difference between a join that
/// succeeds a moment later and one that fails permanently.
enum KeyFetchRound {
    /// A peer served the key envelope.
    Key(Vec<u8>),
    /// Every peer answered, and none held the key. Terminal.
    NobodyHasIt {
        tally: String,
        last_rejection: Option<String>,
    },
    /// At least one peer never answered (stream open, send, or recv failed), so
    /// the round cannot rule out that the holder is simply unreachable right now.
    Unanswered {
        tally: String,
        last_rejection: Option<String>,
    },
}

impl KeyFetchRound {
    /// The per-peer tally, for logging a round that is about to be retried.
    fn tally(&self) -> &str {
        match self {
            Self::Key(_) => "key served",
            Self::NobodyHasIt { tally, .. } | Self::Unanswered { tally, .. } => tally,
        }
    }
}

/// The group a join request is actually for.
///
/// The stream is keyed by NAMESPACE, but the invitation names the group being
/// joined, which is a subgroup whenever the invite was issued on one. The two
/// coincide only for a namespace invitation, so reading the request's id
/// instead serves the wrong group's key bound to the wrong scope, and the
/// joiner's unwrap refuses the envelope it is sent.
///
/// A signed invitation is authority over the group it names and nothing else,
/// so that group has to resolve back to the namespace being served.
fn join_target_group(
    store: &calimero_store::Store,
    namespace: calimero_context_config::types::ContextGroupId,
    invitation: &calimero_context_config::types::SignedGroupOpenInvitation,
) -> Result<calimero_context_config::types::ContextGroupId, String> {
    let group_id = invitation.invitation.group_id;
    match calimero_governance_store::NamespaceRepository::new(store).resolve(&group_id) {
        Ok(owner) if owner == namespace => Ok(group_id),
        Ok(_) => Err("invitation names a group outside this namespace".to_owned()),
        Err(err) => Err(format!("could not resolve the invited group: {err}")),
    }
}

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
        let namespace_id = match calimero_governance_store::get_group_for_context(store, context_id)
        {
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
            // Sentinel party id (no owned key to prove); backfill serves only
            // already-signed deltas, which the requester re-verifies on receipt.
            pop: None,
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
            self.sync_namespace_from_peer(namespace_id, Some(peer), None)
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
                self.sync_namespace_from_peer(namespace_id, Some(peer), None)
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
                // `GroupDeviceBinding` shares this key's exact layout, so a
                // binding row parses here and — on a namespace root, where the
                // group id IS the namespace id — passes the id check too. Stop
                // on the family, never on width plus id.
                if !key.is_gov_op_row() {
                    break;
                }
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

    /// Name the joiner behind a `NamespaceJoinRequest`, or say why not.
    ///
    /// Verification only — nothing is written. The responder needs the account
    /// before it decides whether to serve anything, and applying the binding
    /// first would let an unauthorized request mutate state on its way to being
    /// refused.
    ///
    /// Three things have to hold, and the third is the one that is easy to miss:
    ///
    /// 1. the genesis hashes to the account the certificate claims, so the
    ///    credential cannot name an account it does not descend from;
    /// 2. the certificate chains to that genesis through the root-key handoffs
    ///    (`verify_device_cert`, the same check the apply path runs);
    /// 3. the certificate names THIS request's `joiner_public_key`. Without it a
    ///    credential is a bearer token: anyone who observed one could replay it
    ///    and be admitted as its owner, which is a worse hole than the one this
    ///    check closes.
    fn verified_joiner_account(
        credential_bytes: &[u8],
        joiner_public_key: &PublicKey,
    ) -> Result<calimero_account::AccountId, String> {
        let credential: calimero_context_client::local_governance::JoinAccountCredential =
            borsh::from_slice(credential_bytes).map_err(|e| format!("undecodable: {e}"))?;

        if credential.genesis.account_id() != credential.statement.account {
            return Err("genesis does not derive the account the certificate claims".to_owned());
        }

        let verified = calimero_account::verify_device_cert(
            credential.statement.account,
            &credential.genesis,
            &credential.chain,
            &credential.statement,
        )
        .map_err(|e| format!("{e}"))?;

        if AsRef::<[u8; 32]>::as_ref(&verified.sign_pk)
            != AsRef::<[u8; 32]>::as_ref(joiner_public_key)
        {
            return Err("certificate names a different signing key than the request".to_owned());
        }

        Ok(credential.statement.account)
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
        joiner_credential_bytes: &[u8],
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        use calimero_context_config::types::ContextGroupId;
        use calimero_context_config::types::SignedGroupOpenInvitation;
        use calimero_governance_store::enumerate_group_contexts;
        use calimero_governance_store::NamespaceMembershipService;
        use calimero_governance_store::ReentryRepository;

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

        let namespace = ContextGroupId::from(namespace_id);
        let store = self.context_client.datastore_handle().into_inner();

        let group_id = match join_target_group(&store, namespace, &invitation) {
            Ok(target) => target,
            Err(reason) => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::NamespaceJoinRejected { reason },
                    next_nonce: nonce,
                };
                crate::sync::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
        };

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
        let now_secs = calimero_governance_store::now_secs();
        if let Err(err) = NamespaceMembershipService::new(&store, namespace_id.into())
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

        // Re-entry gate, and it is load-bearing rather than a nicety. Two things
        // hang off it:
        //
        //   1. The pre-register below calls `add_member`, which retracts the
        //      joiner's deny-list entry at the membership choke point. Without
        //      this gate a removed member could open a join stream and un-silence
        //      themselves on this node — their `MemberJoined` op would still be
        //      rejected at apply, but their state deltas would already be flowing
        //      past the receive filter.
        //   2. It runs before the group key is wrapped, so a removed member does
        //      not get handed the current key on the way to being rejected.
        //
        // The apply-path checks remain the authority — a joiner can publish
        // `MemberJoined` straight to gossip and never open a stream at all. This
        // gate is what makes the rejection land as a clean, diagnosable error on
        // the joiner instead of a silent stall.
        //
        // Skipped for an identity that is already a member: the block governs
        // RE-ENTRY, and a current member is not re-entering. They land here on a
        // perfectly ordinary re-sync or a retried join round — and since a
        // successful join consumes the invitation, gating them would reject every
        // repeat request they ever make with their own invitation.
        // The joiner is named by KEY on the wire and the rows are keyed by
        // ACCOUNT, so the request carries the credential that bridges the two —
        // and this responder verifies it rather than trusting it.
        //
        // Refusing an unverifiable credential is the whole point. When the gate
        // could not name a requester it admitted them, so a denied account that
        // presented a device this responder held no binding for had its deny row
        // go unread, and collected the backfill and the wrapped group key ahead
        // of the apply-time check that does reject it.
        let joiner_account =
            match Self::verified_joiner_account(joiner_credential_bytes, &joiner_public_key) {
                Ok(account) => account,
                Err(reason) => {
                    let msg = StreamMessage::Message {
                        sequence_id: 0,
                        payload: MessagePayload::NamespaceJoinRejected {
                            reason: format!("join credential rejected: {reason}"),
                        },
                        next_nonce: nonce,
                    };
                    crate::sync::stream::send(stream, &msg, None).await?;
                    return Ok(());
                }
            };

        let already_member = MembershipRepository::new(&store)
            .has_direct_member(&group_id, &joiner_account)
            .unwrap_or(false);
        // Skipped for an identity that is already a member: the block governs
        // RE-ENTRY, and a current member is not re-entering. They land here on a
        // perfectly ordinary re-sync or a retried join round — and since a
        // successful join consumes the invitation, gating them would reject every
        // repeat request they ever make with their own invitation.
        let admission = if already_member {
            Ok(())
        } else {
            ReentryRepository::new(&store).require_invitation_admits(
                &group_id,
                &joiner_account,
                invitation.invitation.invitation_nonce,
            )
        };
        if let Err(err) = admission {
            warn!(
                namespace_id = %hex::encode(namespace_id),
                %joiner_public_key,
                %err,
                "rejecting namespace join: joiner may not re-enter this group"
            );
            let msg = StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::NamespaceJoinRejected {
                    reason: format!("{err}"),
                },
                next_nonce: nonce,
            };
            crate::sync::stream::send(stream, &msg, None).await?;
            return Ok(());
        }

        let key_envelope_bytes = match GroupKeyring::new(&store, group_id).load_current_key()? {
            Some((_key_id, group_key)) => {
                let ns_identity =
                    NamespaceRepository::new(&store).resolve_identity_record(&namespace)?;
                match ns_identity {
                    Some(record) => {
                        let sender_sk =
                            calimero_primitives::identity::PrivateKey::from(record.private_key);
                        match GroupKeyring::wrap_for_member(
                            &sender_sk,
                            &joiner_public_key,
                            &group_id.to_bytes(),
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

        // Pre-register the joiner as a group member so that when it opens a sync
        // stream, this node's membership check passes immediately.
        //
        // Unconditional now: the request carries a verified credential, so every
        // joiner that reaches this line — first-timer included — has an account
        // to key the row under. It used to be skipped whenever the account could
        // not be named, which was exactly the first-join case the optimisation
        // exists for.
        if let Err(e) = MembershipRepository::new(&store).add_member(
            &group_id,
            &joiner_account,
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
                    &calimero_store::types::ContextIdentity { private_key: None },
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
            .default_capabilities(&namespace)?
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
        use calimero_context_config::types::ContextGroupId;
        use calimero_governance_store::MembershipPath;

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
        let Some(joiner_account) = calimero_governance_store::member_account_in_namespace(
            &store,
            &subgroup_gid,
            &joiner_public_key,
        )?
        else {
            // A key bound to no account reaches the subgroup by no path.
            return Err(eyre::eyre!(
                "joiner identity is bound to no account in this namespace"
            ));
        };
        match MembershipRepository::new(&store).check_path(&subgroup_gid, &joiner_account)? {
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
                            &subgroup_gid.to_bytes(),
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
    /// Sign the transport-binding proof for a namespace/subgroup join `Init`.
    ///
    /// The joiner's key lives in the namespace identity store (keyed by the
    /// namespace group id), not the per-context identity store, so this loads
    /// it directly rather than via [`SyncManager::build_init_pop`]. The proof
    /// binds the zero context id — matching the sentinel the join `Init`
    /// carries — plus `joiner_public_key` and this node's transport `PeerId`,
    /// so the responder can confirm the caller controls the identity it is
    /// about to pre-register. See [`InitProof`].
    async fn build_join_init_pop(
        &self,
        namespace_id: [u8; 32],
        joiner_public_key: PublicKey,
    ) -> Option<InitProof> {
        use zeroize::Zeroize;
        let store = self.context_client.datastore_handle().into_inner();
        let group_id = calimero_context_config::types::ContextGroupId::from(namespace_id);
        let mut record = NamespaceRepository::new(&store)
            .resolve_identity_record(&group_id)
            .ok()
            .flatten()?;
        let private_key = calimero_primitives::identity::PrivateKey::from(record.private_key);
        // `PrivateKey::from` copies into its own zeroizing wrapper; wipe the
        // plain `[u8; 32]` copy left in the record struct on the stack (mirrors
        // the discipline in join_namespace / request_missing_deltas).
        record.private_key.zeroize();
        // Bind the proof to the target namespace (the join `Init` itself carries
        // a sentinel context_id) so it can't be replayed against a different
        // namespace; the responder verifies against the same namespace id.
        self.sign_init_pop(
            ContextId::from(namespace_id),
            joiner_public_key,
            &private_key,
        )
        .await
    }

    pub(super) async fn initiate_open_subgroup_join(
        &self,
        params: OpenSubgroupJoinParams,
    ) -> eyre::Result<Vec<u8>> {
        let join_pop = self
            .build_join_init_pop(params.namespace_id, params.joiner_public_key)
            .await;
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

        fetch_open_subgroup_key(
            self.sync_network.as_ref(),
            &topic,
            &params,
            join_pop,
            peers,
            self.sync_config.timeout,
            crate::sync::config::OPEN_SUBGROUP_JOIN_KEY_ROUNDS,
            std::time::Duration::from_millis(
                crate::sync::config::OPEN_SUBGROUP_JOIN_KEY_RETRY_DELAY_MS,
            ),
        )
        .await
    }
    /// Collect all governance ops for a namespace (reused by the join responder).
    ///
    /// Returns bare `SignedNamespaceOp` bytes (not `StoredNamespaceEntry` wrapped)
    /// so recipients can `borsh::from_slice::<SignedNamespaceOp>` directly.
    fn collect_namespace_governance_ops(
        &self,
        namespace_id: [u8; 32],
    ) -> eyre::Result<Vec<Vec<u8>>> {
        // Bound how much this collects into memory / one response, mirroring the
        // backfill path's cap. Without it a namespace with a very long (or
        // maliciously inflated) governance-op history is read fully into RAM and
        // shipped in a single message.
        const MAX_COLLECT_OPS: usize = 500;
        const MAX_COLLECT_BYTES: usize = 8 * 1024 * 1024;

        let store = self.context_client.datastore_handle().into_inner();
        let handle = store.handle();
        let mut ops = Vec::new();
        let mut total_bytes = 0usize;

        let start = calimero_store::key::NamespaceGovOp::new(namespace_id, [0u8; 32]);
        let mut iter = handle.iter::<calimero_store::key::NamespaceGovOp>()?;
        let first = iter.seek(start).transpose();

        for entry in first.into_iter().chain(iter.keys()) {
            let key = match entry {
                Ok(k) => k,
                Err(_) => break,
            };
            // See the sibling walk above: a same-layout binding row would
            // otherwise be read as a gov op.
            if !key.is_gov_op_row() {
                break;
            }
            if key.namespace_id() != namespace_id {
                break;
            }
            if ops.len() >= MAX_COLLECT_OPS || total_bytes >= MAX_COLLECT_BYTES {
                warn!(
                    namespace_id = %hex::encode(namespace_id),
                    ops = ops.len(),
                    total_bytes,
                    "collect_namespace_governance_ops hit cap; truncating response"
                );
                break;
            }
            if let Ok(Some(value)) = handle.get(&key) {
                if let Some(bytes) =
                    crate::sync::helpers::extract_signed_op_bytes(&value.skeleton_bytes)
                {
                    total_bytes = total_bytes.saturating_add(bytes.len());
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
        let join_pop = self
            .build_join_init_pop(params.namespace_id, params.joiner_public_key)
            .await;
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
                self.sync_config.namespace_discovery_wait,
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
                pop: join_pop,
                payload: InitPayload::NamespaceJoinRequest {
                    namespace_id: params.namespace_id,
                    invitation_bytes: params.invitation_bytes.clone(),
                    joiner_public_key: params.joiner_public_key,
                    joiner_credential_bytes: params.joiner_credential_bytes.clone(),
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
    /// Pull the namespace governance DAG from `peer` (or a mesh peer when `None`).
    /// Returns the number of governance ops received in the backfill response — `0`
    /// on any best-effort failure (no peer, stream/​send/​recv error, unexpected
    /// response), so a caller correcting a divergence can tell whether the pull
    /// actually delivered anything rather than treating it as a silent no-op.
    ///
    /// `fallback` is a peer to use **only when discovery finds nobody**, and it is
    /// deliberately a separate argument from `peer` rather than a second way to
    /// spell the same thing. `peer = Some(_)` means "ask this one and no other";
    /// `fallback` means "prefer a subscriber, but do not give up if the subscriber
    /// table is empty".
    ///
    /// That distinction is load-bearing because the table can be empty while a
    /// perfectly good peer is talking to us. Discovery reads gossipsub's record of
    /// who subscribes to the namespace topic, and that record is not always
    /// complete — a node that restarts with an unchanged `PeerId` may never be
    /// told what its peers follow (see `calimero_network`'s `subscription_repair`).
    /// Without a fallback this returns `0` while a reachable, caught-up member is
    /// beaconing at us every few seconds, and a governance op that needs a parent
    /// stays buffered for want of anyone to ask.
    ///
    /// Only pass a `fallback` that is known to hold this namespace: a peer whose
    /// readiness beacon just verified qualifies, an arbitrary connected peer does
    /// not — a non-member stores only the [`StoredNamespaceEntry::Opaque`]
    /// skeleton, so pulling from one delivers nothing and burns the attempt.
    pub(crate) async fn sync_namespace_from_peer(
        &self,
        namespace_id: [u8; 32],
        peer: Option<PeerId>,
        fallback: Option<PeerId>,
    ) -> usize {
        use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

        let peer = match peer {
            Some(p) => p,
            None => {
                match discover_namespace_pull_peer(
                    self.sync_network.as_ref(),
                    namespace_id,
                    fallback,
                )
                .await
                {
                    NamespacePullPeer::Subscriber(p) => p,
                    // Logged unconditionally, and at `info`: this is the branch
                    // that says discovery was broken and something else carried
                    // the pull, which is exactly what a later reader needs to
                    // tell "the fallback rescued it" from "it never ran".
                    NamespacePullPeer::Fallback(p) => {
                        info!(
                            namespace_id = %hex::encode(namespace_id),
                            peer = %p,
                            "no subscribers for the namespace topic; falling back to a peer \
                             known to hold this namespace"
                        );
                        p
                    }
                    NamespacePullPeer::Nobody => {
                        debug!(
                            namespace_id = %hex::encode(namespace_id),
                            "no mesh peers for namespace sync"
                        );
                        return 0;
                    }
                }
            }
        };

        let Ok(mut stream) = self.sync_network.open_stream(peer).await else {
            debug!("failed to open stream for namespace sync");
            return 0;
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
            // Sentinel party id; see the catch-up backfill site above.
            pop: None,
        };

        if let Err(err) = crate::sync::stream::send(&mut stream, &msg, None).await {
            debug!(%err, "failed to send NamespaceBackfillRequest");
            return 0;
        }

        match crate::sync::stream::recv(&mut stream, None, self.sync_config.timeout).await {
            Ok(Some(StreamMessage::Message {
                payload: MessagePayload::NamespaceBackfillResponse { deltas },
                ..
            })) => {
                let ops_received = deltas.len();
                // The kinds, not just the count. A backfill that returns the
                // same tally every time is ambiguous in exactly the way that
                // matters: an op the responder never had looks identical to one
                // it served and this node dropped, and telling those apart
                // otherwise means correlating two nodes' logs by timestamp and
                // guessing. A device waiting on a `KeyDelivery` it missed on
                // gossip is the case that made this worth logging.
                debug!(
                    namespace_id = %hex::encode(namespace_id),
                    kinds = %backfill_op_kinds(&deltas),
                    "namespace backfill contents"
                );
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
                                        // `NamespaceOp` is `#[non_exhaustive]`.
                                        _ => "Unknown".to_owned(),
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
                ops_received
            }
            _ => {
                debug!("unexpected response to namespace sync request");
                0
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

        // The device we ask as — MINTED here if this node has none yet, not just
        // read.
        //
        // A responder that knows an account for our identity serves that account's
        // devices and nothing else: identity addressing cannot be a fallback,
        // because a revoked device would simply omit its id and be handed the very
        // key the rotation excluded it from. So asking without a device is asking
        // for nothing, and a node that has not enrolled yet would sit keyless —
        // unable to decrypt any group op — until something else happened to enrol
        // it.
        //
        // Read before minting, and mint only when there is nothing to read.
        // `ensure_enrolled` is idempotent only for a device of THIS node's own
        // account: handed a row belonging to another account it releases the slot
        // and mints a replacement, which is exactly what a paired device is. So
        // calling it unconditionally destroyed the pairing on the first pull —
        // and the key already in flight named the device it destroyed, so it
        // arrived, matched nothing, and was dropped. The link op that would have
        // protected the row is itself encrypted under that key, so the pairing
        // could never recover; the next pull just did it again.
        //
        // Asking as a device we already are is right regardless of whose account
        // it speaks for: the point is to be addressable, not to be ourselves.
        //
        // Unless it has been REVOKED. Releasing a revoked row so a fresh device
        // is minted is the one replacement `ensure_enrolled` must still perform —
        // a node that revoked itself out of the namespace re-enters under a new
        // id, and reusing the revoked one would ask for keys the revocation
        // exists to withhold. Reading past that check skipped it, and the node
        // came back as the device it had just revoked.
        let devices = calimero_governance_store::NodeDeviceRepository::new(&store);
        let device = match devices.reusable_device(&ns_gid) {
            Ok(Some(existing)) => Some(existing.secret.device),
            Ok(None) => devices
                .ensure_enrolled(&ns_gid)
                .map(|own| Some(own.secret.device))
                .unwrap_or_else(|err| {
                    debug!(%err, "failed to enrol this node's device for key recovery");
                    None
                }),
            Err(err) => {
                debug!(%err, "failed to read this node's device for key recovery");
                None
            }
        };
        let requester = calimero_governance_store::KeyRequester {
            identity: requester_public_key,
            device,
        };

        // `(group_id, key_id)` pairs we're stranded on — we ask each peer for
        // the EXACT key epoch a buffered op needs, so a rotated-out key can be
        // recovered (a current-key-only request could not deliver it).
        let awaiting = match calimero_governance_store::namespace_group_keys_awaiting(
            &store,
            namespace_id.into(),
        ) {
            Ok(pairs) => pairs,
            Err(err) => {
                debug!(%err, "failed to enumerate group keys awaiting");
                return;
            }
        };
        // Membership-driven set (#3295): groups we're a member of but hold no
        // key for, even with no buffered op. Without this, a joiner stranded
        // "joined, pending key" under a quiescent namespace never appears in
        // the op-driven `awaiting` set above and never recovers. Non-fatal on
        // error — fall back to the op-driven set alone.
        let member_keyless = match calimero_governance_store::namespace_groups_member_but_keyless(
            &store,
            namespace_id.into(),
        ) {
            Ok(groups) => groups,
            Err(err) => {
                debug!(%err, "failed to enumerate member-but-keyless groups");
                Vec::new()
            }
        };
        drop(store);

        // Merge into one request list of `(group_id, Option<key_id>)`: op-driven
        // pairs target the EXACT stranded epoch (`Some`), membership-driven
        // groups ask for the group's CURRENT key (`None`) since no op pins an
        // epoch. Skip a membership group already covered by an op-driven pair to
        // avoid a redundant request.
        let op_group_ids: std::collections::BTreeSet<[u8; 32]> =
            awaiting.iter().map(|(g, _)| *g).collect();
        let mut requests: Vec<([u8; 32], Option<[u8; 32]>)> =
            awaiting.into_iter().map(|(g, k)| (g, Some(k))).collect();
        for g in member_keyless {
            if !op_group_ids.contains(&g) {
                requests.push((g, None));
            }
        }
        if requests.is_empty() {
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

        for (group_id, key_id) in requests {
            for peer in &candidates {
                let Some((envelope_bytes, responder_identity)) = self
                    .request_group_key_from_peer(*peer, namespace_id, group_id, requester, key_id)
                    .await
                else {
                    continue;
                };
                if envelope_bytes.is_empty() {
                    // This peer doesn't hold the key — try the next one.
                    continue;
                }
                let store = self.context_client.datastore_handle().into_inner();
                let outcome = calimero_governance_store::apply_received_group_key(
                    &store,
                    namespace_id.into(),
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

                        // The arrived key may have made governance ops applied (and
                        // frozen as `Noop`) before the key landed now decode to their
                        // real payload. Two-step refresh so the projection — which backs
                        // onto the op-store after the C3 Stage 4 flip — reflects the
                        // membership: (1) re-persist the op-store from the gov-DAG with
                        // the key present, then (2) re-ingest the corrected ops into the
                        // MAINTAINED projection (`ingest_op` upgrades the stale `Noop`
                        // op-log entries in place). BEFORE the drain, whose membership
                        // re-checks must see the corrected ops, not the stale `Noop`.
                        //
                        // The in-process re-ingest reads the gov-DAG fold
                        // (`collect_namespace_ops`), NOT the op-store read-back: the
                        // re-persist is best-effort, so a partial `persist_op` failure
                        // must not leave the maintained projection with fewer ops than
                        // the gov-DAG (the exact gap the flip closes). We have the
                        // freshly-decrypted fold in hand here; the op-store mirror is
                        // for the cold-start read path.
                        let store = self.context_client.datastore_handle().into_inner();
                        calimero_context::scope_projection::ScopeProjections::repersist_namespace_ops(
                            &store,
                            namespace_id,
                        );
                        let refreshed =
                            calimero_context::scope_projection::ScopeProjections::collect_namespace_ops(
                                &store,
                                namespace_id,
                            );
                        drop(store);
                        if let Some(ops) = refreshed {
                            self.node_state
                                .write_scope_projections()
                                .apply_backfill(namespace_id, ops);
                        }

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
        requester: calimero_governance_store::KeyRequester,
        key_id: Option<[u8; 32]>,
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
            party_id: requester.identity,
            payload: InitPayload::GroupKeyRequest {
                namespace_id,
                group_id,
                requester_public_key: requester.identity,
                requester_device: requester.device,
                key_id,
            },
            next_nonce: {
                use rand::Rng;
                rand::thread_rng().gen()
            },
            // The response is an ECDH key envelope sealed either to
            // `requester_public_key` or to `requester_device`'s certified KEM
            // key; only the holder of the matching secret can unwrap it, so
            // possession is proven implicitly and no signed proof is required on
            // this path — including for `requester_device`, which is why it needs
            // no authentication of its own.
            pop: None,
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
        requester: calimero_governance_store::KeyRequester,
        requested_key_id: Option<[u8; 32]>,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        use calimero_node_primitives::sync::{MessagePayload, StreamMessage};

        let store = self.context_client.datastore_handle().into_inner();
        let (key_envelope_bytes, responder_identity) =
            match calimero_governance_store::build_group_key_delivery(
                &store,
                namespace_id.into(),
                group_id,
                requester,
                requested_key_id,
            ) {
                Ok(pair) => pair,
                Err(err) => {
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        group_id = %hex::encode(group_id),
                        %err,
                        "failed to build group-key delivery"
                    );
                    (Vec::new(), requester.identity)
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

/// Walk the mesh for the subgroup key, in bounded ROUNDS.
///
/// Split out of `initiate_open_subgroup_join`, and parameterised on `rounds` /
/// `retry_delay` rather than reading the constants directly, so a test can drive
/// the retry on a virtual clock (the shape `sync::peers`' discovery loop uses).
#[allow(
    clippy::too_many_arguments,
    reason = "every knob is injected so the retry is testable without booting a node"
)]
async fn fetch_open_subgroup_key(
    network: &dyn crate::sync::network::SyncNetwork,
    topic: &libp2p::gossipsub::TopicHash,
    params: &OpenSubgroupJoinParams,
    join_pop: Option<InitProof>,
    mut peers: Vec<PeerId>,
    recv_timeout: std::time::Duration,
    rounds: u32,
    retry_delay: std::time::Duration,
) -> eyre::Result<Vec<u8>> {
    // Walk the mesh in bounded ROUNDS, not once.
    //
    // The distinction the rounds exist for: a key-less reply is an ANSWER —
    // that peer genuinely does not hold the subgroup key, and asking again
    // cannot change it. A transport failure is not an answer at all. Treating
    // the two alike made one dropped stream to the sole key holder
    // indistinguishable from "nobody has the key", which fails the join
    // permanently — and right after a subgroup is created there IS exactly one
    // holder (the creator), so that is the normal shape rather than an edge.
    //
    // So a round that ends with every peer having answered fails immediately
    // (no latency added to the genuine "nobody has it" case), and only a round
    // left incomplete by a transport failure is retried.
    let mut last_round: Option<KeyFetchRound> = None;
    for round in 1..=rounds {
        if round > 1 {
            time::sleep(retry_delay).await;
            // Re-read the subscriber set: the holder may have only just joined
            // the mesh, and a peer that transport-failed may be gone from it.
            // Keep the previous list if the fresh read is empty rather than
            // turning a retry into an immediate "no mesh peers" failure.
            let fresh = network.subscribed_peers(topic.clone()).await;
            if !fresh.is_empty() {
                peers = fresh;
            }
        }

        match fetch_open_subgroup_key_once(network, params, join_pop, &peers, recv_timeout).await {
            KeyFetchRound::Key(bytes) => return Ok(bytes),
            // Everybody answered, and nobody has it. Retrying would re-ask the
            // same peers the same question and get the same answer.
            outcome @ KeyFetchRound::NobodyHasIt { .. } => {
                last_round = Some(outcome);
                break;
            }
            outcome @ KeyFetchRound::Unanswered { .. } => {
                debug!(
                    subgroup_id = %hex::encode(params.subgroup_id),
                    round,
                    rounds = rounds,
                    tally = %outcome.tally(),
                    "open-subgroup join: round left unanswered by a transport \
                     failure, retrying"
                );
                last_round = Some(outcome);
            }
        }
    }

    // No peer yielded the key. Surface the most informative cause, always
    // including the full per-peer tally so a mixed failure (some peers
    // key-less, one rejecting, some transport errors) is diagnosable from a
    // single line — and say how many rounds were spent, so a retried failure
    // is distinguishable from a fail-fast one.
    let (tally, last_rejection) = match last_round {
        Some(KeyFetchRound::NobodyHasIt {
            tally,
            last_rejection,
        })
        | Some(KeyFetchRound::Unanswered {
            tally,
            last_rejection,
        }) => (tally, last_rejection),
        // Unreachable: the loop runs at least once and the `Key` arm returns.
        _ => (String::from("no rounds run"), None),
    };
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

/// One walk over `peers`, asking each for the subgroup key.
///
/// Returns on the first peer that yields one. The caller decides whether to walk
/// Which peer a namespace pull should go to when the caller named none.
///
/// Three outcomes rather than an `Option<PeerId>`, so the *reason* survives to
/// the caller: "a subscriber" and "the fallback" both yield a peer, but only the
/// second one means discovery was broken, and that difference is what the log
/// has to say out loud.
#[derive(Debug, Eq, PartialEq)]
enum NamespacePullPeer {
    /// Discovery worked: a peer gossipsub records as following the namespace topic.
    Subscriber(PeerId),
    /// Discovery found nobody, and the caller supplied someone known to hold the
    /// namespace. Reaching this arm means the subscriber table is wrong, not that
    /// the node is alone.
    Fallback(PeerId),
    /// Nobody to ask.
    Nobody,
}

/// Pick a peer to pull namespace governance from, preferring a real subscriber.
///
/// A free function over the [`SyncNetwork`](crate::sync::network::SyncNetwork)
/// trait rather than a `SyncManager` method, so a test can drive the empty-table
/// case against a scripted mock instead of booting a node — the same shape
/// [`fetch_open_subgroup_key_once`] uses.
async fn discover_namespace_pull_peer(
    network: &dyn crate::sync::network::SyncNetwork,
    namespace_id: [u8; 32],
    fallback: Option<PeerId>,
) -> NamespacePullPeer {
    let topic = libp2p::gossipsub::TopicHash::from_raw(format!("ns/{}", hex::encode(namespace_id)));
    let peers = network.subscribed_peers(topic).await;
    match (peers.first().copied(), fallback) {
        (Some(p), _) => NamespacePullPeer::Subscriber(p),
        (None, Some(p)) => NamespacePullPeer::Fallback(p),
        (None, None) => NamespacePullPeer::Nobody,
    }
}

/// again, which is why the outcome distinguishes "everybody answered and nobody
/// has it" from "somebody never answered" — see [`KeyFetchRound`].
///
/// A free function over the [`SyncNetwork`] trait rather than a `SyncManager`
/// method, so a test can drive it with a scripted mock instead of booting a node
/// (the same shape `sync::peers`' discovery loop uses).
async fn fetch_open_subgroup_key_once(
    network: &dyn crate::sync::network::SyncNetwork,
    params: &OpenSubgroupJoinParams,
    join_pop: Option<InitProof>,
    peers: &[PeerId],
    recv_timeout: std::time::Duration,
) -> KeyFetchRound {
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

    for peer in peers {
        let mut stream = match network.open_stream(*peer).await {
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
            pop: join_pop,
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

        match crate::sync::stream::recv(&mut stream, None, recv_timeout).await {
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
                return KeyFetchRound::Key(key_envelope_bytes);
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

    let tally = format!(
        "{} peer(s): {} key-less, {} transport error(s)",
        peers.len(),
        keyless_peers,
        transport_errors
    );
    if transport_errors == 0 {
        KeyFetchRound::NobodyHasIt {
            tally,
            last_rejection,
        }
    } else {
        KeyFetchRound::Unanswered {
            tally,
            last_rejection,
        }
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

#[cfg(test)]
mod open_subgroup_key_tests {
    //! The distinction the round loop exists for: a key-less reply is an ANSWER,
    //! a transport failure is not.
    //!
    //! These drive [`fetch_open_subgroup_key_once`] against a scripted
    //! [`MockSyncNetwork`] rather than a booted node — the shape `sync::peers`
    //! uses — so the classification is asserted directly instead of inferred from
    //! whether a join happened to succeed.

    use std::time::Duration;

    use calimero_network_primitives::stream::Stream;
    use calimero_primitives::identity::PublicKey;

    use libp2p::gossipsub::TopicHash;

    use super::{
        fetch_open_subgroup_key, fetch_open_subgroup_key_once, KeyFetchRound, MessagePayload,
        OpenSubgroupJoinParams, PeerId, StreamMessage,
    };
    use crate::sync::network::mock::MockSyncNetwork;

    fn peer(n: u8) -> PeerId {
        let kp = libp2p::identity::Keypair::ed25519_from_bytes([n; 32]).expect("valid seed");
        PeerId::from_public_key(&kp.public())
    }

    fn params() -> OpenSubgroupJoinParams {
        OpenSubgroupJoinParams {
            namespace_id: [0xAA; 32],
            subgroup_id: [0xBB; 32],
            joiner_public_key: PublicKey::from([0xCC; 32]),
        }
    }

    /// Answer one join request on `end` with `key_envelope_bytes`, mirroring what
    /// `handle_open_subgroup_join_request` puts on the wire.
    fn spawn_responder(
        mut end: Stream,
        key_envelope_bytes: Vec<u8>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let _req = crate::sync::stream::recv(&mut end, None, Duration::from_secs(5))
                .await
                .expect("responder: recv the join request");
            let reply = StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::OpenSubgroupJoinResponse { key_envelope_bytes },
                next_nonce: [0; 12],
            };
            crate::sync::stream::send(&mut end, &reply, None)
                .await
                .expect("responder: send the reply");
        })
    }

    /// **A round left incomplete by a transport failure is not "nobody has it".**
    ///
    /// The regression: with one key-less peer and one transport error — the exact
    /// tally from the CI failure that filed this — the old code bailed. The holder
    /// never answered, so the round cannot rule it out.
    #[tokio::test]
    async fn a_transport_failure_leaves_the_round_unanswered() {
        let mock = MockSyncNetwork::default();
        let keyless_end = mock.push_open_stream_ok_with_peer();
        let responder = spawn_responder(keyless_end, Vec::new()); // empty == key-less
        mock.push_open_stream_err("connection reset");

        let outcome = fetch_open_subgroup_key_once(
            &mock,
            &params(),
            None,
            &[peer(1), peer(2)],
            Duration::from_secs(5),
        )
        .await;

        responder.await.expect("responder task");
        assert!(
            matches!(outcome, KeyFetchRound::Unanswered { .. }),
            "a peer that never answered must leave the round retryable, not \
             terminal: {:?}",
            outcome.tally()
        );
        assert_eq!(
            outcome.tally(),
            "2 peer(s): 1 key-less, 1 transport error(s)",
            "the per-peer tally is what makes this diagnosable in a log"
        );
    }

    /// **Every peer answering key-less is terminal.**
    ///
    /// The other half, and the one that keeps the fix from costing latency: when
    /// nobody holds the key, re-asking the same peers cannot change the answer, so
    /// the join must fail on the first round.
    #[tokio::test]
    async fn everybody_answering_key_less_is_terminal() {
        let mock = MockSyncNetwork::default();
        let a = mock.push_open_stream_ok_with_peer();
        let b = mock.push_open_stream_ok_with_peer();
        let ra = spawn_responder(a, Vec::new());
        let rb = spawn_responder(b, Vec::new());

        let outcome = fetch_open_subgroup_key_once(
            &mock,
            &params(),
            None,
            &[peer(1), peer(2)],
            Duration::from_secs(5),
        )
        .await;

        ra.await.expect("responder a");
        rb.await.expect("responder b");
        assert!(
            matches!(outcome, KeyFetchRound::NobodyHasIt { .. }),
            "authoritative key-less answers must not be retried: {:?}",
            outcome.tally()
        );
    }

    /// **A holder later in the list is found despite an earlier transport error.**
    ///
    /// Within a single round: one dropped peer must not stop the walk, which is
    /// the behaviour the round loop then generalises across rounds.
    #[tokio::test]
    async fn a_key_holder_after_a_failed_peer_still_serves() {
        let mock = MockSyncNetwork::default();
        mock.push_open_stream_err("connection reset");
        let holder = mock.push_open_stream_ok_with_peer();
        let responder = spawn_responder(holder, b"the-key-envelope".to_vec());

        let outcome = fetch_open_subgroup_key_once(
            &mock,
            &params(),
            None,
            &[peer(1), peer(2)],
            Duration::from_secs(5),
        )
        .await;

        responder.await.expect("responder task");
        match outcome {
            KeyFetchRound::Key(bytes) => assert_eq!(bytes, b"the-key-envelope"),
            other => panic!("the second peer held the key: {:?}", other.tally()),
        }
    }

    /// **The regression: the sole key holder transport-fails, and the join still
    /// succeeds.**
    ///
    /// The observed CI shape — one legitimately key-less peer and the creator (the
    /// only holder) dropping its stream — which the old single pass reported as
    /// "no mesh peer held the subgroup key" and failed permanently. Re-running the
    /// identical commit passed, which is what identified it as transient.
    ///
    /// Round 2 is where the holder answers, so this fails if the retry is removed.
    #[tokio::test(start_paused = true)]
    async fn a_join_survives_a_transport_failure_to_the_only_key_holder() {
        let mock = MockSyncNetwork::default();

        // Round 1: peer 1 answers key-less, peer 2 (the holder) drops.
        let keyless_end = mock.push_open_stream_ok_with_peer();
        let keyless = spawn_responder(keyless_end, Vec::new());
        mock.push_open_stream_err("connection reset by peer");

        // Round 2: the same two peers, and this time the holder answers.
        let keyless_again = mock.push_open_stream_ok_with_peer();
        let keyless2 = spawn_responder(keyless_again, Vec::new());
        let holder_end = mock.push_open_stream_ok_with_peer();
        let holder = spawn_responder(holder_end, b"the-key-envelope".to_vec());

        let key = fetch_open_subgroup_key(
            &mock,
            &TopicHash::from_raw("ns/test"),
            &params(),
            None,
            vec![peer(1), peer(2)],
            Duration::from_secs(5),
            3,
            Duration::from_millis(300),
        )
        .await
        .expect("the holder answered on the second round");

        keyless.await.expect("keyless responder");
        keyless2.await.expect("keyless responder, round 2");
        holder.await.expect("holder responder");
        assert_eq!(key, b"the-key-envelope");
    }

    /// **Fail-fast is preserved: nobody holding the key costs exactly one round.**
    ///
    /// The cost of the fix has to be zero for the genuine "nobody has it" case, so
    /// this scripts only ONE round's worth of responses. A second round would draw
    /// from an exhausted queue — the mock errors on exhaust — and the assertion on
    /// the tally would see a transport error that the test never scripted.
    #[tokio::test(start_paused = true)]
    async fn nobody_holding_the_key_fails_without_a_second_round() {
        let mock = MockSyncNetwork::default();
        let a = mock.push_open_stream_ok_with_peer();
        let b = mock.push_open_stream_ok_with_peer();
        let ra = spawn_responder(a, Vec::new());
        let rb = spawn_responder(b, Vec::new());

        let err = fetch_open_subgroup_key(
            &mock,
            &TopicHash::from_raw("ns/test"),
            &params(),
            None,
            vec![peer(1), peer(2)],
            Duration::from_secs(5),
            3,
            Duration::from_millis(300),
        )
        .await
        .expect_err("nobody held the key");

        ra.await.expect("responder a");
        rb.await.expect("responder b");
        let msg = err.to_string();
        assert!(
            msg.contains("2 peer(s): 2 key-less, 0 transport error(s)"),
            "the per-peer tally must survive into the final error, and must show \
             the single round that actually ran: {msg}"
        );
    }
}

/// The credential check that decides whether a join request gets named at all.
///
/// Every case here is a rejection the responder must make BEFORE it wraps the
/// group key or serves backfill — the point of carrying a credential is that
/// the deny-list gate downstream has an account to read its rows under.
#[cfg(test)]
mod joiner_credential_tests {
    use calimero_account::{AccountGenesis, DeviceCert, DeviceId, KemPublicKey};
    use calimero_context_client::local_governance::JoinAccountCredential;
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use rand::rngs::OsRng;

    use super::SyncManager;

    /// An account root plus a credential certifying `sign_pk` under it.
    fn credential_for(sign_pk: &PublicKey) -> (JoinAccountCredential, AccountGenesis) {
        let root_sk = PrivateKey::random(&mut OsRng);
        let genesis = AccountGenesis::new(root_sk.public_key());
        let cert = DeviceCert::sign(
            &root_sk,
            genesis.account_id(),
            DeviceId::from([0xD1; 32]),
            sign_pk,
            &KemPublicKey::from([0x2B; 32]),
            0,
            0,
        )
        .expect("the account root signs its own device cert");
        (
            JoinAccountCredential {
                genesis,
                chain: vec![],
                statement: cert,
            },
            genesis,
        )
    }

    #[test]
    fn a_credential_certifying_the_requesting_key_names_its_account() {
        let joiner = PublicKey::from([0x11; 32]);
        let (credential, genesis) = credential_for(&joiner);

        let account =
            SyncManager::verified_joiner_account(&borsh::to_vec(&credential).unwrap(), &joiner)
                .expect("a well-formed credential for this key must resolve");

        assert_eq!(
            account,
            genesis.account_id(),
            "the resolved account must be the one the genesis derives, not one the \
             certificate merely claims"
        );
    }

    /// The replay guard, and the reason the check is not just `verify_device_cert`.
    ///
    /// A credential travels in the clear inside a join request, so any peer that
    /// has served one holds a copy. If the responder did not tie it to the key
    /// making THIS request, that copy would be a bearer token: replay it and be
    /// admitted as its owner — including past a deny-list entry, which is worse
    /// than the gap this whole change closes.
    #[test]
    fn a_credential_for_a_different_key_is_refused() {
        let owner = PublicKey::from([0x11; 32]);
        let (credential, _) = credential_for(&owner);

        let attacker = PublicKey::from([0x22; 32]);
        let err =
            SyncManager::verified_joiner_account(&borsh::to_vec(&credential).unwrap(), &attacker)
                .expect_err(
                    "a credential certifying someone else's key must not name this requester",
                );

        assert!(
            err.contains("different signing key"),
            "the refusal should say the certificate is for another key: {err}"
        );
    }

    /// The certificate names the account; the genesis is what PROVES it. A
    /// credential pairing one account's genesis with a certificate claiming
    /// another is how a requester would try to wear an account it cannot derive.
    #[test]
    fn a_genesis_that_does_not_derive_the_claimed_account_is_refused() {
        let joiner = PublicKey::from([0x11; 32]);
        let (mut credential, _) = credential_for(&joiner);
        let (other, _) = credential_for(&joiner);
        credential.genesis = other.genesis;

        let err =
            SyncManager::verified_joiner_account(&borsh::to_vec(&credential).unwrap(), &joiner)
                .expect_err("a genesis from another account must not certify this one");

        assert!(
            err.contains("genesis does not derive"),
            "the refusal should name the mismatch: {err}"
        );
    }

    #[test]
    fn undecodable_credential_bytes_are_refused_not_ignored() {
        let joiner = PublicKey::from([0x11; 32]);
        let err = SyncManager::verified_joiner_account(b"not a credential", &joiner)
            .expect_err("garbage must refuse rather than fall through to an unnamed admit");
        assert!(err.contains("undecodable"), "{err}");
    }

    /// An empty credential is what an older initiator effectively sends. It must
    /// refuse, not admit: "could not name them" was precisely the condition that
    /// used to skip the deny-list check.
    #[test]
    fn an_absent_credential_is_refused() {
        let joiner = PublicKey::from([0x11; 32]);
        assert!(SyncManager::verified_joiner_account(&[], &joiner).is_err());
    }
}

#[cfg(test)]
mod namespace_pull_peer_tests {
    //! Peer selection for a namespace pull, and specifically the case the
    //! selection exists for: gossipsub's subscriber table can be EMPTY while a
    //! peer that holds the namespace is reachable and talking to us.
    //!
    //! Driven against a scripted [`MockSyncNetwork`] rather than a booted node,
    //! the same shape the open-subgroup key round uses.

    use libp2p::gossipsub::TopicHash;
    use libp2p::PeerId;

    use super::{discover_namespace_pull_peer, NamespacePullPeer};
    use crate::sync::network::mock::MockSyncNetwork;

    const NS: [u8; 32] = [0x5c; 32];

    fn ns_topic() -> TopicHash {
        TopicHash::from_raw(format!("ns/{}", hex::encode(NS)))
    }

    #[tokio::test]
    async fn a_subscriber_is_preferred_over_the_fallback() {
        let subscriber = PeerId::random();
        let fallback = PeerId::random();
        let mock = MockSyncNetwork::default();
        let _ = mock.push_subscribed_peers_for(ns_topic(), vec![subscriber]);

        assert_eq!(
            discover_namespace_pull_peer(&mock, NS, Some(fallback)).await,
            NamespacePullPeer::Subscriber(subscriber),
            "a working subscriber table must decide it; the fallback is a last \
             resort, not a preference",
        );
    }

    /// The regression. Before the fallback existed this returned "nobody" and the
    /// pull did not happen — while the peer that could serve it was beaconing at
    /// us every few seconds.
    #[tokio::test]
    async fn an_empty_subscriber_table_still_reaches_the_fallback() {
        let fallback = PeerId::random();
        let mock = MockSyncNetwork::default();
        let _ = mock.push_subscribed_peers_for(ns_topic(), vec![]);

        assert_eq!(
            discover_namespace_pull_peer(&mock, NS, Some(fallback)).await,
            NamespacePullPeer::Fallback(fallback),
            "an empty table means the table is wrong, not that the node is alone",
        );
    }

    #[tokio::test]
    async fn without_a_fallback_an_empty_table_is_still_nobody() {
        let mock = MockSyncNetwork::default();
        let _ = mock.push_subscribed_peers_for(ns_topic(), vec![]);

        assert_eq!(
            discover_namespace_pull_peer(&mock, NS, None).await,
            NamespacePullPeer::Nobody,
            "callers that supply no fallback keep the old behaviour exactly — \
             this must not start guessing at a peer on its own",
        );
    }
}

#[cfg(test)]
mod join_target_tests {
    use std::sync::Arc;

    use calimero_context_config::types::{
        ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };
    use calimero_governance_store::NamespaceRepository;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use rand::rngs::OsRng;
    use sha2::{Digest, Sha256};

    use super::join_target_group;

    fn invitation_to(admin_sk: &PrivateKey, group_id: ContextGroupId) -> SignedGroupOpenInvitation {
        let invitation = GroupInvitationFromAdmin {
            inviter_identity: SignerId::from(*admin_sk.public_key().digest()),
            group_id,
            expiration_timestamp: 0,
            invitation_nonce: [0x11; 32],
            invited_role: 1,
            admitters: Vec::new(),
        };
        let signature = admin_sk
            .sign(&Sha256::digest(borsh::to_vec(&invitation).unwrap()))
            .unwrap();
        SignedGroupOpenInvitation {
            inviter_account: None,
            invitation,
            inviter_signature: hex::encode(signature.to_bytes()),
            application_id: None,
            bytecode_id: None,
            admitter_addrs: Vec::new(),
        }
    }

    #[test]
    fn a_subgroup_invitation_resolves_to_the_subgroup() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let namespace = ContextGroupId::from([0x01; 32]);
        let subgroup = ContextGroupId::from([0x02; 32]);
        NamespaceRepository::new(&store)
            .nest(&namespace, &subgroup)
            .expect("nest the subgroup");
        let admin_sk = PrivateKey::random(&mut OsRng);

        // The request names the namespace; only the invitation knows it is for
        // the subgroup, and serving the namespace instead is the whole bug.
        assert_eq!(
            join_target_group(&store, namespace, &invitation_to(&admin_sk, subgroup)).unwrap(),
            subgroup
        );
    }

    #[test]
    fn a_namespace_invitation_resolves_to_the_namespace() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let namespace = ContextGroupId::from([0x01; 32]);
        let admin_sk = PrivateKey::random(&mut OsRng);

        assert_eq!(
            join_target_group(&store, namespace, &invitation_to(&admin_sk, namespace)).unwrap(),
            namespace
        );
    }

    #[test]
    fn an_invitation_from_another_namespace_is_refused() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let namespace = ContextGroupId::from([0x01; 32]);
        let elsewhere = ContextGroupId::from([0x03; 32]);
        let stranger = ContextGroupId::from([0x04; 32]);
        NamespaceRepository::new(&store)
            .nest(&elsewhere, &stranger)
            .expect("nest the foreign subgroup");
        let admin_sk = PrivateKey::random(&mut OsRng);

        // Pairing a valid invitation with someone else's namespace must not
        // release that namespace's key material.
        assert!(join_target_group(&store, namespace, &invitation_to(&admin_sk, stranger)).is_err());
    }
}
