use crate::{
    CapabilitiesRepository, DenyListRepository, GroupKeyring, MembershipRepository, MetaRepository,
    NamespaceRepository,
};
use calimero_context_client::local_governance::{
    hash_scoped_namespace, AckRouter, EncryptedGroupOp, GroupOp, KeyEnvelope, NamespaceOp, RootOp,
    SignedGroupOp, SignedNamespaceOp,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::application::ZERO_APPLICATION_ID;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::Result as EyreResult;
use libp2p::gossipsub::TopicHash;

use crate::governance_broadcast::{
    self, assert_transport_ready, classify_publish_readiness, ns_topic,
    publish_and_await_ack_namespace, timeout_for_namespace_op, DeliveryReport, PublishReadiness,
};
use crate::metrics::{record_governance_publish_mesh_peers, record_namespace_retry_event};
use crate::op_events::{notify as notify_op_event, OpEvent};

use super::super::{
    apply_group_op_mutations, load_nonce_window, restore_member_context_identities,
    store_nonce_window,
};
use super::dag::{NamespaceDagService, NamespaceHead};
use super::op_log::NamespaceOpLogService;
use super::retry::NamespaceRetryService;

/// A key rotation unwrap failure that the caller should handle.
#[derive(Debug)]
pub struct KeyUnwrapFailure {
    pub group_id: [u8; 32],
    pub reason: String,
}

/// Result of applying a namespace governance op.
#[derive(Debug, Default)]
pub struct ApplyNamespaceOpResult {
    pub key_unwrap_failures: Vec<KeyUnwrapFailure>,
    /// Post-apply hash divergence detected by the cross-DAG state-hash
    /// check inside a `MemberRemoved` / `MemberLeft` apply. The node
    /// handler routes this to the reconcile-via-anchor sync trigger.
    /// `None` for ops that don't carry signed convergence claims, for
    /// ops that match the receiver's view, and for ops that don't go
    /// through the verify path at all.
    pub divergence: Option<super::super::DivergenceReport>,
}

pub(crate) fn min_acks_after_local_mutation(
    _known_at_gate: usize,
    known_at_publish: usize,
) -> usize {
    if known_at_publish == 0 {
        0
    } else {
        governance_broadcast::DEFAULT_MIN_ACKS
    }
}

/// Classify a best-effort governance publish from the role of its ack
/// signers. Shared by `NamespaceGovernance::sign_apply_and_publish` and
/// `GroupGovernancePublisher` (a sibling module — hence `pub(crate)`).
pub(crate) fn classify_report_readiness(
    store: &Store,
    namespace_id: [u8; 32],
    report: &DeliveryReport,
    known_subscribers: usize,
) -> PublishReadiness {
    let authoritative_ack = report.acked_by.iter().any(|pk| {
        MembershipRepository::new(store)
            .is_authoritative_namespace_identity(namespace_id, pk)
            .unwrap_or(false)
    });
    classify_publish_readiness(authoritative_ack, report.acked_by.len(), known_subscribers)
}

/// Domain API for namespace DAG and governance operation lifecycle.
pub struct NamespaceGovernance<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
    /// The op's causal cut (its parent op hashes), threaded to the apply gates so
    /// they can authorize against the projection AS OF the op's parents. Empty for
    /// constructions outside the live apply path (sign/read/tests), which keep the
    /// live resolver via the default authorizer below.
    parents: &'a [[u8; 32]],
    /// The at-cut apply-auth decision source (F5 #28). Defaults to
    /// [`LiveFallbackAuthorizer`] (always `None` → live), overridden on the live
    /// apply path by [`with_apply_auth`](Self::with_apply_auth) with a projection-
    /// backed authorizer.
    authorizer: &'a dyn crate::authorizer::AtCutAuthorizer,
}

impl<'a> NamespaceGovernance<'a> {
    pub fn new(store: &'a Store, namespace_id: [u8; 32]) -> Self {
        Self {
            store,
            namespace_id,
            parents: &[],
            authorizer: &crate::authorizer::LIVE_FALLBACK_AUTHORIZER,
        }
    }

    /// Attach the op's causal cut + the at-cut apply-auth source for the live apply
    /// path (F5 #28). Without this the gates use the live resolver (the default
    /// authorizer returns `None`); with it they consult the projection at `parents`
    /// and fall back to live only when the cited ancestry isn't fully folded.
    #[must_use]
    pub fn with_apply_auth(
        mut self,
        parents: &'a [[u8; 32]],
        authorizer: &'a dyn crate::authorizer::AtCutAuthorizer,
    ) -> Self {
        self.parents = parents;
        self.authorizer = authorizer;
        self
    }

    /// Returns current DAG head as parent hashes + next nonce.
    pub fn read_head_record(&self) -> EyreResult<NamespaceHead> {
        NamespaceDagService::new(self.store, self.namespace_id).read_head_record()
    }

    /// Backward-compatible tuple facade for existing call sites.
    pub fn read_head(&self) -> EyreResult<(Vec<[u8; 32]>, u64)> {
        Ok(self.read_head_record()?.into_tuple())
    }

    pub fn advance_dag_head(
        &self,
        delta_id: [u8; 32],
        parent_ids: &[[u8; 32]],
        sequence: u64,
    ) -> EyreResult<()> {
        NamespaceDagService::new(self.store, self.namespace_id)
            .advance_dag_head(delta_id, parent_ids, sequence)
    }

    /// Persist a namespace governance op in the local DAG log.
    pub fn store_operation(&self, op: &SignedNamespaceOp) -> EyreResult<()> {
        NamespaceDagService::new(self.store, self.namespace_id).store_operation(op)
    }

    pub fn collect_skeleton_delta_ids_for_group(
        &self,
        group_id: [u8; 32],
    ) -> EyreResult<Vec<[u8; 32]>> {
        NamespaceDagService::new(self.store, self.namespace_id)
            .collect_skeleton_delta_ids_for_group(group_id)
    }

    pub fn apply_signed_op(&self, op: &SignedNamespaceOp) -> EyreResult<ApplyNamespaceOpResult> {
        op.verify_signature()
            .map_err(|e| eyre::eyre!("signed namespace op: {e}"))?;

        let delta_id = op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;

        // Idempotency guard. If this exact op is already in our local op-log
        // it has already been applied — `advance_dag_head` ran for it (see the
        // store/advance ordering note below) and any side effects fired. A
        // re-receive (typically a node's *own* published op coming back via
        // sync backfill — the in-memory `DagStore` dedup set never saw the
        // publisher path, so it can't filter it) must be a no-op: re-running
        // `advance_dag_head` here would append `delta_id` to the head set a
        // second time, and a head set with duplicates makes
        // `compute_governance_position` refuse to embed a position, so every
        // peer then rejects all of this node's state deltas (#2327).
        //
        // The guard suppresses *all* of the apply work below — the per-op-kind
        // side effects in the match arms included. Re-running them would be
        // redundant (they're written replay-safe); a maintainer adding a new
        // side effect should keep it replay-safe rather than rely on this
        // guard never firing. The encrypted-op *retry* path is unaffected: it
        // re-applies
        // via `decrypt_and_apply_group_op` → `apply_group_op_inner`, not
        // `apply_signed_op`, and is bounded by the per-signer nonce check there.
        if NamespaceOpLogService::new(self.store, self.namespace_id).contains_op(delta_id)? {
            tracing::debug!(
                namespace_id = %hex::encode(self.namespace_id),
                delta_id = %hex::encode(delta_id),
                "namespace governance op already applied; skipping re-apply (#2327)"
            );
            // #2770: this early-return is BEFORE the RootOp mutations, so a
            // replay re-collects no events and the post-`store_operation` flush
            // below never runs for an already-logged op — same no-re-emit-on-
            // replay behaviour (and same accepted crash-window gap) as the
            // canonical dedup-tradeoff note in `apply_local_signed_group_op`.
            return Ok(ApplyNamespaceOpResult::default());
        }

        let mut result = ApplyNamespaceOpResult::default();
        let mut root_events: Vec<crate::op_events::OpEvent> = Vec::new();

        match &op.op {
            NamespaceOp::Root(root) => {
                root_events = self.apply_root_op(op, root)?;

                match root {
                    RootOp::KeyDelivery {
                        group_id,
                        ref envelope,
                    } => {
                        // Admin-initiated delivery (add_group_members /
                        // admit_tee_node) of a group key to a member that
                        // can't yet decrypt the group. Reuse the joiner-side
                        // apply: unwrap for our identity, store, seed the
                        // bootstrap scaffolding (placeholder meta + own member
                        // row + default caps — NOT the founding admin, which
                        // comes from the NamespaceCreated genesis since #2474),
                        // and replay buffered ops. Best-effort:
                        // a failure here must not block the DAG (every later
                        // op would orphan), so errors are logged, not
                        // propagated.
                        let envelope_bytes = borsh::to_vec(envelope).unwrap_or_default();
                        match self.apply_received_group_key(*group_id, &envelope_bytes, op.signer) {
                            Ok(retry_divergence) => {
                                if retry_divergence.is_some() {
                                    result.divergence = retry_divergence;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    group_id = %hex::encode(group_id),
                                    error = %e,
                                    "KeyDelivery side-effect failed; DAG apply continues"
                                );
                                result.key_unwrap_failures.push(KeyUnwrapFailure {
                                    group_id: *group_id,
                                    reason: format!("KeyDelivery side-effect failed: {e}"),
                                });
                            }
                        }
                    }
                    RootOp::MemberJoined { .. } => {
                        // The joiner obtains the group key via the direct
                        // join response and the joiner-side pull
                        // (`recover_missing_group_keys`); no delivery is
                        // triggered from this apply path.
                    }
                    RootOp::MemberJoinedOpen { member, group_id } => {
                        // The joiner pulls the subgroup key directly from a
                        // sync peer; no delivery is triggered here. Authority
                        // for this op is validated in `execute_member_joined_open`
                        // (we ran it via `apply_root_op` above before this
                        // match), so by the time we get here the path is
                        // confirmed Inherited.
                        let group_id_typed = ContextGroupId::from(*group_id);
                        // Clear deny-list on EVERY peer, not just the
                        // local rejoiner. A prior `MemberLeft` (or
                        // `MemberRemoved` followed by inheritance rejoin)
                        // stamped node-2 on each peer's per-subgroup
                        // deny-list at `mod.rs:1248` / `:1627`; without
                        // clearing it here, peers continue to drop
                        // node-2's state-delta traffic at the receive
                        // filter even after the rejoin completes. The
                        // sibling `MemberAdded` arm at `mod.rs:1215`
                        // already does this; `MemberJoinedViaTeeAttestation`
                        // at `mod.rs:1502` does this; `MemberJoinedOpen`
                        // was the missing third arm. Without it the
                        // `kick → inheritance-rejoin → write` and
                        // `leave → inheritance-rejoin → write` flows
                        // converge on the rejoiner's local store but
                        // never replicate to peers — symptom: post-rejoin
                        // sync diverges in the kick/leave-rejoin e2e.
                        // Idempotent on a member who was never denied.
                        DenyListRepository::new(self.store).clear(&group_id_typed, member)?;
                        // Local rejoiner recovery: restore any per-context
                        // `ContextIdentity` rows that a prior `MemberLeft`
                        // cascade deleted. The local-rejoiner anti-spoof
                        // gate is enforced inside
                        // `restore_member_context_identities` — on peers
                        // whose namespace identity differs from `member`
                        // it is a no-op. On first-time inheritance joiners
                        // the row may not exist yet — it is written so the
                        // joiner can author state-DAG ops as soon as
                        // `KeyDelivery` populates `sender_key`. Idempotent:
                        // an existing row from a prior `join_context` is
                        // left untouched.
                        restore_member_context_identities(self.store, &group_id_typed, member)?;
                    }
                    RootOp::GroupCreated { group_id, .. } => {
                        // #2848: GroupCreated just wrote this subgroup's meta +
                        // admin row (via `apply_root_op` above), so an
                        // encrypted ContextRegistered that was buffered before
                        // it landed — and previously bailed at the staleness
                        // check because the meta did not exist — can now apply.
                        // Re-drive, but only when the node plausibly holds a key
                        // that could decrypt this group's buffered ops. This is
                        // a CHEAP keyring presence check — a single
                        // first-key-exists lookup per keyring
                        // (`holds_any_key`) — NOT an op-log scan: gating with a
                        // full op-log scan would defeat the gate's purpose,
                        // since the retry it guards
                        // (`collect_retry_candidates_for_group`) already does a
                        // full scan, so a scanning gate saves nothing on the
                        // GroupCreated hot path. The check mirrors the
                        // dual-keyring resolution the apply/retry path uses
                        // (#2256): the subgroup's own keyring (Restricted) OR
                        // the namespace keyring (Open subgroups encrypt under
                        // it). Gating on the subgroup keyring alone was wrong —
                        // a node holding only the namespace key still has
                        // decryptable Open buffered ops and must be re-driven.
                        //
                        // W3/S1 fix: gate on whether the keyring holds ANY key,
                        // not just the *current* one. The retry resolves each
                        // buffered op by its `key_id` (`load_key_by_id`), so
                        // after a key ROTATION a node may hold only the OLD key
                        // that a buffered op was encrypted under while
                        // `load_current_key` returns the newer key (or, if the
                        // node never received the new delivery, the old key IS
                        // still the entry but distinct from the op's key_id).
                        // The old current-key gate produced a false-negative in
                        // exactly that case — the op was decryptable yet the
                        // re-drive was skipped. "Holds any key" has no such
                        // false-negative (if the matching key_id is held, the
                        // keyring is non-empty); the residual false-positive
                        // (non-empty keyring without the specific key_id) just
                        // costs one bounded, self-gating retry scan that finds
                        // no candidates — harmless. When neither keyring holds
                        // any key (the common case and the deleted-group exit
                        // since purge clears keys) the retry is skipped without
                        // touching the op-log. Best-effort: log, never
                        // propagate.
                        let gid = *group_id;
                        let gid_typed = ContextGroupId::from(gid);
                        let ns_typed = ContextGroupId::from(self.namespace_id);
                        let holds_key = GroupKeyring::new(self.store, gid_typed)
                            .holds_any_key()
                            .unwrap_or(false)
                            || GroupKeyring::new(self.store, ns_typed)
                                .holds_any_key()
                                .unwrap_or(false);
                        if holds_key {
                            match self.retry_encrypted_ops_for_group(*group_id) {
                                Ok(retry_divergence) => {
                                    if retry_divergence.is_some() {
                                        result.divergence = retry_divergence;
                                    }
                                }
                                Err(e) => tracing::warn!(
                                    ?e,
                                    group_id = %hex::encode(group_id),
                                    "retry after GroupCreated failed (#2848)"
                                ),
                            }
                        }
                    }
                    _ => {}
                }
            }
            NamespaceOp::Group {
                group_id,
                key_id,
                encrypted,
                key_rotation,
            } => {
                let group_id_typed = ContextGroupId::from(*group_id);

                // Issue #2256: an `Open` subgroup is encrypted with the
                // parent namespace's key (see `GroupGovernancePublisher`).
                // The receiver doesn't need to know whether the op is
                // Open- or Restricted-encrypted at decode time — it
                // tries the subgroup's keyring first (Restricted case),
                // then falls back to the namespace's keyring (Open case).
                // This also handles a flip race cleanly: if the publisher
                // saw `Open` but the receiver has already applied a flip
                // to `Restricted`, the fallback still resolves the key
                // because both keyrings persist their entries.
                let resolved_key =
                    match GroupKeyring::new(self.store, group_id_typed).load_key_by_id(key_id)? {
                        Some(k) => Some(k),
                        None => {
                            let ns_id_typed = ContextGroupId::from(self.namespace_id);
                            GroupKeyring::new(self.store, ns_id_typed).load_key_by_id(key_id)?
                        }
                    };

                if let Some(group_key) = resolved_key {
                    // Surface any post-apply hash divergence reported by
                    // `MemberRemoved` / `MemberLeft` apply so the node
                    // handler can route it to the reconcile-via-anchor
                    // trigger. Multiple group ops can be applied per
                    // namespace op in theory (e.g. retry path replays);
                    // in practice each namespace op carries one
                    // encrypted group op, so the assignment is a simple
                    // overwrite. Any prior `None` is preserved if this
                    // op reports `None`.
                    let report = self.decrypt_and_apply_group_op(
                        op,
                        &group_id_typed,
                        &group_key,
                        encrypted,
                    )?;
                    if report.is_some() {
                        result.divergence = report;
                    }
                }

                if let Some(rotation) = key_rotation {
                    let ns_id = ContextGroupId::from(op.namespace_id);
                    if let Some(identity) =
                        NamespaceRepository::new(self.store).identity_record(&ns_id)?
                    {
                        let recipient_sk = PrivateKey::from(identity.private_key);
                        for envelope in &rotation.envelopes {
                            if envelope.recipient == recipient_sk.public_key() {
                                match GroupKeyring::unwrap_for_recipient(&recipient_sk, envelope) {
                                    Ok(new_key) => {
                                        let _ = GroupKeyring::new(self.store, group_id_typed)
                                            .store_key(&new_key)?;
                                        tracing::info!(
                                            group_id = %hex::encode(group_id),
                                            "stored rotated group key"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            ?e,
                                            "failed to unwrap key rotation envelope"
                                        );
                                        result.key_unwrap_failures.push(KeyUnwrapFailure {
                                            group_id: *group_id,
                                            reason: format!("key rotation unwrap failed: {e}"),
                                        });
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Ordering: advance the head first, then write the op to the log —
        // `publish_post_gate` uses the same order. The store gives us no
        // transaction across the two keys, so this ordering is the one that
        // keeps the idempotency guard's invariant ("op in the log ⟹ its head
        // advance already ran"): a crash between the two writes leaves the op
        // *not* in the log, so a retry sees `contains_op == false` and
        // re-applies — re-running the (replay-safe) side effects above and
        // calling `advance_dag_head` again, where the dedup makes the second
        // add a no-op. The reverse order would leave the op logged but the
        // head un-advanced, and a later re-receive would hit the guard, skip
        // the apply, and never advance the head for it. A truly atomic update
        // would need a single-batch write spanning both keys.
        let head = self.read_head_record()?;
        self.advance_dag_head(delta_id, &op.parent_op_hashes, head.next_nonce)?;
        self.store_operation(op)?;

        // #2770: flush RootOp-path events only after the namespace op is appended.
        for event in root_events {
            crate::op_events::notify(event);
        }

        Ok(result)
    }

    pub async fn sign_apply_and_publish(
        &self,
        node_client: &calimero_node_primitives::client::NodeClient,
        ack_router: &AckRouter,
        signer_sk: &PrivateKey,
        op: NamespaceOp,
    ) -> EyreResult<DeliveryReport> {
        let topic = ns_topic(self.namespace_id);

        // Apply-FIRST: governance ops are locally authoritative. The local
        // DAG mutation must not be gated on transport state — a node alone
        // or with an unformed mesh still applies its own op and relies on
        // sync to carry it to peers. (Previously a `NamespaceNotReady` gate
        // ran here first and dropped the op entirely.) The publish below is
        // best-effort: a transport failure downgrades the readiness
        // classification, it never fails the call. See the
        // best-effort-readiness design doc. The orphan-op-on-retry concern
        // the old gate guarded against no longer applies: there is no
        // rejection and no caller retry, so the op applies exactly once.
        let head = self.read_head_record()?;
        // Group ops are observed in `GroupGovernancePublisher` with the
        // cleartext `GroupOp` label; observing them here too would double-count.
        let observe_mesh = !matches!(op, NamespaceOp::Group { .. });
        let op_kind = op.op_kind_label();
        let op_timeout = timeout_for_namespace_op(&op);
        let signed = SignedNamespaceOp::sign(
            signer_sk,
            self.namespace_id,
            head.parent_hashes,
            head.next_nonce,
            op,
        )?;
        // Topic-scoped hash — the same identifier
        // `publish_and_await_ack_namespace` records in a *successful*
        // `DeliveryReport`. Computing it the same way here keeps the
        // `op_hash` on the synthesized best-effort report (below)
        // consistent with the success path, so log correlation works
        // regardless of whether the publish confirmed.
        let op_hash = hash_scoped_namespace(topic.as_str().as_bytes(), &signed)
            .map_err(|e| eyre::eyre!("hash_scoped_namespace: {e}"))?;

        self.apply_signed_op(&signed)?;

        // Notify the readiness FSM that we just advanced the local DAG on
        // the publisher path. Without this, `state_per_namespace` only
        // populates from gossipsub-receive deliveries — a node that
        // *publishes* an op never observes its own monotonic advance and
        // the FSM stays at `Bootstrapping` forever. See
        // `notify_namespace_op_applied` for the cross-crate plumbing.
        node_client.notify_namespace_op_applied(self.namespace_id);

        let mesh = node_client
            .mesh_peer_count_for_namespace(self.namespace_id)
            .await;
        let known = node_client.known_subscribers(&topic);
        if observe_mesh {
            record_governance_publish_mesh_peers(op_kind, mesh);
        }
        let min_acks = min_acks_after_local_mutation(known, known);

        // Best-effort publish: the op is already committed locally, so a
        // `NoAckReceived` / `Publish` failure is NOT fatal — synthesize an
        // empty report and let the readiness classification record it.
        let mut report = match publish_and_await_ack_namespace(
            self.store,
            node_client.network_client(),
            ack_router,
            self.namespace_id,
            topic,
            signed,
            op_timeout,
            min_acks,
            None,
        )
        .await
        {
            Ok(report) => report,
            Err(e) => {
                tracing::debug!(
                    op_kind,
                    namespace_id = %hex::encode(self.namespace_id),
                    error = %e,
                    "namespace governance op applied locally; publish did not \
                     confirm (best-effort) — will propagate via sync"
                );
                DeliveryReport {
                    op_hash,
                    acked_by: Vec::new(),
                    elapsed_ms: 0,
                    readiness: PublishReadiness::Degraded,
                }
            }
        };

        report.readiness = classify_report_readiness(self.store, self.namespace_id, &report, known);
        tracing::debug!(
            op_kind,
            namespace_id = %hex::encode(self.namespace_id),
            acks = report.acked_by.len(),
            readiness = report.readiness.label(),
            elapsed_ms = report.elapsed_ms,
            op_hash = %hex::encode(report.op_hash),
            "namespace governance op published"
        );
        Ok(report)
    }

    pub async fn sign_and_publish_without_apply(
        &self,
        node_client: &calimero_node_primitives::client::NodeClient,
        ack_router: &AckRouter,
        signer_sk: &PrivateKey,
        op: NamespaceOp,
        required_signers: Option<Vec<PublicKey>>,
    ) -> EyreResult<DeliveryReport> {
        let topic = ns_topic(self.namespace_id);
        let mesh = node_client
            .mesh_peer_count_for_namespace(self.namespace_id)
            .await;
        let known = node_client.known_subscribers(&topic);
        assert_transport_ready(mesh, known, node_client.gossipsub_mesh_n_low())
            .map_err(|e| eyre::eyre!(e))?;

        // Quorum / no-local-apply path: a publish that never reaches a
        // quorum is a genuine failure — nothing was applied locally to
        // fall back on. `best_effort = false` keeps the hard error.
        self.publish_post_gate(
            node_client,
            ack_router,
            signer_sk,
            op,
            topic,
            mesh,
            known,
            required_signers,
            false,
        )
        .await
    }

    /// Post-gate variant of [`sign_and_publish_without_apply`]: takes the
    /// `mesh` / `known_subscribers` snapshot the caller already observed,
    /// so this never re-samples or re-runs `assert_transport_ready`.
    /// Used by [`GroupGovernancePublisher::sign_apply_and_publish_inner`]
    /// after the local group store has already been mutated. That caller
    /// passes `best_effort = true`: it has no gate of its own (the local
    /// apply is unconditional), so a publish failure here is non-fatal and
    /// the op propagates via sync rather than diverging on a retry.
    // Gossip-publish entry on the namespace governance path: transport handles,
    // the op, and ack/gate sizing are orthogonal with no cohesive grouping.
    #[allow(clippy::too_many_arguments, reason = "orthogonal broadcast-path args")]
    pub(crate) async fn sign_and_publish_post_gate(
        &self,
        node_client: &calimero_node_primitives::client::NodeClient,
        ack_router: &AckRouter,
        signer_sk: &PrivateKey,
        op: NamespaceOp,
        mesh: usize,
        known: usize,
        best_effort: bool,
    ) -> EyreResult<DeliveryReport> {
        let topic = ns_topic(self.namespace_id);
        self.publish_post_gate(
            node_client,
            ack_router,
            signer_sk,
            op,
            topic,
            mesh,
            known,
            None,
            best_effort,
        )
        .await
    }

    /// Shared body of [`sign_and_publish_without_apply`] and
    /// [`sign_and_publish_post_gate`]. Takes the caller's `mesh` snapshot
    /// to feed the metric; the subscriber count is re-sampled at publish
    /// time (see below) so transient peer departures don't skew `min_acks`.
    ///
    /// `best_effort` selects the failure mode of the publish:
    /// * `false` (quorum / no-local-apply path) — a publish that gathers
    ///   no acks is a genuine `Err`; nothing was applied locally.
    /// * `true` (group-op apply-and-publish path) — the local mutation is
    ///   already committed, so a publish failure is swallowed into a
    ///   `Degraded` [`DeliveryReport`] and propagation falls to sync.
    // Gossip-publish entry on the namespace governance path: transport handles,
    // the op, and ack/gate sizing are orthogonal with no cohesive grouping.
    #[allow(clippy::too_many_arguments, reason = "orthogonal broadcast-path args")]
    async fn publish_post_gate(
        &self,
        node_client: &calimero_node_primitives::client::NodeClient,
        ack_router: &AckRouter,
        signer_sk: &PrivateKey,
        op: NamespaceOp,
        topic: TopicHash,
        mesh: usize,
        known_at_gate: usize,
        required_signers: Option<Vec<PublicKey>>,
        best_effort: bool,
    ) -> EyreResult<DeliveryReport> {
        let head = self.read_head_record()?;
        let observe_mesh = !matches!(op, NamespaceOp::Group { .. });
        let op_kind = op.op_kind_label();
        let op_timeout = timeout_for_namespace_op(&op);
        let signed = SignedNamespaceOp::sign(
            signer_sk,
            self.namespace_id,
            head.parent_hashes,
            head.next_nonce,
            op,
        )?;
        let delta_id = signed
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        // Topic-scoped hash for the `DeliveryReport` — matches the
        // `op_hash` that `publish_and_await_ack_namespace` records on a
        // successful report (`delta_id` above is the DAG content hash,
        // a different value). Used by the best-effort error arm below.
        let scoped_op_hash = hash_scoped_namespace(topic.as_str().as_bytes(), &signed)
            .map_err(|e| eyre::eyre!("hash_scoped_namespace: {e}"))?;
        let parent_ids = signed.parent_op_hashes.clone();

        // Advance the head first, then write the op to the log — same order as
        // `apply_signed_op`. This keeps the invariant the idempotency guard in
        // `apply_signed_op` relies on: "op in the local op-log ⟹ its head
        // advance already ran". If these were reversed, a crash in between
        // would leave the op logged but the head un-advanced, and a later
        // re-receive of that op would hit the guard, skip the apply, and never
        // advance the head for it — orphaning it from the DAG head lineage.
        self.advance_dag_head(delta_id, &parent_ids, head.next_nonce)?;
        self.store_operation(&signed)?;

        // Same signal as in `sign_apply_and_publish` above — the local DAG
        // just advanced on the publisher path, so the readiness FSM needs
        // to be told. Both paths converge at `Handler<NamespaceOpApplied>`
        // on `ReadinessManager`, mirroring the gossipsub-receive route.
        node_client.notify_namespace_op_applied(self.namespace_id);

        if observe_mesh {
            record_governance_publish_mesh_peers(op_kind, mesh);
        }

        // Refresh `known` here, AFTER all the local-mutation /
        // encryption / key-rotation / store_operation work above and
        // immediately before deciding `min_acks`. The caller's snapshot
        // (`known_at_gate`) was taken many awaits ago; in group-publish
        // flows it predates `sign_apply_local_group_op_borsh` and the
        // per-removal key mint. If a peer unsubscribed in the meantime,
        // sticking with the stale count would leave `min_acks = 1`
        // against an empty subscriber set and force `NoAckReceived`
        // after the local DAG has already advanced. For `best_effort`
        // callers that `NoAckReceived` is swallowed into a `Degraded`
        // report; for quorum callers it is the genuine failure they
        // expect. `known_subscribers` is a cheap synchronous DashMap
        // lookup on `NodeClient` (no actor mailbox round-trip), so
        // re-sampling here costs effectively nothing while making the
        // solo-namespace fast-path responsive to live state.
        let known_at_publish = node_client.known_subscribers(&topic);
        let min_acks = min_acks_after_local_mutation(known_at_gate, known_at_publish);

        let report = match publish_and_await_ack_namespace(
            self.store,
            node_client.network_client(),
            ack_router,
            self.namespace_id,
            topic,
            signed,
            op_timeout,
            min_acks,
            required_signers,
        )
        .await
        {
            Ok(report) => report,
            // Best-effort callers (the group-op apply-and-publish path) have
            // already committed the local mutation; a publish failure is
            // non-fatal and propagation falls to sync. Quorum callers pass
            // `best_effort = false` and still get the hard error.
            Err(e) if best_effort => {
                tracing::debug!(
                    op_kind,
                    namespace_id = %hex::encode(self.namespace_id),
                    error = %e,
                    "namespace governance op applied locally; publish did not \
                     confirm (best-effort) — will propagate via sync"
                );
                DeliveryReport {
                    op_hash: scoped_op_hash,
                    acked_by: Vec::new(),
                    elapsed_ms: 0,
                    readiness: PublishReadiness::Degraded,
                }
            }
            Err(e) => return Err(eyre::eyre!(e)),
        };
        // `best_effort` distinguishes the two callers: the quorum path
        // (`sign_and_publish_without_apply`, `best_effort = false`) does no
        // local apply, while the group-op path (`sign_and_publish_post_gate`,
        // `best_effort = true`) reaches here *after* `GroupGovernancePublisher`
        // already committed the local mutation. Logging it as a field keeps
        // the message accurate for both.
        tracing::debug!(
            op_kind,
            namespace_id = %hex::encode(self.namespace_id),
            acks = report.acked_by.len(),
            elapsed_ms = report.elapsed_ms,
            op_hash = %hex::encode(report.op_hash),
            best_effort,
            "namespace governance op published"
        );
        Ok(report)
    }

    /// Seed the namespace root's bootstrap rows (placeholder meta, the
    /// deliverer's own member row, and default caps) when no local state exists.
    ///
    /// This closes the no-invitation bootstrap gap for TEE fleet-join: a node
    /// that replays the namespace DAG needs a root meta row + its own membership
    /// so the encrypted-op replay path can read state and pass its membership
    /// checks. The invited path recovers this from the invitation; the TEE path
    /// has none, so this seed provides the minimal scaffolding on the first
    /// `KeyDelivery`.
    ///
    /// #2474 — this seed NO LONGER establishes the founding admin/owner. It
    /// previously TOFU-trusted the KeyDelivery signer as the namespace admin,
    /// but the signer need only HOLD the group key (any current member), so when
    /// a non-owner delivered the key the replica pinned the WRONG admin and
    /// permanently rejected the true owner's ops, wedging backfill (production-
    /// confirmed). The authoritative founder now comes from the replayable
    /// `RootOp::NamespaceCreated` genesis op
    /// (`ops/namespace/namespace_created.rs`), emitted at namespace creation in
    /// `handlers/create_group.rs`. The genesis is the FIRST op in the DAG —
    /// defined by having NO parents (its nonce is 1, since `read_head_record`
    /// defaults `next_nonce` to 1 when the head is absent; `op.nonce` is
    /// informational/signature-covered, DAG sequencing derives from
    /// `read_head_record().next_nonce`, not `op.nonce`). Being the parentless
    /// root, backfill applies it before any owner op and the correct admin row
    /// is present by the time owner ops apply.
    ///
    /// Strictly gated and idempotent:
    /// - only acts when `group_id` is the namespace root (`== namespace_id`);
    /// - the meta it writes (when absent) has a ZERO `admin_identity`/
    ///   `owner_identity` — a placeholder that grants authority to nobody. The
    ///   genesis op recognises this placeholder (admin == zero) and overwrites it
    ///   with the real founder; an established namespace (non-zero admin) is left
    ///   untouched, so seed-vs-genesis ordering converges either way;
    /// - the deliverer's own member row is written as a non-authoritative
    ///   `Member` (not `Admin`);
    /// - `target_application_id` is left zero and self-heals on the first
    ///   `ContextRegistered` apply (same contract as `join_group`);
    /// - each row is gated on its OWN presence so a partial seed self-repairs on
    ///   a later re-entry.
    ///
    /// Because no authority is conferred here, the old trust-on-first-use threat
    /// (PR #2473 finding 3 / #2474) is closed: a non-owner KeyDelivery can no
    /// longer pin the wrong admin.
    // `pub(crate)` (not private) so the repair/idempotency invariant below is
    // directly exercisable from `group_store::tests` without driving a full
    // `KeyDelivery` apply; it is not part of any published surface.
    pub(crate) fn seed_bootstrap_admin_if_absent(
        &self,
        group_id: [u8; 32],
        founder: &PublicKey,
    ) -> EyreResult<()> {
        if group_id != self.namespace_id {
            return Ok(());
        }
        let gid = ContextGroupId::from(group_id);

        // Repairable / idempotent seed. The seed writes two independent rows —
        // group meta and the admin member row — and `calimero-store` has no
        // atomic multi-key write (see the CRASH-SAFETY INVARIANT in
        // `local_state::persist_group_op_log_entry`), so a crash (or a transient
        // error) between the two `put`s can leave meta present but the admin
        // member row missing. Gating the WHOLE seed on `load_group_meta(..)
        // .is_some()` would then return early forever, never adding the member
        // row, and encrypted replay would keep failing the verifier-membership
        // check with no way to self-repair.
        //
        // Instead, gate each row on its OWN presence: only write meta if absent,
        // and ALWAYS ensure the admin member row exists. A later `KeyDelivery`
        // re-enters here and repairs whichever half a previous partial seed left
        // behind. Both writes are individually idempotent, so re-running is safe.
        // #2474: the bootstrap seed no longer pins the founding ADMIN/OWNER
        // identity from the KeyDelivery deliverer. The deliverer need only HOLD
        // the group key (any current member), so TOFU-trusting them as admin
        // pinned the WRONG admin whenever a non-owner delivered the key and
        // permanently wedged backfill. The authoritative founder now comes from
        // the replayable `RootOp::NamespaceCreated` genesis op
        // (`ops/namespace/namespace_created.rs`).
        //
        // The seed still writes a placeholder root meta when none exists so the
        // encrypted-op replay path has a meta row to read, but with a ZERO
        // `admin_identity`/`owner_identity` — granting authority to NOBODY. The
        // genesis op recognises this placeholder (admin == zero) and fills in
        // the real founder over it; an established namespace (non-zero admin) is
        // protected from a forged second genesis. Either ordering — seed first
        // or genesis first — converges on the genesis-supplied founder.
        // Shared zero-key sentinel — see `crate::PLACEHOLDER_ADMIN_IDENTITY`.
        // The genesis anti-hijack gate compares against the SAME constant, so
        // the seed and the gate cannot drift on this magic value (#2474).
        let placeholder_admin = crate::placeholder_admin_identity();
        let meta_existed = MetaRepository::new(self.store).load(&gid)?.is_some();
        if !meta_existed {
            let meta = calimero_store::key::GroupMetaValue {
                app_key: [0u8; 32],
                target_application_id: calimero_primitives::application::ApplicationId::from(
                    [0u8; 32],
                ),
                upgrade_policy: calimero_primitives::context::UpgradePolicy::default(),
                created_at: 0,
                admin_identity: placeholder_admin,
                owner_identity: placeholder_admin,
                migration: None,
                auto_join: true,
            };
            MetaRepository::new(self.store).save(&gid, &meta)?;
        }

        // Own-membership bootstrap: ensure the deliverer has a member row so the
        // encrypted-op replay membership checks pass — but as a non-authoritative
        // `Member`, NOT `Admin`. Founding authority is established by genesis, not
        // here. If genesis (or another path) has already recorded a richer role
        // for this identity, leave it untouched.
        let member_existed = MembershipRepository::new(self.store)
            .role_of(&gid, founder)?
            .is_some();
        if !member_existed {
            MembershipRepository::new(self.store).add_member(
                &gid,
                founder,
                GroupMemberRole::Member,
            )?;
        }

        // Seed the root's default capabilities so members added before the
        // separate `DefaultCapabilitiesSet` gossip arrives still inherit
        // `CAN_JOIN_OPEN_SUBGROUPS`, the bit that gates inheritance into Open
        // child subgroups. This mirrors the owner-side precedent in
        // `context::handlers::store_group_meta` (which sets the same default
        // when bootstrapping root meta from gossip).
        //
        // Without this, a TEE replica that bootstraps the namespace root via
        // this seed path admits its own `MemberJoinedViaTeeAttestation` row
        // with `caps = 0` (the row snapshots the group's default caps at apply
        // time), so `check_path` of any Open subgroup returns `None`,
        // auto-follow declines to `join_context`, and the Open subgroup's
        // context never replicates on the replica.
        //
        // Gated on its own absence so re-running (and a steady-state re-entry)
        // is idempotent and never clobbers an admin-authored override that the
        // gossiped `DefaultCapabilitiesSet` may have since installed.
        let default_caps_existed = CapabilitiesRepository::new(self.store)
            .default_capabilities(&gid)?
            .is_some();
        if !default_caps_existed {
            CapabilitiesRepository::new(self.store)
                .set_default_capabilities(&gid, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)?;
        }

        // Nothing to do (and nothing to log) if all halves were already
        // present — the common steady-state re-entry.
        if !meta_existed || !member_existed || !default_caps_existed {
            tracing::info!(
                namespace_id = %hex::encode(group_id),
                %founder,
                meta_seeded = !meta_existed,
                member_seeded = !member_existed,
                default_caps_seeded = !default_caps_existed,
                "seeded/repaired namespace bootstrap rows (placeholder meta + deliverer \
                 member + default caps); founding admin comes from NamespaceCreated genesis"
            );
        }
        Ok(())
    }

    /// Responder side of direct key delivery — the counterpart to
    /// [`apply_received_group_key`](Self::apply_received_group_key). Build
    /// an ECDH-wrapped group key for `requester` when they are a current
    /// member of `group_id` (which must belong to this namespace) and we
    /// hold the key.
    ///
    /// Returns `(envelope_bytes, responder_identity)`. `envelope_bytes` is
    /// **empty** for every non-deliverable case — `group_id` not in this
    /// namespace (cross-namespace pin), `requester` not a member, key not
    /// held locally, no namespace identity, or a wrap failure — so the
    /// requester simply tries another peer; an empty reply leaks no
    /// membership oracle. `responder_identity` is our namespace identity
    /// (the wrap sender / bootstrap trust anchor) when we wrapped; for an
    /// empty envelope it is irrelevant (the joiner ignores it).
    pub(crate) fn build_group_key_delivery(
        &self,
        group_id: [u8; 32],
        requester: PublicKey,
    ) -> EyreResult<(Vec<u8>, PublicKey)> {
        let group_gid = ContextGroupId::from(group_id);
        let ns_gid = ContextGroupId::from(self.namespace_id);

        // Cross-namespace pin: the requested group must belong to the
        // namespace the requester named, otherwise an attacker on namespace
        // A could elicit a key for a group of namespace B. Combined with the
        // membership check, this is the full authorisation gate.
        let group_in_namespace = matches!(
            NamespaceRepository::new(self.store).resolve(&group_gid),
            Ok(ns) if ns.to_bytes() == self.namespace_id
        );
        if !group_in_namespace
            || !MembershipRepository::new(self.store).is_member(&group_gid, &requester)?
        {
            return Ok((Vec::new(), requester));
        }

        let Some((_key_id, group_key)) =
            GroupKeyring::new(self.store, group_gid).load_current_key()?
        else {
            return Ok((Vec::new(), requester));
        };
        let Some(record) = NamespaceRepository::new(self.store).resolve_identity_record(&ns_gid)?
        else {
            tracing::warn!(
                namespace_id = %hex::encode(self.namespace_id),
                "no namespace identity, cannot wrap group key"
            );
            return Ok((Vec::new(), requester));
        };
        let sender_sk = PrivateKey::from(record.private_key);
        let responder_identity = sender_sk.public_key();
        match GroupKeyring::wrap_for_member(&sender_sk, &requester, &group_key) {
            Ok(envelope) => Ok((
                borsh::to_vec(&envelope).unwrap_or_default(),
                responder_identity,
            )),
            Err(err) => {
                tracing::warn!(
                    namespace_id = %hex::encode(self.namespace_id),
                    group_id = %hex::encode(group_id),
                    %err,
                    "failed to wrap group key for requester"
                );
                Ok((Vec::new(), responder_identity))
            }
        }
    }

    /// Apply a group key received out-of-band via the direct
    /// (pull-based) `GroupKeyRequest`/`GroupKeyResponse` sync exchange.
    /// This is the durable replacement for the on-DAG `KeyDelivery` op's
    /// side effect: unwrap the ECDH envelope for our namespace identity,
    /// store the key, seed the bootstrap admin (for a keyless root-group
    /// join), and replay any encrypted ops buffered awaiting the key.
    ///
    /// `responder_identity` is the peer that served the key — the trust
    /// anchor used to seed the namespace admin when this node joined
    /// without an invitation (the TEE fleet-join path). It plays the
    /// role the old `KeyDelivery` op's signer did: only an existing
    /// key-holding member of this namespace can mint a wrapped key for
    /// us, so for the bootstrap case that member is the admitting owner.
    ///
    /// Returns any post-apply divergence surfaced by the replayed ops
    /// (the caller routes it to reconcile-via-anchor), or `None`. An
    /// envelope not addressed to us, or that fails to unwrap, is a benign
    /// `Ok(None)` — the joiner simply tries another peer next round.
    pub(crate) fn apply_received_group_key(
        &self,
        group_id: [u8; 32],
        envelope_bytes: &[u8],
        responder_identity: PublicKey,
    ) -> EyreResult<Option<super::super::DivergenceReport>> {
        let ns_id = ContextGroupId::from(self.namespace_id);

        let envelope: KeyEnvelope = match borsh::from_slice(envelope_bytes) {
            Ok(env) => env,
            Err(e) => {
                tracing::warn!(?e, "failed to decode GroupKeyResponse envelope");
                return Ok(None);
            }
        };

        let Some(identity) = NamespaceRepository::new(self.store).identity_record(&ns_id)? else {
            return Ok(None);
        };
        let recipient_sk = PrivateKey::from(identity.private_key);
        // Defensive: the responder wraps for the public key we asked
        // with, but a misbehaving/stale peer could send an envelope for
        // someone else. Storing a key we can't actually use would be
        // harmless but pointless; reject it.
        if envelope.recipient != recipient_sk.public_key() {
            return Ok(None);
        }

        let group_key = match GroupKeyring::unwrap_for_recipient(&recipient_sk, &envelope) {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!(?e, "failed to unwrap received group key envelope");
                return Ok(None);
            }
        };

        let gid = ContextGroupId::from(group_id);
        let key_id = GroupKeyring::new(self.store, gid)
            .store_key(&group_key)
            .map_err(|e| eyre::eyre!("store_group_key: {e}"))?;
        tracing::info!(
            group_id = %hex::encode(group_id),
            key_id = %hex::encode(key_id),
            "received group key via direct delivery"
        );

        // Wake any `join_group` future waiting on the key. Emit before
        // the (potentially slow) encrypted-op replay so the wake-up
        // isn't blocked behind it.
        notify_op_event(OpEvent::GroupKeyDelivered {
            group_id,
            recipient: recipient_sk.public_key(),
        });

        // Bootstrap the founding admin/owner for a node that joined
        // WITHOUT an invitation (the TEE fleet-join path). Gated inside
        // `seed_bootstrap_admin_if_absent` to the namespace root group
        // and only when no group meta exists yet, so the invited-join
        // path (which already wrote meta) is never touched.
        if let Err(e) = self.seed_bootstrap_admin_if_absent(group_id, &responder_identity) {
            tracing::warn!(
                group_id = %hex::encode(group_id),
                error = %format!("{e:#}"),
                "failed to seed bootstrap admin from direct key delivery; \
                 encrypted-op replay may reject"
            );
        }

        self.retry_encrypted_ops_for_group(group_id)
            .map_err(|e| eyre::eyre!("retry_encrypted_ops_for_group: {e}"))
    }

    fn retry_encrypted_ops_for_group(
        &self,
        group_id: [u8; 32],
    ) -> EyreResult<Option<super::super::DivergenceReport>> {
        let gid_typed = ContextGroupId::from(group_id);
        let retry_service = NamespaceRetryService::new(self.store, self.namespace_id);
        let retry_candidates = retry_service
            .collect_retry_candidates_for_group(group_id)
            .map_err(|e| eyre::eyre!("collect_retry_candidates_for_group: {e}"))?;
        let attempted = retry_candidates.len();
        if attempted > 0 {
            record_namespace_retry_event("collected");
        }

        // Last-writer-wins across retry candidates that surface
        // divergence. The outbox carrying this report to the node
        // handler is a single slot (see `governance_dag.rs`), so
        // collapsing here matches the fresh-arrival path's LWW
        // semantics. In practice each retry batch unblocks a small
        // number of ops and at most one is a `MemberRemoved` /
        // `MemberLeft` that could report divergence.
        let mut retry_divergence: Option<super::super::DivergenceReport> = None;

        for candidate in &retry_candidates {
            let NamespaceOp::Group { ref encrypted, .. } = candidate.signed_op.op else {
                continue;
            };
            match self.decrypt_and_apply_group_op(
                &candidate.signed_op,
                &gid_typed,
                &candidate.group_key,
                encrypted,
            ) {
                // Surface divergence from retry-path applies. Once a
                // retry replay applies an op, the DAG marks any later
                // fresh arrival of the same op as `Duplicate` and the
                // apply work — including the post-apply hash check —
                // is skipped. That makes the retry path the *only*
                // opportunity to detect divergence on retried ops:
                // dropping it here means the reconcile trigger never
                // fires for `MemberRemoved` / `MemberLeft` ops that
                // were buffered pending `KeyDelivery`.
                Ok(divergence) => {
                    record_namespace_retry_event("applied");
                    tracing::info!(
                        group_id = %hex::encode(group_id),
                        "retried encrypted op after KeyDelivery"
                    );
                    if divergence.is_some() {
                        retry_divergence = divergence;
                    }
                }
                Err(e) => {
                    record_namespace_retry_event("failed");
                    tracing::warn!(
                        group_id = %hex::encode(group_id),
                        error = %format!("{e:#}"),
                        "failed to retry encrypted op after KeyDelivery"
                    );
                }
            }
        }

        if attempted == 0 {
            record_namespace_retry_event("none");
        }

        Ok(retry_divergence)
    }

    /// Curative re-drive variant that returns the count of ops it ACTUALLY
    /// applied (#2848 Part C). The `Option<DivergenceReport>` returned by
    /// [`retry_encrypted_ops_for_group`](Self::retry_encrypted_ops_for_group)
    /// is not a usable progress signal — a re-driven `ContextRegistered`
    /// applies yet reports `None` (divergence is surfaced only for
    /// `MemberRemoved`/`MemberLeft`), and an already-applied op is
    /// nonce-deduped to `Ok(None)` too. The startup sweep needs to know
    /// whether a pass made progress; this counts it directly.
    ///
    /// "Applied" is detected by the nonce window: a candidate whose
    /// (signer, nonce) was NOT in the window before the apply but IS after it
    /// was genuinely applied this call; an op already in the window is an
    /// idempotent nonce-skip and does not count. This avoids threading an
    /// `applied` flag through the whole apply stack while still distinguishing
    /// a real apply from a no-op replay.
    ///
    /// TOCTOU caveat (accepted, bounded, benign): the pre/post nonce-window
    /// read is not atomic with the apply, so a concurrent gossip apply of the
    /// same op can make us miscount. The inflating race is a concurrent apply
    /// landing BEFORE our apply: we pre-read `was_present == false`, the gossip
    /// path then windows the nonce, and our `decrypt_and_apply` short-circuits
    /// to `Ok(None)` WITHOUT writing — yet the post-read sees `now_present ==
    /// true`, so `!was_present && now_present` holds and we count a false
    /// "applied" for an op we did not actually apply. (This does NOT produce a
    /// spurious `nonce_skip`: that branch is taken only when the post-read is
    /// still `false`, which a concurrent apply cannot cause.) The only
    /// consequence is the convergence count being over-counted, which at worst
    /// triggers one extra sweep pass; that is bounded by `MAX_PASSES`. The
    /// count is a best-effort convergence SIGNAL, not a correctness gate — no
    /// state can diverge from a miscount. The proper long-term fix is to have
    /// `decrypt_and_apply_group_op` return an explicit `Applied | Skipped |
    /// Failed` outcome instead of inferring it from the window; that refactor
    /// is deliberately deferred.
    fn redrive_encrypted_ops_for_group_counted(&self, group_id: [u8; 32]) -> EyreResult<usize> {
        let gid_typed = ContextGroupId::from(group_id);
        let retry_service = NamespaceRetryService::new(self.store, self.namespace_id);
        let retry_candidates = retry_service
            .collect_retry_candidates_for_group(group_id)
            .map_err(|e| eyre::eyre!("collect_retry_candidates_for_group: {e}"))?;
        if !retry_candidates.is_empty() {
            record_namespace_retry_event("collected");
        }

        let mut applied = 0usize;
        for candidate in &retry_candidates {
            let NamespaceOp::Group { ref encrypted, .. } = candidate.signed_op.op else {
                continue;
            };
            let signer = &candidate.signed_op.signer;
            let nonce = candidate.signed_op.nonce;
            // Pre-apply nonce-window membership tells a real apply apart from an
            // idempotent replay: `apply_group_op_inner` short-circuits an
            // already-windowed nonce to `Ok(None)` WITHOUT mutating, and
            // advances the window only on a genuine apply.
            let was_present = load_nonce_window(self.store, &gid_typed, signer)
                .map(|w| w.contains(nonce))
                .unwrap_or(false);
            match self.decrypt_and_apply_group_op(
                &candidate.signed_op,
                &gid_typed,
                &candidate.group_key,
                encrypted,
            ) {
                Ok(_divergence) => {
                    let now_present = load_nonce_window(self.store, &gid_typed, signer)
                        .map(|w| w.contains(nonce))
                        .unwrap_or(false);
                    if !was_present && now_present {
                        // Genuine apply: the (signer, nonce) was not in the
                        // window before and is now. Only this path counts and
                        // only this path increments the "applied" metric — a
                        // nonce-deduped replay short-circuits to `Ok(None)`
                        // WITHOUT writing, so counting it as "applied" would
                        // inflate the metric (review fix B).
                        record_namespace_retry_event("applied");
                        applied += 1;
                        tracing::info!(
                            group_id = %hex::encode(group_id),
                            "curative re-drive applied a stranded encrypted op (#2848)"
                        );
                    } else {
                        // Idempotent nonce-deduped replay: nothing was written.
                        record_namespace_retry_event("nonce_skip");
                    }
                }
                Err(e) => {
                    record_namespace_retry_event("failed");
                    tracing::warn!(
                        group_id = %hex::encode(group_id),
                        error = %format!("{e:#}"),
                        "curative re-drive: failed to apply a buffered encrypted op (#2848)"
                    );
                }
            }
        }

        Ok(applied)
    }

    /// Decrypt an encrypted group op and apply it via
    /// [`apply_group_op_inner`](Self::apply_group_op_inner).
    fn decrypt_and_apply_group_op(
        &self,
        ns_op: &SignedNamespaceOp,
        group_id: &ContextGroupId,
        group_key: &[u8; 32],
        encrypted: &EncryptedGroupOp,
    ) -> EyreResult<Option<super::super::DivergenceReport>> {
        let inner_op = GroupKeyring::decrypt_op(group_key, encrypted)?;

        let signed_group_op = SignedGroupOp {
            version: calimero_context_client::local_governance::SIGNED_GROUP_OP_SCHEMA_VERSION,
            group_id: group_id.to_bytes(),
            parent_op_hashes: ns_op.parent_op_hashes.clone(),
            signer: ns_op.signer,
            nonce: ns_op.nonce,
            op: inner_op,
            signature: ns_op.signature,
        };

        self.apply_group_op_inner(group_id, &signed_group_op)
    }

    fn apply_group_op_inner(
        &self,
        group_id: &ContextGroupId,
        signed_group_op: &SignedGroupOp,
    ) -> EyreResult<Option<super::super::DivergenceReport>> {
        let signer = &signed_group_op.signer;
        let nonce = signed_group_op.nonce;
        let op = &signed_group_op.op;

        // Windowed nonce dedup (the anti-replay gate; C5.S3b removed the op-level
        // state_hash staleness check that used to follow it).
        // `retry_encrypted_ops_for_group` (fires on every KeyDelivery) reads back
        // the entire op log for the group and re-feeds each entry through
        // `decrypt_and_apply_group_op` → `apply_group_op_inner`, so already-applied
        // ops arrive again; the window short-circuits them to `Ok(None)`.
        // A nonce is a duplicate iff it's at or below the contiguous floor OR
        // already in the above-floor set. This is what fixes #2516 — two concurrent
        // same-signer ops are DAG siblings with consecutive nonces and can arrive in
        // either order; the window holds the higher one in `above` and still applies
        // the lower one when it arrives.
        let mut nonce_window = load_nonce_window(self.store, group_id, signer)?;
        if nonce_window.contains(nonce) {
            tracing::debug!(
                nonce,
                floor = nonce_window.floor(),
                signer = %signer,
                "ignoring namespace group op with already-processed nonce"
            );
            return Ok(None);
        }

        if let GroupOp::ContextRegistered {
            application_id,
            blob_id,
            source,
            ..
        } = op
        {
            // service_name is stored by apply_group_op_mutations (called below)
            // via set_context_service_name. We intentionally do NOT write
            // ContextMeta here — that would cause has_context() to return true
            // and skip the bootstrap path in join_context.
            if *application_id != ZERO_APPLICATION_ID {
                let app_key = calimero_store::key::ApplicationMeta::new(*application_id);
                let handle = self.store.handle();
                if !handle.has(&app_key)? {
                    drop(handle);
                    let blob_meta = calimero_store::key::BlobMeta::new(*blob_id);
                    let effective_source = if source.starts_with("file://") || source.is_empty() {
                        "calimero://pending-blob-share".to_owned()
                    } else {
                        source.clone()
                    };
                    let stub = calimero_store::types::ApplicationMeta::new(
                        blob_meta,
                        0,
                        effective_source.into_boxed_str(),
                        Vec::new().into_boxed_slice(),
                        blob_meta,
                        calimero_store::types::PackageInfo {
                            package: String::new().into_boxed_str(),
                            version: String::new().into_boxed_str(),
                            signer_id: String::new().into_boxed_str(),
                        },
                    );
                    let mut wh = self.store.handle();
                    wh.put(&app_key, &stub)?;
                    tracing::info!(
                        %application_id,
                        blob_id = %blob_id,
                        "created stub application entry from ContextRegistered"
                    );
                }
            }
        }

        // Group ops carried in namespace envelopes authorize at the cut of THEIR OWN
        // enclosing namespace op (F5 #28 stage 4) — `signed_group_op.parent_op_hashes`
        // is that op's parents (copied in `decrypt_and_apply_group_op`). Use it, NOT
        // `self.parents`: `retry_encrypted_ops_for_group` re-applies buffered group
        // candidates within ONE KeyDelivery apply, so `self.parents` would be the
        // KeyDelivery cut for every replayed candidate — the projection-backed gates
        // would then judge each at the wrong cut. The authorizer carries no per-op
        // state (its fold is the whole namespace; the cut is applied at resolve
        // time), so reusing `self.authorizer` with the candidate's parents is correct.
        let (handled, divergence, pending_events) = apply_group_op_mutations(
            self.store,
            group_id,
            signer,
            op,
            &signed_group_op.parent_op_hashes,
            self.authorizer,
        )?;
        if !handled {
            tracing::debug!(
                ?op,
                "namespace group op variant not handled by inner apply, stored as skeleton"
            );
        }

        // Append the decrypted op to the local group op-log, mirroring the
        // authoring node's `apply_local_signed_group_op`. Several readers
        // reconstruct namespace-scoped state from this log rather than from
        // dedicated tables — notably `read_tee_admission_policy`,
        // `is_quote_hash_used`, and `is_tee_admitted_identity`
        // (`group_store/tee.rs`). Before this, the op-log was only ever
        // written on the node that *authored* an op, so a replica that
        // received the same op via the namespace governance DAG could apply
        // its state mutation but could NOT later read it back through these
        // log-scanning helpers. That left a freshly-admitted TEE replica
        // unable to validate (and therefore apply) its own
        // `MemberJoinedViaTeeAttestation` op — the policy that admission
        // requires is read from this very log. Persisting here makes the
        // replica's op-log symmetric with the author's, so policy / quote /
        // admitted-identity lookups resolve on every member. The op-log
        // sequence is node-local (not part of consensus), so a divergent
        // sequence between nodes is fine. Only persist a *handled* op — an
        // unhandled variant is intentionally skeleton-only.
        if handled {
            let content_hash = signed_group_op
                .content_hash()
                .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
            // CONCURRENCY ASSUMPTION (single-threaded per-group apply): the
            // read-then-write of the op-log below (max-sequence scan + persist)
            // is NOT individually atomic, so it is only correct if applies for a
            // given group never interleave. They don't: every receive-path apply
            // runs inside `ContextManager`'s actix actor, which processes its
            // mailbox sequentially, so all `apply_signed_op` →
            // `apply_group_op_inner` calls for one namespace/group are
            // serialized. The authoring path (`apply_local_signed_group_op`)
            // documents the same "callers must serialize per `group_id`"
            // contract. Concurrent applies for the SAME group would be unsafe
            // (duplicate/overwritten sequences); cross-group concurrency is fine.
            //
            // Idempotency: a re-received op (gossip duplicate, backfill
            // replay) must not append a second log entry — that would
            // duplicate policy/quote rows and skew the node-local sequence.
            // The nonce guard above already short-circuits the common
            // re-receive; this content-hash check covers the retry path,
            // which re-applies via `decrypt_and_apply_group_op` (bypassing
            // the nonce window's `record`/`store_nonce_window` on first apply).
            //
            // Dedup against the PERSISTED op-log, not the op-head's
            // `dag_heads`: scanning the log is monotonic, so an op that was
            // ever logged stays deduped, whereas a head-based check would miss
            // a superseded-then-replayed op and append a duplicate — skewing
            // the very log scans (`is_quote_hash_used`, policy replay,
            // `read_tee_admission_policy`) this entry feeds.
            let already_logged = super::super::local_state::op_log_contains_content_hash(
                self.store,
                group_id,
                &content_hash,
            )?;
            if !already_logged {
                // Derive `next_seq` from the ACTUAL max op-log sequence, not
                // from `GroupOpHeadValue.sequence`. The head can lag the log
                // after a crash that landed between the entry `put` and the
                // head `put` in `persist_group_op_log_entry`: a stale head
                // would make a different op reuse the orphan's sequence and
                // silently overwrite it (e.g. losing a `TeeAdmissionPolicySet`
                // that a later membership op depends on). Scanning the log is
                // self-healing — the next op always lands strictly above every
                // persisted entry. This also removes any reliance on a possibly
                // stale `get_op_head` snapshot for sequencing.
                let next_seq =
                    super::super::local_state::max_op_log_sequence(self.store, group_id)?
                        .map_or(1, |max| max.saturating_add(1));
                let op_bytes =
                    borsh::to_vec(signed_group_op).map_err(|e| eyre::eyre!("borsh: {e}"))?;
                // The group op-log is a node-local LINEAR append-only sequence;
                // its op-head's `dag_heads` is a purely-local frontier that
                // never escapes the node (all wire/heartbeat/readiness positions
                // read `NamespaceGovHead.dag_heads`, a different key). Set the
                // head to exactly the just-logged op's hash. The previous
                // append-then-prune used `signed_group_op.parent_op_hashes`,
                // which on this replica path are the reconstructed NAMESPACE op's
                // DAG parents (see `decrypt_and_apply_group_op`), NOT group-op
                // hashes — so the prune `filter` never matched and `dag_heads`
                // grew without bound. A linear single-element head is correct for
                // the only remaining group-op-head reader (the authoring path's
                // `parent_op_hashes`, also node-local) and bounded by design.
                let new_heads = vec![content_hash];
                super::super::local_state::persist_group_op_log_entry(
                    self.store, group_id, next_seq, new_heads, &op_bytes,
                )?;
                // #2770: flush after the op-log append; a re-received op
                // (already_logged) drops its queued events (no re-emit). See
                // the canonical dedup-tradeoff note in
                // `apply_local_signed_group_op` (lib.rs) for why dropping on
                // replay is correct and why the crash-between-append-and-flush
                // window is an accepted, bounded gap (not an FS hole).
                for event in pending_events {
                    crate::op_events::notify(event);
                }
            }
        }

        // INVARIANT: the per-(group, signer) nonce only advances AFTER the op
        // has been fully applied above — i.e. `apply_group_op_mutations`
        // returned `Ok` and (for handled ops) the op-log entry is durably
        // written. Any precondition failure inside `apply_group_op_mutations`
        // (e.g. `MemberJoinedViaTeeAttestation` reading a not-yet-visible
        // `TeeAdmissionPolicySet`, or a verifier-membership check that depends
        // on an earlier op) returns `Err` via the `?` above and short-circuits
        // BEFORE this line, so the nonce is left unadvanced and the op is
        // re-attempted on the next sync/retry pass once its predecessor op is
        // durable. This is what lets a freshly-admitted TEE replica recover:
        // within a single retry batch the policy op (nonce N) commits its
        // op-log entry (the store is unbuffered, so the write is immediately
        // readable), and the membership op (nonce N+1) — applied next because
        // candidates are sorted by (signer, nonce) — sees it. Were the policy
        // ever not-yet-visible, the membership op would `Err` here and NOT burn
        // its nonce, so a later round retries it rather than skipping it
        // forever as "already-processed". Conversely a genuinely-applied op
        // (or an unhandled skeleton variant) reaches this line and advances the
        // nonce so it is not replayed. We deliberately do NOT advance the nonce
        // for a deferrable precondition failure, and we deliberately DO advance
        // it for a successful apply — the security check against the policy is
        // never skipped, only made visible in time.
        // Record into the window loaded at the dedup guard above. `record`
        // advances the contiguous floor through any run this nonce completed,
        // or parks it in the above-floor set if an earlier sibling is still
        // missing — so the missing sibling is NOT treated as already-processed
        // when it arrives (the #2516 fix). The same post-apply ordering as the
        // old single-nonce write holds: a deferrable precondition failure
        // `Err`s above and leaves the window untouched, so the op retries.
        nonce_window.record(nonce);
        store_nonce_window(self.store, group_id, signer, &nonce_window)?;
        Ok(divergence)
    }

    fn apply_root_op(
        &self,
        op: &SignedNamespaceOp,
        root: &RootOp,
    ) -> EyreResult<Vec<crate::op_events::OpEvent>> {
        // C5.S3b removed the op-level state_hash staleness telemetry that used to
        // run here for state-committing root variants. `scope_root` is the
        // convergence signal now; signature + the nonce window (applied by the
        // caller) remain the safety gates.

        // Per-variant logic lives in `ops/namespace/<variant>.rs` (#2481). The
        // op's causal cut + the at-cut authorizer ride along so the gates can
        // authorize against the projection at the op's parents (F5 #28).
        let mut ctx = super::super::ops::namespace::NamespaceApplyCtx::new(
            self.store,
            self.namespace_id,
            self.parents,
            self.authorizer,
        );
        super::super::ops::namespace::dispatch_root_op(&mut ctx, op, root)?;
        Ok(ctx.take_events())
    }
}

/// Apply a signed namespace op with the LIVE apply-auth gates (no causal cut).
/// The backward-compatible entry for call sites without an at-cut authorizer
/// (tests, internal facades); the production apply path uses
/// [`apply_signed_namespace_op_at_cut`].
pub fn apply_signed_namespace_op(
    store: &Store,
    op: &SignedNamespaceOp,
) -> EyreResult<ApplyNamespaceOpResult> {
    apply_signed_namespace_op_at_cut(store, op, &[], &crate::authorizer::LIVE_FALLBACK_AUTHORIZER)
}

/// Apply a signed namespace op, authorizing the apply gates against `authorizer`
/// at the op's causal cut `parents` (F5 #28). The gate consults the projection-
/// backed authorizer first and falls back to the live resolver on `None` (an
/// incomplete fold). This is the production apply path.
pub fn apply_signed_namespace_op_at_cut(
    store: &Store,
    op: &SignedNamespaceOp,
    parents: &[[u8; 32]],
    authorizer: &dyn crate::authorizer::AtCutAuthorizer,
) -> EyreResult<ApplyNamespaceOpResult> {
    NamespaceGovernance::new(store, op.namespace_id)
        .with_apply_auth(parents, authorizer)
        .apply_signed_op(op)
}

/// Decrypt the cleartext [`GroupOp`] carried by a `NamespaceOp::Group` envelope
/// **without applying it** — the read-only counterpart of the private
/// `decrypt_and_apply_group_op`, for the unified-op projection shadow feed,
/// which folds the cleartext membership op but must never re-run the mutation.
///
/// Mirrors the same key resolution the apply path uses: try the subgroup's own
/// keyring first, then fall back to the parent namespace's key (issue #2256 —
/// an Open subgroup is encrypted with the namespace key). `Ok(None)` when no
/// key resolves locally, i.e. the op was never decryptable on this node and so
/// there is nothing to fold.
pub fn decrypt_group_op(
    store: &Store,
    namespace_id: [u8; 32],
    group_id: ContextGroupId,
    key_id: &[u8; 32],
    encrypted: &EncryptedGroupOp,
) -> EyreResult<Option<GroupOp>> {
    let resolved = match GroupKeyring::new(store, group_id).load_key_by_id(key_id)? {
        Some(k) => Some(k),
        None => {
            GroupKeyring::new(store, ContextGroupId::from(namespace_id)).load_key_by_id(key_id)?
        }
    };
    match resolved {
        Some(group_key) => Ok(Some(GroupKeyring::decrypt_op(&group_key, encrypted)?)),
        None => Ok(None),
    }
}

/// Build an ECDH-wrapped group key to deliver to `requester` in response
/// to a `GroupKeyRequest`. See
/// [`NamespaceGovernance::build_group_key_delivery`].
pub fn build_group_key_delivery(
    store: &Store,
    namespace_id: [u8; 32],
    group_id: [u8; 32],
    requester: PublicKey,
) -> EyreResult<(Vec<u8>, PublicKey)> {
    NamespaceGovernance::new(store, namespace_id).build_group_key_delivery(group_id, requester)
}

/// Apply a group key delivered out-of-band via the direct
/// `GroupKeyRequest`/`GroupKeyResponse` sync exchange. See
/// [`NamespaceGovernance::apply_received_group_key`].
pub fn apply_received_group_key(
    store: &Store,
    namespace_id: [u8; 32],
    group_id: [u8; 32],
    envelope_bytes: &[u8],
    responder_identity: PublicKey,
) -> EyreResult<Option<super::super::DivergenceReport>> {
    NamespaceGovernance::new(store, namespace_id).apply_received_group_key(
        group_id,
        envelope_bytes,
        responder_identity,
    )
}

/// Re-drive any buffered encrypted group ops for `group_id` that were
/// effect-skipped because their dependencies (key or subgroup meta) were
/// not yet present. See
/// [`NamespaceGovernance::retry_encrypted_ops_for_group`]. Used by the
/// context crate to recover ops stranded between `GroupCreated` and
/// `KeyDelivery` (#2848). The #2848 Part C curative startup sweep is the
/// in-tree caller and lands in a follow-up task.
pub fn retry_encrypted_ops_for_group(
    store: &Store,
    namespace_id: [u8; 32],
    group_id: [u8; 32],
) -> EyreResult<Option<super::super::DivergenceReport>> {
    NamespaceGovernance::new(store, namespace_id).retry_encrypted_ops_for_group(group_id)
}

/// Distinct group ids in `namespace_id` that have at least one buffered
/// encrypted group op the local node cannot yet decrypt because it holds
/// no key for that group (nor the namespace key, which `Open` subgroups
/// may have been encrypted under). This is the joiner-side recovery set
/// for the direct key-delivery pull: each round of sync asks a peer for
/// the keys to exactly these groups.
pub fn namespace_groups_awaiting_key(
    store: &Store,
    namespace_id: [u8; 32],
) -> EyreResult<Vec<[u8; 32]>> {
    NamespaceRetryService::new(store, namespace_id).groups_awaiting_key()
}

/// Distinct group ids in `namespace_id` that have at least one buffered
/// encrypted group op the local node CAN already decrypt — the inverse of
/// [`namespace_groups_awaiting_key`]. This is the held-key, buffered-op set
/// the #2848 Part C curative startup sweep re-drives: a node stranded before
/// the live re-drive landed holds the key but has no future trigger.
pub fn namespace_groups_with_held_key_buffered_ops(
    store: &Store,
    namespace_id: [u8; 32],
) -> EyreResult<Vec<[u8; 32]>> {
    NamespaceRetryService::new(store, namespace_id).groups_with_held_key_buffered_ops()
}

/// Enumerate every namespace this node holds an identity for — the node's
/// full set of known namespaces. The #2848 Part C curative startup sweep
/// iterates this to re-drive stranded buffered ops across all of them.
pub fn known_namespace_identities(store: &Store) -> EyreResult<Vec<[u8; 32]>> {
    Ok(NamespaceRepository::new(store)
        .iter_identities()?
        .into_iter()
        .map(|gid| gid.to_bytes())
        .collect())
}

/// Curative re-drive of a single group's buffered encrypted ops for the
/// #2848 Part C startup sweep. Returns the count of ops it ACTUALLY applied
/// this call (idempotent nonce-replays are not counted) — the sweep uses this
/// as its monotone convergence signal.
///
/// Metric emission is delegated to the inner
/// `redrive_encrypted_ops_for_group_counted`, which records
/// `namespace_retry_events{status="applied"}` only on genuine applies and
/// `status="nonce_skip"` on dedup. We intentionally do NOT emit a per-call
/// `status="redriven"` counter here: that inflated the metric on every sweep
/// pass over an already-drained group, so the count reflected invocations
/// rather than ops actually recovered.
pub fn redrive_buffered_ops_for_group(
    store: &Store,
    namespace_id: [u8; 32],
    group_id: [u8; 32],
) -> EyreResult<usize> {
    NamespaceGovernance::new(store, namespace_id).redrive_encrypted_ops_for_group_counted(group_id)
}

pub async fn sign_apply_and_publish_namespace_op(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    ack_router: &AckRouter,
    namespace_id: [u8; 32],
    signer_sk: &PrivateKey,
    op: NamespaceOp,
) -> EyreResult<DeliveryReport> {
    NamespaceGovernance::new(store, namespace_id)
        .sign_apply_and_publish(node_client, ack_router, signer_sk, op)
        .await
}

pub async fn sign_and_publish_namespace_op(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    ack_router: &AckRouter,
    namespace_id: [u8; 32],
    signer_sk: &PrivateKey,
    op: NamespaceOp,
    required_signers: Option<Vec<PublicKey>>,
) -> EyreResult<DeliveryReport> {
    NamespaceGovernance::new(store, namespace_id)
        .sign_and_publish_without_apply(node_client, ack_router, signer_sk, op, required_signers)
        .await
}

pub fn collect_skeleton_delta_ids_for_group(
    store: &Store,
    namespace_id: [u8; 32],
    group_id: [u8; 32],
) -> EyreResult<Vec<[u8; 32]>> {
    NamespaceGovernance::new(store, namespace_id).collect_skeleton_delta_ids_for_group(group_id)
}
