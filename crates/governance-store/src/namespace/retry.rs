use super::op_log::NamespaceOpLogService;
use crate::GroupKeyring;
use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_store::Store;
use eyre::Result as EyreResult;

/// A namespace group operation that can be retried locally because the
/// corresponding group key is now available.
pub struct RetryCandidate {
    pub signed_op: SignedNamespaceOp,
    pub group_key: [u8; 32],
}

/// Service for retrying deferred encrypted group operations after key delivery.
pub struct NamespaceRetryService<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
}

impl<'a> NamespaceRetryService<'a> {
    pub fn new(store: &'a Store, namespace_id: [u8; 32]) -> Self {
        Self {
            store,
            namespace_id,
        }
    }

    /// Distinct group ids that have at least one buffered encrypted op the
    /// local node cannot yet decrypt — decided **per op `key_id`**, not by
    /// whether the node holds *some* key for the group.
    ///
    /// The distinction is load-bearing: a node can hold the namespace
    /// (root) key — delivered with its join — yet still lack a
    /// **Restricted** subgroup's own key. Such a subgroup's ops are
    /// encrypted under the subgroup key, so the node must still pull it.
    /// Mirroring the apply path (which resolves `key_id` against the
    /// subgroup keyring then falls back to the namespace keyring for the
    /// `Open` case), a group is awaiting iff some buffered op's `key_id`
    /// resolves to no key in either keyring. Driving off
    /// buffered-and-undecryptable ops means a group with nothing pending
    /// is never requested, so the set is naturally self-limiting.
    pub fn groups_awaiting_key(&self) -> EyreResult<Vec<[u8; 32]>> {
        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);
        let op_keys = op_log
            .collect_buffered_group_op_keys()
            .map_err(|e| eyre::eyre!("op_log.collect_buffered_group_op_keys: {e}"))?;
        let ns_typed = ContextGroupId::from(self.namespace_id);

        let mut awaiting = std::collections::BTreeSet::new();
        for (group_id, key_id) in op_keys {
            let gid_typed = ContextGroupId::from(group_id);
            // Same resolution order as `apply_signed_op`'s Group arm:
            // the subgroup's own keyring first (Restricted), then the
            // namespace keyring (Open subgroups are encrypted under it).
            let resolvable = GroupKeyring::new(self.store, gid_typed)
                .load_key_by_id(&key_id)
                .map_err(|e| eyre::eyre!("load_key_by_id(group): {e}"))?
                .is_some()
                || GroupKeyring::new(self.store, ns_typed)
                    .load_key_by_id(&key_id)
                    .map_err(|e| eyre::eyre!("load_key_by_id(namespace): {e}"))?
                    .is_some();
            if !resolvable {
                awaiting.insert(group_id);
            }
        }
        Ok(awaiting.into_iter().collect())
    }

    /// Distinct group ids that have at least one buffered encrypted op whose
    /// `key_id` the local node CAN already resolve — the exact INVERSE of
    /// [`groups_awaiting_key`](Self::groups_awaiting_key)'s filter.
    ///
    /// This is the #2848 Part C curative-sweep enumerator: a node stranded
    /// before the live re-drive landed holds the key (the `KeyDelivery`
    /// arrived after `GroupCreated` long applied) yet still has buffered ops
    /// that were effect-skipped because no future trigger re-drives them.
    /// This returns exactly those groups so the sweep can re-drive them.
    ///
    /// Resolution mirrors the apply path (subgroup keyring first for
    /// `Restricted`, then the namespace keyring for `Open`), identical to
    /// `groups_awaiting_key` — so a group whose key is genuinely held is
    /// returned, and a group still awaiting its key is NOT. The held-key
    /// filter is ALSO the deleted-group exit: a purged group has no key in
    /// either keyring, so it never appears here (and re-driving it would be a
    /// no-op regardless).
    ///
    /// Driving off buffered-and-decryptable ops means a group with nothing
    /// pending is never returned, so the set is naturally self-limiting.
    pub fn groups_with_held_key_buffered_ops(&self) -> EyreResult<Vec<[u8; 32]>> {
        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);
        let op_keys = op_log
            .collect_buffered_group_op_keys()
            .map_err(|e| eyre::eyre!("op_log.collect_buffered_group_op_keys: {e}"))?;
        let ns_typed = ContextGroupId::from(self.namespace_id);

        let mut held = std::collections::BTreeSet::new();
        for (group_id, key_id) in op_keys {
            let gid_typed = ContextGroupId::from(group_id);
            // Same resolution order as `apply_signed_op`'s Group arm and
            // `groups_awaiting_key`: subgroup keyring first (Restricted),
            // then the namespace keyring (Open subgroups encrypt under it).
            let resolvable = GroupKeyring::new(self.store, gid_typed)
                .load_key_by_id(&key_id)
                .map_err(|e| eyre::eyre!("load_key_by_id(group): {e}"))?
                .is_some()
                || GroupKeyring::new(self.store, ns_typed)
                    .load_key_by_id(&key_id)
                    .map_err(|e| eyre::eyre!("load_key_by_id(namespace): {e}"))?
                    .is_some();
            if resolvable {
                held.insert(group_id);
            }
        }
        Ok(held.into_iter().collect())
    }

    pub fn collect_retry_candidates_for_group(
        &self,
        group_id: [u8; 32],
    ) -> EyreResult<Vec<RetryCandidate>> {
        let mut candidates = Vec::new();
        let gid_typed = ContextGroupId::from(group_id);
        let ns_typed = ContextGroupId::from(self.namespace_id);
        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);
        let entries = op_log
            .collect_signed_group_ops_for_group(group_id)
            .map_err(|e| eyre::eyre!("op_log.collect_signed_group_ops_for_group: {e}"))?;
        for entry in entries {
            let NamespaceOp::Group { key_id, .. } = entry.signed_op.op else {
                continue;
            };
            // Issue #2256: same fallback as the live-apply path — the op
            // may have been encrypted with the namespace key if the
            // subgroup was `Open` at publish time.
            let group_key = match GroupKeyring::new(self.store, gid_typed)
                .load_key_by_id(&key_id)
                .map_err(|e| eyre::eyre!("load_group_key_by_id(group): {e}"))?
            {
                Some(k) => k,
                None => {
                    let Some(k) = GroupKeyring::new(self.store, ns_typed)
                        .load_key_by_id(&key_id)
                        .map_err(|e| eyre::eyre!("load_group_key_by_id(namespace): {e}"))?
                    else {
                        continue;
                    };
                    k
                }
            };
            let signed_op: SignedNamespaceOp = entry.signed_op;
            candidates.push(RetryCandidate {
                signed_op,
                group_key,
            });
        }

        // Sort by (signer_bytes, nonce) ascending so the apply order
        // matches publish order *per signer*. Without this sort,
        // candidates come back in column-iteration order (sorted by
        // `delta_id`, which is essentially a content hash) — when a
        // higher-nonce op applies first, `apply_group_op_inner`
        // advances the per-(group, signer) `last_nonce`, then
        // incorrectly treats subsequent legitimate lower-nonce ops
        // from the same signer as duplicates and skips them. That
        // permanently loses earlier ops in the sequence (e.g. a
        // `ContextRegistered` published before a later `MemberAdded`
        // from the same admin), leaving a downstream
        // `ContextMetadataSet` to bail at the "context not registered
        // in this group" precondition.
        //
        // Note on multi-signer ordering: this sort groups ops by
        // signer-public-key lexicographically, then by nonce within
        // each signer. Cross-signer interleaving (signer A nonce 1 →
        // signer B nonce 1 → signer A nonce 2) is NOT preserved — all
        // of signer A's ops apply first, then all of signer B's. This
        // is safe for correctness because `last_nonce` is tracked
        // per-(group, signer), so each signer's nonce check is
        // independent. Cross-signer causal ordering, where it
        // matters, is enforced separately by `parent_op_hashes` on
        // the namespace DAG at the time ops are received — the retry
        // path here is just replaying ops that were already
        // DAG-validated before being buffered awaiting `KeyDelivery`.
        candidates.sort_by_key(|c| {
            let signer_bytes: &[u8; 32] = c.signed_op.signer.as_ref();
            (*signer_bytes, c.signed_op.nonce)
        });

        Ok(candidates)
    }
}
