use calimero_context_client::local_governance::{
    AckRouter, EncryptedGroupOp, GroupOp, NamespaceOp, RootOp, SignedGroupOp, SignedNamespaceOp,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ZERO_APPLICATION_ID;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use libp2p::gossipsub::TopicHash;

use crate::governance_broadcast::{
    self, assert_transport_ready, ns_topic, publish_and_await_ack_namespace,
    timeout_for_namespace_op, DeliveryReport,
};
use crate::metrics::{record_governance_publish_mesh_peers, record_namespace_retry_event};
use crate::op_events::{notify as notify_op_event, OpEvent};

use super::{
    add_group_member, apply_group_op_mutations, clear_denied, decrypt_group_op,
    get_local_gov_nonce, get_namespace_identity_record, is_group_admin,
    is_group_admin_or_has_capability, load_current_group_key_record, load_group_key_by_id,
    load_group_meta,
    namespace_dag::{NamespaceDagService, NamespaceHead},
    namespace_membership::NamespaceMembershipService,
    namespace_op_log::NamespaceOpLogService,
    namespace_retry::NamespaceRetryService,
    restore_member_context_identities, save_group_meta, set_local_gov_nonce, store_group_key,
    unwrap_group_key, PermissionChecker,
};

/// Side effect returned by namespace-op application when an existing
/// member should deliver the group key to a joiner.
#[derive(Debug)]
pub struct PendingKeyDelivery {
    pub namespace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub joiner_pk: PublicKey,
}

/// A key delivery or rotation unwrap failure that the caller should handle.
#[derive(Debug)]
pub struct KeyUnwrapFailure {
    pub group_id: [u8; 32],
    pub reason: String,
}

/// Result of applying a namespace governance op.
#[derive(Debug, Default)]
pub struct ApplyNamespaceOpResult {
    pub pending_deliveries: Vec<PendingKeyDelivery>,
    pub key_unwrap_failures: Vec<KeyUnwrapFailure>,
    /// Post-apply hash divergence detected by the cross-DAG state-hash
    /// check inside a `MemberRemoved` / `MemberLeft` apply. The node
    /// handler routes this to the reconcile-via-anchor sync trigger.
    /// `None` for ops that don't carry signed convergence claims, for
    /// ops that match the receiver's view, and for ops that don't go
    /// through the verify path at all.
    pub divergence: Option<super::DivergenceReport>,
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

/// Domain API for namespace DAG and governance operation lifecycle.
pub struct NamespaceGovernance<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
}

impl<'a> NamespaceGovernance<'a> {
    pub fn new(store: &'a Store, namespace_id: [u8; 32]) -> Self {
        Self {
            store,
            namespace_id,
        }
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
        // side effects in the match arms included. Re-running them would either
        // be redundant (they're written replay-safe) or actively wrong (a
        // second `PendingKeyDelivery`); a maintainer adding a new side effect
        // should keep it replay-safe rather than rely on this guard never
        // firing. The encrypted-op *retry* path is unaffected: it re-applies
        // via `decrypt_and_apply_group_op` → `apply_group_op_inner`, not
        // `apply_signed_op`, and is bounded by the per-signer nonce check there.
        if NamespaceOpLogService::new(self.store, self.namespace_id).contains_op(delta_id)? {
            tracing::debug!(
                namespace_id = %hex::encode(self.namespace_id),
                delta_id = %hex::encode(delta_id),
                "namespace governance op already applied; skipping re-apply (#2327)"
            );
            return Ok(ApplyNamespaceOpResult::default());
        }

        let mut result = ApplyNamespaceOpResult::default();

        match &op.op {
            NamespaceOp::Root(root) => {
                self.apply_root_op(op, root)?;

                match root {
                    RootOp::KeyDelivery {
                        group_id,
                        ref envelope,
                    } => {
                        let ns_id = ContextGroupId::from(op.namespace_id);
                        // Any error inside the KeyDelivery side-effect path below
                        // is captured and logged, but NOT propagated. KeyDelivery
                        // is an idempotent best-effort op — its side-effect (storing
                        // a group key locally) is not part of governance consensus.
                        // Failing to apply the side-effect must not block the DAG,
                        // because every subsequent governance op for this namespace
                        // would then be orphaned as an unreconcilable pending delta.
                        // This was the root cause of the "Unexpected length of input"
                        // stuck-sync observed when a KeyDelivery op's retry-apply
                        // path hit a pre-existing stored op that failed to decode.
                        // Each `?` site below tags the error with the failing
                        // step so the warn log at line ~158 names the exact
                        // call. Without this, "Unexpected length of input"
                        // is ambiguous between the identity read, the key
                        // store, or the retry walk.
                        let mut apply_kd = || -> EyreResult<Option<super::DivergenceReport>> {
                            if let Some(identity) =
                                get_namespace_identity_record(self.store, &ns_id).map_err(|e| {
                                    eyre::eyre!("get_namespace_identity_record: {e}")
                                })?
                            {
                                let recipient_sk = PrivateKey::from(identity.private_key);
                                if envelope.recipient == recipient_sk.public_key() {
                                    match unwrap_group_key(&recipient_sk, envelope) {
                                        Ok(group_key) => {
                                            let gid = ContextGroupId::from(*group_id);
                                            let key_id =
                                                store_group_key(self.store, &gid, &group_key)
                                                    .map_err(|e| {
                                                        eyre::eyre!("store_group_key: {e}")
                                                    })?;
                                            tracing::info!(
                                                group_id = %hex::encode(group_id),
                                                key_id = %hex::encode(key_id),
                                                "received group key via KeyDelivery"
                                            );
                                            // Wake any `join_group` future
                                            // waiting on the gossip-fallback
                                            // path. The apply path already
                                            // filtered by
                                            // `envelope.recipient ==
                                            // recipient_sk.public_key()`, so
                                            // this only fires for our own
                                            // identity. Emit *before*
                                            // `retry_encrypted_ops_for_group`
                                            // so the wake-up isn't blocked
                                            // by a slow retry pass.
                                            notify_op_event(OpEvent::GroupKeyDelivered {
                                                group_id: *group_id,
                                                recipient: recipient_sk.public_key(),
                                            });
                                            let retry_divergence = self
                                                .retry_encrypted_ops_for_group(*group_id)
                                                .map_err(|e| {
                                                    eyre::eyre!(
                                                        "retry_encrypted_ops_for_group: {e}"
                                                    )
                                                })?;
                                            return Ok(retry_divergence);
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                ?e,
                                                "failed to unwrap KeyDelivery envelope"
                                            );
                                            result.key_unwrap_failures.push(KeyUnwrapFailure {
                                                group_id: *group_id,
                                                reason: format!("KeyDelivery unwrap failed: {e}"),
                                            });
                                        }
                                    }
                                }
                            }
                            Ok(None)
                        };
                        match apply_kd() {
                            Ok(retry_divergence) => {
                                if retry_divergence.is_some() {
                                    // KeyDelivery itself never produces a
                                    // post-apply divergence — only the
                                    // retried encrypted ops can. Merge
                                    // their LWW report into the same
                                    // outbox slot the fresh-arrival path
                                    // uses so the node handler routes it
                                    // to `reconcile_after_divergence`.
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
                    RootOp::MemberJoined {
                        member,
                        ref signed_invitation,
                    } => {
                        let gid = signed_invitation.invitation.group_id;
                        let group_id_typed = ContextGroupId::from(gid);
                        if load_current_group_key_record(self.store, &group_id_typed)?.is_some() {
                            result.pending_deliveries.push(PendingKeyDelivery {
                                namespace_id: op.namespace_id,
                                group_id: group_id_typed.to_bytes(),
                                joiner_pk: *member,
                            });
                        }
                    }
                    RootOp::MemberJoinedOpen { member, group_id } => {
                        // Same delivery-trigger semantics as `MemberJoined` —
                        // any peer that holds the group key publishes a
                        // `KeyDelivery` wrapped for the joiner. Authority
                        // for this op is validated in `execute_member_joined_open`
                        // (we ran it via `apply_root_op` above before this
                        // match), so by the time we get here the path is
                        // confirmed Inherited.
                        let group_id_typed = ContextGroupId::from(*group_id);
                        if load_current_group_key_record(self.store, &group_id_typed)?.is_some() {
                            result.pending_deliveries.push(PendingKeyDelivery {
                                namespace_id: op.namespace_id,
                                group_id: group_id_typed.to_bytes(),
                                joiner_pk: *member,
                            });
                        }
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
                        clear_denied(self.store, &group_id_typed, member)?;
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
                        restore_member_context_identities(
                            self.store,
                            &group_id_typed,
                            member,
                        )?;
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
                let resolved_key = match load_group_key_by_id(self.store, &group_id_typed, key_id)?
                {
                    Some(k) => Some(k),
                    None => {
                        let ns_id_typed = ContextGroupId::from(self.namespace_id);
                        load_group_key_by_id(self.store, &ns_id_typed, key_id)?
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
                    if let Some(identity) = get_namespace_identity_record(self.store, &ns_id)? {
                        let recipient_sk = PrivateKey::from(identity.private_key);
                        for envelope in &rotation.envelopes {
                            if envelope.recipient == recipient_sk.public_key() {
                                match unwrap_group_key(&recipient_sk, envelope) {
                                    Ok(new_key) => {
                                        let _ =
                                            store_group_key(self.store, &group_id_typed, &new_key)?;
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
        // Phase-1 readiness gate runs FIRST — before any signing or local
        // DAG mutation. Apply-before-readiness leaks orphan ops on retry:
        // a rejected op stays in the local DAG, the retry signs a *new*
        // op (different op_hash), and we accumulate duplicate writes on
        // every attempt. Checking readiness up front makes the rejection
        // side-effect-free and cleanly retryable.
        let mesh = node_client
            .mesh_peer_count_for_namespace(self.namespace_id)
            .await;
        let known_at_gate = node_client.known_subscribers(&topic);
        assert_transport_ready(mesh, known_at_gate, node_client.gossipsub_mesh_n_low())
            .map_err(|e| eyre::eyre!(e))?;

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
            [0u8; 32],
            head.next_nonce,
            op,
        )?;

        self.apply_signed_op(&signed)?;

        // Notify the readiness FSM that we just advanced the local DAG on
        // the publisher path. Without this, `state_per_namespace` only
        // populates from gossipsub-receive deliveries — a node that
        // *publishes* an op never observes its own monotonic advance and
        // the FSM stays at `Bootstrapping` forever. See
        // `notify_namespace_op_applied` for the cross-crate plumbing.
        node_client.notify_namespace_op_applied(self.namespace_id);

        if observe_mesh {
            record_governance_publish_mesh_peers(op_kind, mesh);
        }

        // Refresh after `apply_signed_op`: if all peers departed since
        // the readiness gate, using the stale gate snapshot would turn
        // `NoPeersSubscribedToTopic` into `NoAckReceived` after the local
        // DAG has already advanced.
        let known_at_publish = node_client.known_subscribers(&topic);
        let min_acks = min_acks_after_local_mutation(known_at_gate, known_at_publish);

        let report = publish_and_await_ack_namespace(
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
        .map_err(|e| eyre::eyre!(e))?;
        tracing::debug!(
            op_kind,
            namespace_id = %hex::encode(self.namespace_id),
            acks = report.acked_by.len(),
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

        self.publish_post_gate(
            node_client,
            ack_router,
            signer_sk,
            op,
            topic,
            mesh,
            known,
            required_signers,
        )
        .await
    }

    /// Caller-gated variant of [`sign_and_publish_without_apply`]: assumes
    /// the caller already ran `assert_transport_ready` and is providing
    /// the `mesh` / `known_subscribers` snapshot it observed at gate-time.
    /// Used by [`GroupGovernancePublisher::sign_apply_and_publish_inner`]
    /// to avoid re-running the readiness gate after the local store has
    /// already been mutated — a second gate that flips between "Ready"
    /// at the outer call and "NotReady" here would leave the local op
    /// applied and the remote peers unaware (state divergence on retry).
    pub(crate) async fn sign_and_publish_post_gate(
        &self,
        node_client: &calimero_node_primitives::client::NodeClient,
        ack_router: &AckRouter,
        signer_sk: &PrivateKey,
        op: NamespaceOp,
        mesh: usize,
        known: usize,
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
        )
        .await
    }

    /// Shared body of [`sign_and_publish_without_apply`] and
    /// [`sign_and_publish_post_gate`]. Assumes the readiness gate has
    /// already been run by the caller; takes the gate-time `mesh`
    /// snapshot to feed the metric. The subscriber count is
    /// re-sampled at publish time (see below) so transient peer
    /// departures between the gate and the publish don't cause an
    /// `Err(NoAckReceived)` after the local store has already mutated.
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
    ) -> EyreResult<DeliveryReport> {
        let head = self.read_head_record()?;
        let observe_mesh = !matches!(op, NamespaceOp::Group { .. });
        let op_kind = op.op_kind_label();
        let op_timeout = timeout_for_namespace_op(&op);
        let signed = SignedNamespaceOp::sign(
            signer_sk,
            self.namespace_id,
            head.parent_hashes,
            [0u8; 32],
            head.next_nonce,
            op,
        )?;
        let delta_id = signed
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
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
        // immediately before deciding `min_acks`. The gate-time
        // snapshot (`known_at_gate`) was taken many awaits ago; in
        // group-publish flows it predates `sign_apply_local_group_op_borsh`
        // and the per-removal key mint. If a peer unsubscribed in the
        // meantime, sticking with the stale count would leave
        // `min_acks = 1` against an empty subscriber set and force
        // `NoAckReceived` after the local DAG has already advanced —
        // exactly the orphan-op-on-retry pattern the readiness gate
        // exists to prevent. `known_subscribers` is a cheap synchronous
        // DashMap lookup on `NodeClient` (no actor mailbox round-trip),
        // so re-sampling here costs effectively nothing while making
        // the solo-namespace fast-path responsive to live state.
        let known_at_publish = node_client.known_subscribers(&topic);
        let min_acks = min_acks_after_local_mutation(known_at_gate, known_at_publish);

        let report = publish_and_await_ack_namespace(
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
        .map_err(|e| eyre::eyre!(e))?;
        tracing::debug!(
            op_kind,
            namespace_id = %hex::encode(self.namespace_id),
            acks = report.acked_by.len(),
            elapsed_ms = report.elapsed_ms,
            op_hash = %hex::encode(report.op_hash),
            "namespace governance op published (no local apply)"
        );
        Ok(report)
    }

    fn retry_encrypted_ops_for_group(
        &self,
        group_id: [u8; 32],
    ) -> EyreResult<Option<super::DivergenceReport>> {
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
        let mut retry_divergence: Option<super::DivergenceReport> = None;

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
                        ?e,
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

    fn decrypt_and_apply_group_op(
        &self,
        ns_op: &SignedNamespaceOp,
        group_id: &ContextGroupId,
        group_key: &[u8; 32],
        encrypted: &EncryptedGroupOp,
    ) -> EyreResult<Option<super::DivergenceReport>> {
        let inner_op = decrypt_group_op(group_key, encrypted)?;

        let signed_group_op = SignedGroupOp {
            version: calimero_context_client::local_governance::SIGNED_GROUP_OP_SCHEMA_VERSION,
            group_id: group_id.to_bytes(),
            parent_op_hashes: ns_op.parent_op_hashes.clone(),
            state_hash: ns_op.state_hash,
            signer: ns_op.signer,
            nonce: ns_op.nonce,
            op: inner_op,
            signature: ns_op.signature,
        };

        self.apply_group_op_inner(group_id, &ns_op.signer, ns_op.nonce, &signed_group_op.op)
    }

    fn apply_group_op_inner(
        &self,
        group_id: &ContextGroupId,
        signer: &PublicKey,
        nonce: u64,
        op: &GroupOp,
    ) -> EyreResult<Option<super::DivergenceReport>> {
        let last = get_local_gov_nonce(self.store, group_id, signer)?.unwrap_or(0);
        if nonce <= last {
            tracing::debug!(
                nonce,
                last_nonce = last,
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
                        String::new().into_boxed_str(),
                        String::new().into_boxed_str(),
                        String::new().into_boxed_str(),
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

        let (handled, divergence) = apply_group_op_mutations(self.store, group_id, signer, op)?;
        if !handled {
            tracing::debug!(
                ?op,
                "namespace group op variant not handled by inner apply, stored as skeleton"
            );
        }

        set_local_gov_nonce(self.store, group_id, signer, nonce)?;
        Ok(divergence)
    }

    fn require_namespace_admin(&self, signer: &PublicKey) -> EyreResult<()> {
        let ns_gid = ContextGroupId::from(self.namespace_id);
        if !is_group_admin(self.store, &ns_gid, signer)? {
            bail!(
                "signer {} is not an admin of namespace {}",
                signer,
                hex::encode(self.namespace_id)
            );
        }
        Ok(())
    }

    fn apply_root_op(&self, op: &SignedNamespaceOp, root: &RootOp) -> EyreResult<()> {
        match root {
            RootOp::GroupCreated {
                group_id,
                parent_id,
            } => self.execute_group_created(op, *group_id, *parent_id),
            RootOp::GroupDeleted {
                root_group_id,
                cascade_group_ids,
                cascade_context_ids,
            } => self.execute_group_deleted(
                op,
                *root_group_id,
                cascade_group_ids,
                cascade_context_ids,
            ),
            RootOp::GroupReparented {
                child_group_id,
                new_parent_id,
            } => self.execute_group_reparented(op, *child_group_id, *new_parent_id),
            RootOp::AdminChanged { new_admin } => self.execute_admin_changed(op, *new_admin),
            RootOp::PolicyUpdated { .. } => self.execute_policy_updated(op),
            RootOp::MemberJoined {
                member,
                signed_invitation,
            } => self.execute_member_joined(op, member, signed_invitation),
            RootOp::MemberJoinedOpen { member, group_id } => {
                self.execute_member_joined_open(op, *member, *group_id)
            }
            RootOp::KeyDelivery { .. } => Ok(()),
        }
    }

    fn execute_group_created(
        &self,
        op: &SignedNamespaceOp,
        group_id: [u8; 32],
        parent_id: [u8; 32],
    ) -> EyreResult<()> {
        let gid = ContextGroupId::from(group_id);
        let parent_gid = ContextGroupId::from(parent_id);

        // Namespace roots are created via a different path (local meta +
        // identity writes, no GroupCreated op); GroupCreated itself is only
        // for subgroups. Reject self-parent to make that invariant explicit
        // — a self-parent edge would cause resolve_namespace to cycle.
        if group_id == parent_id {
            eyre::bail!(
                "GroupCreated rejected: self-parent edge (group_id == parent_id). \
                 Namespace roots must not emit GroupCreated — their existence is \
                 recorded by the namespace-identity setup path."
            );
        }

        // Authorization. Namespace-root admins may create a subgroup at any
        // depth (matches `require_namespace_admin`). A non-admin namespace
        // member may create one *directly under the namespace root* if they
        // hold `CAN_CREATE_SUBGROUP` — that bit is honored only at root level
        // because every peer applying this op must be able to verify the
        // creator's authority, and only the root group's capability rows are
        // readable by all namespace members (see the capability's doc).
        let ns_gid = ContextGroupId::from(self.namespace_id);
        let authorized = is_group_admin(self.store, &ns_gid, &op.signer)?
            || (parent_id == self.namespace_id
                && is_group_admin_or_has_capability(
                    self.store,
                    &ns_gid,
                    &op.signer,
                    calimero_context_config::MemberCapabilities::CAN_CREATE_SUBGROUP,
                )?);
        if !authorized {
            bail!(
                "GroupCreated rejected: signer {} is neither an admin of namespace {} \
                 nor a member holding CAN_CREATE_SUBGROUP at the namespace root",
                op.signer,
                hex::encode(self.namespace_id)
            );
        }

        // Verify parent exists in this namespace (root or previously-created subgroup).
        let parent_meta = load_group_meta(self.store, &parent_gid)?.ok_or_else(|| {
            eyre::eyre!("GroupCreated rejected: parent_id '{parent_gid:?}' not found in namespace")
        })?;

        // The originating node's `create_group` handler pre-populates
        // `GroupMeta` (and related state) BEFORE publishing this op, so a
        // naive "if meta exists, return early" idempotency check would
        // short-circuit on the originator's local apply, leaving the group
        // without `GroupParentRef` / `GroupChildIndex` edges. Remote peers
        // applying a fresh op would write edges correctly, causing silent
        // divergence between originator and peers (resolve_namespace,
        // list_child_groups, and reparent would all fail on the originator).
        //
        // Fix: only skip the meta write if it already exists, but ALWAYS
        // ensure parent edge + child index + admin membership are present.
        // These are idempotent puts — a second apply is a no-op with
        // identical effect, so true replay is still safe.
        let meta_existed = load_group_meta(self.store, &gid)?.is_some();
        if !meta_existed {
            // Inherit application ID from the immediate parent (matches
            // mero-drive folder mental model: a subfolder runs the same app
            // as its parent).
            let meta = calimero_store::key::GroupMetaValue {
                admin_identity: op.signer,
                owner_identity: op.signer,
                target_application_id: parent_meta.target_application_id,
                app_key: [0u8; 32],
                upgrade_policy: calimero_primitives::context::UpgradePolicy::default(),
                migration: None,
                created_at: 0,
                auto_join: false,
            };
            save_group_meta(self.store, &gid, &meta)?;
        } else {
            tracing::debug!(
                group_id = %hex::encode(group_id),
                "GroupCreated: meta already present (pre-populated by handler or replay); \
                 skipping meta write but still ensuring parent edge + admin membership"
            );
        }

        // Ordered writes — NOT a single RocksDB atomic batch. Each call
        // below opens its own store handle (save_group_meta above, this put
        // pair, add_group_member below). A crash between any two steps leaves
        // partial state. Recovery path: re-applying the same GroupCreated op
        // is idempotent (meta-exists check skips the meta write; edge writes
        // are idempotent puts; add_group_member is an upsert) — so retries
        // complete whatever was missing. True single-batch atomicity would
        // require threading one store handle through this flow, which
        // matches a codebase-wide architectural decision deferred to a
        // follow-up (see the cascade delete atomicity discussion).
        {
            use calimero_store::key::{GroupChildIndex, GroupParentRef};
            let mut handle = self.store.handle();
            handle.put(&GroupParentRef::new(group_id), &parent_id)?;
            handle.put(&GroupChildIndex::new(parent_id, group_id), &())?;
        }
        add_group_member(self.store, &gid, &op.signer, GroupMemberRole::Admin)?;

        notify_op_event(OpEvent::SubgroupCreated {
            namespace_id: self.namespace_id,
            parent_group_id: parent_id,
            child_group_id: group_id,
        });
        Ok(())
    }

    fn execute_group_reparented(
        &self,
        op: &SignedNamespaceOp,
        child_group_id: [u8; 32],
        new_parent_id: [u8; 32],
    ) -> EyreResult<()> {
        self.require_namespace_admin(&op.signer)?;
        let child = ContextGroupId::from(child_group_id);
        let new_parent = ContextGroupId::from(new_parent_id);
        match super::reparent_group(self.store, &child, &new_parent)? {
            super::ReparentOutcome::Reparented { old_parent } => {
                notify_op_event(OpEvent::SubgroupReparented {
                    namespace_id: self.namespace_id,
                    old_parent_group_id: old_parent.to_bytes(),
                    new_parent_group_id: new_parent_id,
                    child_group_id,
                });
            }
            // Idempotent no-op (new_parent == old_parent). Don't fire an
            // event — downstream subscribers would see a "reparent" with
            // identical old/new parents, falsely implying a structural
            // change occurred.
            super::ReparentOutcome::Unchanged => {}
        }
        Ok(())
    }

    fn execute_group_deleted(
        &self,
        op: &SignedNamespaceOp,
        root_group_id: [u8; 32],
        cascade_group_ids: &[[u8; 32]],
        cascade_context_ids: &[[u8; 32]],
    ) -> EyreResult<()> {
        let root_gid = ContextGroupId::from(root_group_id);
        if root_group_id == self.namespace_id {
            eyre::bail!(
                "cannot delete the namespace root '{root_gid:?}' (use delete_namespace instead)"
            );
        }

        // Authorization. Cascade-delete is allowed for: the owner of the
        // subgroup being deleted; an admin of the namespace root (moderation);
        // or a namespace member holding `CAN_DELETE_SUBGROUP` (an explicit
        // delegation). All three are deterministically verifiable on every
        // peer applying this op — `GroupDeleted` is cleartext, and the
        // deleting peer holds the root group's meta on the *first* apply. The
        // non-owner case routes through `PermissionChecker` to match the local
        // `delete_group` handler.
        //
        // The owner branch checks only `owner_identity == op.signer`, not
        // current namespace membership — `owner_identity` is a persistent
        // record from group creation, and matching it *is* being the owner
        // (32-byte keys don't "happen to collide"). In practice the owner is
        // always a current namespace member anyway: `leave_namespace` /
        // `leave_group` reject an owner with `MustTransferOwnership`, so you
        // can't leave while owning a subgroup in the subtree.
        //
        // Crash-recovery: the cascade below tears the root group's meta down
        // *last* (after every descendant), and only then does `apply_signed_op`
        // advance the DAG head. If the process dies in between, the re-apply
        // finds the root meta already gone — the op was authorized on the
        // first pass, so we skip the auth check here and let the (idempotent)
        // cascade finish any remaining cleanup. Without this, an owner who is
        // not also a namespace admin / `CAN_DELETE_SUBGROUP` holder would
        // permanently stall the DAG. (The pre-existing `require_namespace_admin`
        // check was immune because the namespace root is never part of a
        // cascade.) A `GroupDeleted` for a group that never existed locally
        // likewise reads `None` here and is a harmless no-op below.
        let ns_gid = ContextGroupId::from(self.namespace_id);
        if let Some(root_meta) = load_group_meta(self.store, &root_gid)? {
            if root_meta.owner_identity != op.signer {
                PermissionChecker::new(self.store, ns_gid)
                    .require_can_delete_subgroup(&op.signer)
                    .map_err(|e| {
                        eyre::eyre!(
                            "GroupDeleted rejected: {e} (or be the owner of subgroup {})",
                            hex::encode(root_group_id)
                        )
                    })?;
            }
        }

        // Determinism check: every surviving element of the local subtree MUST
        // be in the op's payload. We use subset rather than exact equality
        // because a previous apply attempt may have crashed mid-cascade,
        // leaving the local subtree as a partial-delete state. In that case:
        // - every still-present descendant is in payload (subset holds) ✓
        // - exact match would fail because the local count is smaller, making
        //   the op permanently un-applyable and stalling the namespace DAG
        //
        // Subset still catches true divergence: if the local subtree contains
        // a group NOT in payload, the check fails (correct rejection).
        // Contexts are always set-compared (order-insensitive) with the same
        // subset rule.
        let local_payload = super::collect_subtree_for_cascade(self.store, &root_gid)?;
        let local_groups: std::collections::BTreeSet<[u8; 32]> = local_payload
            .descendant_groups
            .iter()
            .map(|g| g.to_bytes())
            .collect();
        let local_contexts: std::collections::BTreeSet<[u8; 32]> =
            local_payload.contexts.iter().map(|c| **c).collect();
        let payload_groups: std::collections::BTreeSet<[u8; 32]> =
            cascade_group_ids.iter().copied().collect();
        let payload_contexts: std::collections::BTreeSet<[u8; 32]> =
            cascade_context_ids.iter().copied().collect();
        if !local_groups.is_subset(&payload_groups) {
            let extra: Vec<_> = local_groups.difference(&payload_groups).collect();
            eyre::bail!(
                "GroupDeleted cascade divergence: local subtree has groups not in payload: {extra:?}"
            );
        }
        if !local_contexts.is_subset(&payload_contexts) {
            let extra: Vec<_> = local_contexts.difference(&payload_contexts).collect();
            eyre::bail!(
                "GroupDeleted cascade divergence: local subtree has contexts not in payload: {extra:?}"
            );
        }
        // Inverse direction is *not* an error — it's the expected shape on a
        // crash-recovery re-apply (the local subtree shrank since the op was
        // built) — but log it so a genuinely anomalous payload (a publisher
        // listing IDs this peer has never seen) is visible for debugging.
        let payload_only_groups: Vec<[u8; 32]> =
            payload_groups.difference(&local_groups).copied().collect();
        if !payload_only_groups.is_empty() {
            tracing::warn!(
                root_group_id = %hex::encode(root_group_id),
                groups = ?payload_only_groups.iter().map(hex::encode).collect::<Vec<_>>(),
                "GroupDeleted payload lists groups not present locally (expected on a \
                 crash-recovery re-apply; otherwise investigate for divergence)"
            );
        }
        let payload_only_contexts: Vec<[u8; 32]> = payload_contexts
            .difference(&local_contexts)
            .copied()
            .collect();
        if !payload_only_contexts.is_empty() {
            tracing::warn!(
                root_group_id = %hex::encode(root_group_id),
                contexts = ?payload_only_contexts.iter().map(hex::encode).collect::<Vec<_>>(),
                "GroupDeleted payload lists contexts not present locally (expected on a \
                 crash-recovery re-apply; otherwise investigate for divergence)"
            );
        }

        // Children-first deletion: descendants then root. For each group:
        // 1. Delete contexts registered on this group (cascade-specific).
        // 2. Call delete_group_local_rows for the comprehensive per-group
        //    cleanup (members, signing keys, capabilities, member aliases,
        //    default capabilities/visibility, group alias, context migrations,
        //    upgrade record, op-log + head, meta, governance nonces, and
        //    member-context joins) — single source of truth shared with the
        //    non-cascade GroupOp::GroupDelete path.
        // 3. Remove the parent edge + child-index entry on the parent.
        let all_groups_iter = cascade_group_ids
            .iter()
            .copied()
            .chain(std::iter::once(root_group_id));
        for gid_bytes in all_groups_iter {
            let gid = ContextGroupId::from(gid_bytes);
            for ctx in super::enumerate_group_contexts(self.store, &gid, 0, usize::MAX)? {
                super::unregister_context_from_group(self.store, &gid, &ctx)?;
            }
            // Capture parent before delete_group_local_rows runs (it deletes
            // GroupMeta but leaves parent edges; we still need them to clean
            // up the child-index entry on the parent below).
            let parent_for_cleanup = super::get_parent_group(self.store, &gid)?;
            super::delete_group_local_rows(self.store, &gid)?;
            if let Some(parent) = parent_for_cleanup {
                let mut handle = self.store.handle();
                handle.delete(&calimero_store::key::GroupParentRef::new(gid_bytes))?;
                handle.delete(&calimero_store::key::GroupChildIndex::new(
                    parent.to_bytes(),
                    gid_bytes,
                ))?;
            }
        }

        tracing::info!(
            ?root_gid,
            deleted_groups = cascade_group_ids.len() + 1,
            deleted_contexts = cascade_context_ids.len(),
            "cascade-deleted group subtree"
        );
        Ok(())
    }

    fn execute_admin_changed(
        &self,
        op: &SignedNamespaceOp,
        new_admin: PublicKey,
    ) -> EyreResult<()> {
        self.require_namespace_admin(&op.signer)?;
        let ns_gid = ContextGroupId::from(self.namespace_id);
        let mut meta = load_group_meta(self.store, &ns_gid)?
            .ok_or_else(|| eyre::eyre!("namespace root group not found"))?;
        meta.admin_identity = new_admin;
        save_group_meta(self.store, &ns_gid, &meta)?;
        Ok(())
    }

    fn execute_policy_updated(&self, op: &SignedNamespaceOp) -> EyreResult<()> {
        self.require_namespace_admin(&op.signer)?;
        tracing::debug!("PolicyUpdated: stored in DAG log, no additional state mutation");
        Ok(())
    }

    fn execute_member_joined(
        &self,
        op: &SignedNamespaceOp,
        member: &PublicKey,
        signed_invitation: &calimero_context_config::types::SignedGroupOpenInvitation,
    ) -> EyreResult<()> {
        NamespaceMembershipService::new(self.store, self.namespace_id).apply_member_joined(
            &op.signer,
            member,
            signed_invitation,
        )
    }

    /// Apply check for `RootOp::MemberJoinedOpen`. The op is cleartext,
    /// the outer `SignedNamespaceOp.signer` MUST equal `member` (proves
    /// key ownership), and `member` MUST have an Inherited membership
    /// path to `group_id` — i.e. the subgroup is `Open` and they hold
    /// `CAN_JOIN_OPEN_SUBGROUPS` at the namespace root (the same
    /// check `join_context.rs` runs locally before letting the joiner
    /// proceed). We don't mutate state here — the side-effect (pushing
    /// a `PendingKeyDelivery` if we hold the key) happens in the outer
    /// `apply_signed_op` match.
    fn execute_member_joined_open(
        &self,
        op: &SignedNamespaceOp,
        member: PublicKey,
        group_id: [u8; 32],
    ) -> EyreResult<()> {
        if op.signer != member {
            eyre::bail!(
                "MemberJoinedOpen rejected: outer signer {} doesn't match member {}",
                op.signer,
                member
            );
        }
        let gid = ContextGroupId::from(group_id);
        // Cross-namespace forgery guard: without this check, an attacker
        // on namespace A could publish a MemberJoinedOpen naming a
        // `group_id` from namespace B; `check_group_membership_path`
        // walks parents up to whichever namespace root owns `gid`, so
        // the path check below could succeed against B's data when this
        // op is being applied in namespace A. Pin `gid` to this
        // namespace — matches the implicit assumption in the sibling
        // `MemberJoined` apply path.
        let resolved_ns = super::resolve_namespace(self.store, &gid)?;
        if resolved_ns.to_bytes() != self.namespace_id {
            eyre::bail!(
                "MemberJoinedOpen rejected: group_id {:?} resolves to namespace {:?}, \
                 not this namespace {:?}",
                gid,
                resolved_ns,
                ContextGroupId::from(self.namespace_id)
            );
        }
        match super::check_group_membership_path(self.store, &gid, &member)? {
            super::MembershipPath::Inherited { .. } => Ok(()),
            super::MembershipPath::Direct => {
                // Direct members go through `MemberJoined` or `add_group_members`
                // — they shouldn't be using this op.
                eyre::bail!(
                    "MemberJoinedOpen rejected: signer {} is a direct member of {:?}; \
                     use MemberJoined or add_group_members instead",
                    member,
                    gid
                );
            }
            super::MembershipPath::None => {
                eyre::bail!(
                    "MemberJoinedOpen rejected: signer {} has no membership path to {:?}",
                    member,
                    gid
                );
            }
        }
    }
}

pub fn apply_signed_namespace_op(
    store: &Store,
    op: &SignedNamespaceOp,
) -> EyreResult<ApplyNamespaceOpResult> {
    NamespaceGovernance::new(store, op.namespace_id).apply_signed_op(op)
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
