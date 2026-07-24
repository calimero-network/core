use super::core::NamespaceRepository;
use super::op_log::NamespaceOpLogService;
use crate::{GroupKeyring, MembershipRepository};
use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
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
    namespace_id: NamespaceId,
}

impl<'a> NamespaceRetryService<'a> {
    pub fn new(store: &'a Store, namespace_id: NamespaceId) -> Self {
        Self {
            store,
            namespace_id,
        }
    }

    /// Does the node hold the key epoch `key_id` for `group_id`?
    ///
    /// Mirrors the apply path's resolution order — the group's own keyring
    /// first (a `Restricted` subgroup has its own key), then the namespace
    /// keyring (an `Open` subgroup is encrypted under it). Shared by every
    /// buffered-op enumerator (`groups_awaiting_key`, `awaited_group_keys`,
    /// `groups_with_held_key_buffered_ops`) so the fallback order lives in one
    /// place — a future fix to the resolution order changes only this method.
    fn holds_key_epoch(
        &self,
        group_id: ContextGroupId,
        ns_typed: ContextGroupId,
        key_id: &[u8; 32],
    ) -> EyreResult<bool> {
        Ok(GroupKeyring::new(self.store, group_id)
            .load_key_by_id(key_id)
            .map_err(|e| eyre::eyre!("load_key_by_id(group): {e}"))?
            .is_some()
            || GroupKeyring::new(self.store, ns_typed)
                .load_key_by_id(key_id)
                .map_err(|e| eyre::eyre!("load_key_by_id(namespace): {e}"))?
                .is_some())
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
        let ns_typed = ContextGroupId::from(self.namespace_id.to_bytes());

        let mut awaiting = std::collections::BTreeSet::new();
        for (group_id, key_id) in op_keys {
            let gid_typed = ContextGroupId::from(group_id);
            if !self.holds_key_epoch(gid_typed, ns_typed, &key_id)? {
                awaiting.insert(group_id);
            }
        }
        Ok(awaiting.into_iter().collect())
    }

    /// Distinct group ids in this namespace where the node holds a **direct
    /// membership row for its own namespace identity but no usable group key**
    /// — regardless of whether any op is buffered.
    ///
    /// This is the membership-driven counterpart to [`groups_awaiting_key`],
    /// which is purely op-driven (a group only appears once an undecryptable op
    /// is buffered for it). A node that joins under a thin/healing mesh records
    /// local membership but may fail to obtain the key, and if the namespace is
    /// then quiescent no encrypted op is ever buffered — so the op-driven set
    /// stays empty and the direct-pull recovery never fires. Enumerating
    /// member-but-keyless groups here lets that recovery re-acquire the key
    /// from the interval tick alone, with no buffered op and no manual re-join
    /// (#3295). The requester asks for the group's **current** key (`key_id`
    /// `None`), since with no op there is no specific epoch to target.
    ///
    /// "Keyless" mirrors [`groups_awaiting_key`]'s dual resolution: no current
    /// key in the group's own keyring AND none in the namespace keyring (an
    /// `Open` subgroup is decryptable under the namespace key). A `Restricted`
    /// subgroup whose member holds the namespace key but not the subgroup key
    /// is therefore treated as keyed here — but that case still surfaces
    /// through the op-driven set the moment one of its (subgroup-key-encrypted)
    /// ops is buffered, so it is not stranded.
    pub fn groups_member_but_keyless(&self) -> EyreResult<Vec<[u8; 32]>> {
        let ns_typed = ContextGroupId::from(self.namespace_id.to_bytes());

        // The member we'd be missing a key for is this node's namespace
        // identity. No identity ⇒ nothing to recover.
        let my_identity = match NamespaceRepository::new(self.store).identity_record(&ns_typed)? {
            Some(record) => record.public_key,
            None => return Ok(Vec::new()),
        };

        // The namespace (root) key decrypts the root group AND every `Open`
        // subgroup, so its presence alone means no group here is keyless.
        // Resolve it once rather than re-reading the namespace keyring for
        // every group in the loop.
        let has_namespace_key = GroupKeyring::new(self.store, ns_typed)
            .load_current_key()
            .map_err(|e| eyre::eyre!("load_current_key(namespace): {e}"))?
            .is_some();
        if has_namespace_key {
            return Ok(Vec::new());
        }

        // Every group in the namespace: the root plus all descendants.
        let mut groups = vec![ns_typed];
        groups.extend(NamespaceRepository::new(self.store).collect_descendants(&ns_typed)?);

        let mut out = std::collections::BTreeSet::new();
        for gid in groups {
            if !MembershipRepository::new(self.store).has_direct_member(&gid, &my_identity)? {
                continue;
            }
            // `has_namespace_key` is already known false here; only the group's
            // own keyring can still supply a key (a `Restricted` subgroup).
            let has_key = GroupKeyring::new(self.store, gid)
                .load_current_key()
                .map_err(|e| eyre::eyre!("load_current_key(group): {e}"))?
                .is_some();
            if !has_key {
                let _ = out.insert(gid.to_bytes());
            }
        }
        Ok(out.into_iter().collect())
    }

    /// Distinct `(group_id, key_id)` pairs the local node is buffering an
    /// undecryptable op for — the same set [`groups_awaiting_key`] collapses to
    /// group ids, but keeping the specific `key_id` each op needs. The
    /// direct-pull requester uses this to ask a peer for the EXACT key epoch a
    /// buffered op was encrypted under, instead of only the group's "current"
    /// key: after a rotation the op it's stranded on may be under an older
    /// epoch the peer has since rotated past, which a current-key request could
    /// never deliver.
    pub fn awaited_group_keys(&self) -> EyreResult<Vec<([u8; 32], [u8; 32])>> {
        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);
        let op_keys = op_log
            .collect_buffered_group_op_keys()
            .map_err(|e| eyre::eyre!("op_log.collect_buffered_group_op_keys: {e}"))?;
        let ns_typed = ContextGroupId::from(self.namespace_id.to_bytes());

        let mut awaiting = std::collections::BTreeSet::new();
        for (group_id, key_id) in op_keys {
            let gid_typed = ContextGroupId::from(group_id);
            if !self.holds_key_epoch(gid_typed, ns_typed, &key_id)? {
                awaiting.insert((group_id, key_id));
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
        let ns_typed = ContextGroupId::from(self.namespace_id.to_bytes());

        let mut held = std::collections::BTreeSet::new();
        for (group_id, key_id) in op_keys {
            let gid_typed = ContextGroupId::from(group_id);
            if self.holds_key_epoch(gid_typed, ns_typed, &key_id)? {
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
        let ns_typed = ContextGroupId::from(self.namespace_id.to_bytes());

        // This node's own namespace identity. Ops it SIGNED were applied through
        // the local authoring path (`sign_apply_and_publish` →
        // `sign_apply_local_group_op_borsh`) at publish time, which records the
        // GROUP-level nonce. The retry replays from the namespace op-log and the
        // receive apply dedups on the NAMESPACE-level nonce — a separate sequence
        // the local path never wrote — so a node's own op is NOT recognised as
        // already-applied and its mutation re-runs. For an upsert that is
        // harmless, but re-running a REMOVAL (`MemberLeft` / `MemberRemoved`)
        // out of causal order re-deletes membership / `ContextIdentity` /
        // deny-list / re-entry state a causally-later `MemberAdded` restored
        // (the "re-added leaver can't author" bug). A node's own op is never
        // buffered-awaiting-key in the first place (it held the key to encrypt
        // it), so it never needs re-driving here — skip it.
        let own_identity = super::NamespaceRepository::new(self.store)
            .identity(&ns_typed)
            .map_err(|e| eyre::eyre!("resolve own namespace identity: {e}"))?
            .map(|(pk, _sk, _sender)| pk);

        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);
        let entries = op_log
            .collect_signed_group_ops_for_group(group_id)
            .map_err(|e| eyre::eyre!("op_log.collect_signed_group_ops_for_group: {e}"))?;
        for entry in entries {
            if own_identity == Some(entry.signed_op.signer) {
                continue;
            }
            let NamespaceOp::Group { key_id, .. } = entry.signed_op.op else {
                continue;
            };
            // Issue #2256: same fallback as the live-apply path — the op
            // may have been encrypted with the namespace key if the
            // subgroup was `Open` at publish time.
            let group_key = match GroupKeyring::new(self.store, gid_typed)
                .load_key_by_id(key_id.as_bytes())
                .map_err(|e| eyre::eyre!("load_group_key_by_id(group): {e}"))?
            {
                Some(k) => k,
                None => {
                    let Some(k) = GroupKeyring::new(self.store, ns_typed)
                        .load_key_by_id(key_id.as_bytes())
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
