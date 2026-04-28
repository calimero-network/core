use calimero_context_client::local_governance::{AckRouter, GroupOp, NamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::Result as EyreResult;
use rand::{rngs::OsRng, Rng};

use super::{
    build_key_rotation, encrypt_group_op, get_namespace_identity_record, get_parent_group,
    is_open_chain_to_namespace, load_current_group_key_record, resolve_namespace,
    sign_apply_local_group_op_borsh, store_group_key, NamespaceGovernance,
};
use crate::governance_broadcast::{assert_transport_ready, ns_topic, DeliveryReport};
use crate::metrics::record_governance_publish_mesh_peers;

/// Orchestrates local apply + encrypted namespace publish for group governance ops.
pub struct GroupGovernancePublisher<'a> {
    store: &'a Store,
    node_client: &'a calimero_node_primitives::client::NodeClient,
    group_id: ContextGroupId,
}

impl<'a> GroupGovernancePublisher<'a> {
    pub fn new(
        store: &'a Store,
        node_client: &'a calimero_node_primitives::client::NodeClient,
        group_id: ContextGroupId,
    ) -> Self {
        Self {
            store,
            node_client,
            group_id,
        }
    }

    /// `Ok(Some(report))` is a published-and-acked outcome.
    /// `Ok(None)` is a deliberate skip: this node is not yet a namespace
    /// member (no identity record) or has no group key for the
    /// encrypting group — the local apply still happened, but there is
    /// nothing to publish on the wire.
    pub async fn sign_apply_and_publish(
        &self,
        ack_router: &AckRouter,
        signer_sk: &PrivateKey,
        op: GroupOp,
    ) -> EyreResult<Option<DeliveryReport>> {
        self.sign_apply_and_publish_inner(ack_router, signer_sk, op, None)
            .await
    }

    /// See [`sign_apply_and_publish`](Self::sign_apply_and_publish) for
    /// the meaning of `Ok(None)`.
    pub async fn sign_apply_and_publish_removal(
        &self,
        ack_router: &AckRouter,
        signer_sk: &PrivateKey,
        removed_member: &PublicKey,
    ) -> EyreResult<Option<DeliveryReport>> {
        self.sign_apply_and_publish_inner(
            ack_router,
            signer_sk,
            GroupOp::MemberRemoved {
                member: *removed_member,
            },
            Some(removed_member),
        )
        .await
    }

    async fn sign_apply_and_publish_inner(
        &self,
        ack_router: &AckRouter,
        signer_sk: &PrivateKey,
        op: GroupOp,
        removed_member: Option<&PublicKey>,
    ) -> EyreResult<Option<DeliveryReport>> {
        // Phase-1 readiness gate runs FIRST — before
        // `sign_apply_local_group_op_borsh` (which mutates the local group
        // store) and before the per-removal key-mint+store below. The
        // namespace-level helper invoked at the bottom of this function
        // ALSO runs the gate, but a `NamespaceNotReady` rejection there
        // would arrive *after* local mutation, leaving an orphan group-row
        // and (for member-removal) a freshly-minted-but-unused key on
        // every retry. Gating up front keeps the rejection side-effect-free
        // and cleanly retryable, mirroring `NamespaceGovernance::sign_apply_and_publish`.
        let namespace_id = resolve_namespace(self.store, &self.group_id)?;
        let namespace_bytes = namespace_id.to_bytes();
        let topic = ns_topic(namespace_bytes);
        let mesh = self
            .node_client
            .mesh_peer_count_for_namespace(namespace_bytes)
            .await;
        let known = self.node_client.known_subscribers(&topic);
        assert_transport_ready(mesh, known, self.node_client.gossipsub_mesh_n_low())
            .map_err(|e| eyre::eyre!(e))?;

        let _output =
            sign_apply_local_group_op_borsh(self.store, &self.group_id, signer_sk, op.clone())?;

        let Some(namespace_identity) = get_namespace_identity_record(self.store, &namespace_id)?
        else {
            tracing::debug!(
                group_id = %hex::encode(self.group_id.to_bytes()),
                "no namespace identity, skipping namespace publish"
            );
            return Ok(None);
        };

        // Issue #2256: an `Open` subgroup whose entire ancestor chain up
        // to the namespace is also `Open` is by definition readable by
        // every member of its parent namespace, so we encrypt its ops
        // with the *namespace* key rather than the subgroup's own key.
        // That makes the crypto boundary match the access boundary,
        // eliminates the need for a separate key-delivery path to
        // inheritance-eligible parent members, and makes "Open" mean
        // what it says cryptographically.
        //
        // **Chain check is required, not just immediate visibility:** if
        // any ancestor between this subgroup and the namespace is
        // `Restricted`, the membership walk in
        // `check_group_membership_path` correctly refuses inheritance
        // through that wall — so encrypting with the namespace key
        // would expose this subgroup's content to namespace members
        // who cannot actually join it. `is_open_chain_to_namespace`
        // verifies the whole chain is Open before we widen the crypto
        // boundary.
        //
        // Restricted subgroups (or any subgroup behind a Restricted
        // ancestor) keep their per-subgroup key, unchanged.
        //
        // **Visibility-flip ops are special-cased:**
        // `sign_apply_local_group_op_borsh` above has *already* applied
        // the op locally, so the post-apply visibility of `self.group_id`
        // is the *new* mode. For ordinary ops (member add/remove,
        // capability set, etc.) that's exactly what we want: the new
        // state defines the access boundary the op should be encrypted
        // for. But for `SubgroupVisibilitySet`, post-apply state would
        // strand inheritance-eligible members on an `Open → Restricted`
        // flip — they hold only the namespace key, the post-apply check
        // selects the per-subgroup key, and they can never decrypt the
        // very op that says "you're no longer a member here."
        //
        // Resolution: for `SubgroupVisibilitySet`, decide the encryption
        // boundary from the **parent chain** (excluding `self.group_id`),
        // which is independent of the op being applied. The set of
        // members who *could observe* the flip is precisely the access
        // boundary that existed *before* the flip — i.e. the boundary
        // implied by the parent chain. This works symmetrically:
        // - `Open → Restricted` flip with fully-Open parent chain:
        //   namespace key. Every namespace member observes the wall
        //   going up.
        // - `Restricted → Open` flip with fully-Open parent chain:
        //   namespace key. Direct subgroup members try the subgroup
        //   keyring first (their subgroup is "still" Restricted from
        //   their PoV); the receiver-side namespace-keyring fallback
        //   handles the miss.
        // - Either flip behind a `Restricted` ancestor: per-subgroup
        //   key. The access boundary was never namespace-wide, so
        //   nobody outside the wall is owed visibility into the flip.
        // - Subgroup directly under the namespace (parent IS namespace):
        //   namespace key. The parent chain is trivially Open.
        let parent_chain_open = match &op {
            GroupOp::SubgroupVisibilitySet { .. } => {
                match get_parent_group(self.store, &self.group_id)? {
                    Some(parent) => {
                        parent == namespace_id
                            || is_open_chain_to_namespace(self.store, &parent, &namespace_id)?
                    }
                    None => false,
                }
            }
            _ => is_open_chain_to_namespace(self.store, &self.group_id, &namespace_id)?,
        };
        let encrypting_group_id = if parent_chain_open {
            namespace_id
        } else {
            self.group_id
        };

        let Some(stored_key) = load_current_group_key_record(self.store, &encrypting_group_id)?
        else {
            tracing::debug!(
                group_id = %hex::encode(self.group_id.to_bytes()),
                encrypting_group_id = %hex::encode(encrypting_group_id.to_bytes()),
                "no group key stored, skipping namespace publish"
            );
            return Ok(None);
        };

        let encrypted = encrypt_group_op(&stored_key.group_key, &op)?;

        // Key rotation on member-removal:
        //
        // - Restricted subgroup (`encrypting_group_id == self.group_id`):
        //   mint a new per-subgroup key, distribute it to remaining
        //   direct members via the rotation envelope, and revoke the
        //   removed member's decrypt access to subsequent ops. This is
        //   the standard forward-secrecy path.
        //
        // - Open subgroup (`encrypting_group_id == namespace_id`):
        //   **skip rotation**. The just-published op was encrypted
        //   with the *namespace* key, which the removed member still
        //   holds (their namespace membership is unaffected by a
        //   subgroup-member-removal), so a per-subgroup rotation
        //   would not actually revoke their read access — it would
        //   only mint a key that goes unused while the subgroup stays
        //   Open. This is the documented Option C trade-off (issue
        //   #2256): an Open subgroup's removal revokes *authorization*
        //   (the membership row goes away — the removed identity can
        //   no longer pass the membership walk for governance/write
        //   operations) but NOT cryptographic *read access* — that
        //   would require either rotating the namespace key (broad
        //   blast radius) or flipping the subgroup to Restricted
        //   (the deferred Open→Restricted lifecycle work, which
        //   itself will mint a fresh subgroup key at flip time).
        let key_rotation = if let Some(removed) = removed_member {
            if encrypting_group_id == self.group_id {
                let new_group_key: [u8; 32] = OsRng.gen();
                let _ = store_group_key(self.store, &self.group_id, &new_group_key)?;
                Some(build_key_rotation(
                    self.store,
                    &self.group_id,
                    &new_group_key,
                    signer_sk,
                    Some(removed),
                )?)
            } else {
                None
            }
        } else {
            None
        };

        let namespace_op = NamespaceOp::Group {
            group_id: self.group_id.to_bytes(),
            key_id: stored_key.key_id,
            encrypted,
            key_rotation,
        };

        // Stage-0 baseline: observe mesh-peer count *with the cleartext
        // `GroupOp` variant as the label* before the inner namespace publish
        // hides it inside an encrypted envelope. `NamespaceGovernance::sign_*`
        // skips emission for `NamespaceOp::Group { .. }` so this is the
        // single source of truth for group-op observations. Reuse the value
        // captured by the readiness gate at the top of the function — issuing
        // a second `mesh_peer_count_for_namespace` round-trip here would burn
        // an actor-mailbox hop per publish without observable benefit.
        record_governance_publish_mesh_peers(op.op_kind_label(), mesh);

        let namespace_sk = PrivateKey::from(namespace_identity.private_key);
        let report = NamespaceGovernance::new(self.store, namespace_bytes)
            .sign_and_publish_without_apply(
                self.node_client,
                ack_router,
                &namespace_sk,
                namespace_op,
            )
            .await?;
        Ok(Some(report))
    }
}
