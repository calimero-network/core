use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};

use super::core::NamespaceRepository;
use super::op_log::NamespaceOpLogService;
use crate::{GroupKeyring, MembershipRepository};
use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
use calimero_store::Store;
use eyre::Result as EyreResult;

/// A candidate's rank inside one causal layer of the retry replay.
type TieBreak = ([u8; 32], u64);

/// A namespace group operation that can be retried locally because the
/// corresponding group key is now available.
pub struct RetryCandidate {
    pub signed_op: SignedNamespaceOp,
    pub group_key: [u8; 32],
}

/// Something a retry pass replays: a buffered op, plus whatever the pass needs
/// alongside it to open the op.
///
/// Both replay passes order through [`order_causally`] — the group-op pass and
/// the sealed-root pass — and this is the one thing the ordering needs from
/// either of them.
pub(super) trait Replayable {
    fn signed_op(&self) -> &SignedNamespaceOp;

    /// Order inside one causal layer: a signer's own ops in publish order, and
    /// signer bytes across signers so every replica breaks the tie the same way.
    fn tie_break(&self) -> TieBreak {
        let op = self.signed_op();
        let signer_bytes: &[u8; 32] = op.signer.as_ref();
        (*signer_bytes, op.nonce)
    }
}

impl Replayable for RetryCandidate {
    fn signed_op(&self) -> &SignedNamespaceOp {
        &self.signed_op
    }
}

impl Replayable for super::op_log::StoredSignedGroupOp {
    fn signed_op(&self) -> &SignedNamespaceOp {
        &self.signed_op
    }
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
    /// The namespace root when this node **participates in it and holds an
    /// identity for it, yet holds no key and cannot even resolve itself to an
    /// account** — the state a self-purged TEE replica is left in.
    ///
    /// Disabling fleet HA is a `ReadOnlyTee` self-leave, and the self-purge
    /// then removes the membership row, the account binding, the group keys
    /// and the gov-op log, while deliberately keeping the namespace identity
    /// as a retry anchor. Re-enabling rewrites the participation marker.
    ///
    /// That leaves the replica invisible to BOTH existing worklists:
    /// [`groups_awaiting_key`] needs a buffered op (the log is gone), and
    /// [`groups_member_but_keyless`] resolves the identity to an account
    /// before anything else (the binding is gone). So no key request was
    /// emitted at all, and the node sat subscribed and participating holding
    /// nothing — never reaching the acceptance gate that everyone assumed had
    /// refused it.
    ///
    /// **Asking is not accepting.** This widens only who *emits* a request;
    /// what may be *adopted* is unchanged and still decided by
    /// `key_server_accepted` — a trusted anchor of the group, or a responder
    /// proving it is a device of this node's own account. A node that
    /// qualifies for neither gets a refusal it can log instead of a silent
    /// stall, which is strictly better than emitting nothing.
    ///
    /// Deliberately narrow: this fires only when the identity resolves to NO
    /// account, which is the purged shape. A node that can resolve itself is
    /// already answered by [`groups_member_but_keyless`], so nothing that
    /// worked before changes.
    pub fn root_participating_but_unbootstrapped(&self) -> EyreResult<Vec<[u8; 32]>> {
        let ns_typed = ContextGroupId::from(self.namespace_id.to_bytes());

        // No identity ⇒ nothing to recover, and nothing to recover it AS.
        let Some(record) = NamespaceRepository::new(self.store).identity_record(&ns_typed)? else {
            return Ok(Vec::new());
        };
        let my_identity = record.public_key;

        // No separate participation check: `store_identity` writes the
        // `NamespaceParticipation` row itself, so holding an identity for this
        // namespace IS participating in it. Scanning the participation rows
        // again would cost an O(namespaces) walk on every recovery tick to
        // re-derive what the identity read above already established.

        // Resolvable ⇒ `groups_member_but_keyless` already owns this case.
        if crate::member_account_in_namespace(self.store, &ns_typed, &my_identity)?.is_some() {
            return Ok(Vec::new());
        }

        // Only a root covered by its own keyring can be recovered here, matching
        // `groups_member_but_keyless`.
        if crate::key_covering_group(self.store, &ns_typed)? != ns_typed {
            return Ok(Vec::new());
        }

        let has_key = GroupKeyring::new(self.store, ns_typed)
            .load_current_key()
            .map_err(|e| eyre::eyre!("load_current_key(root): {e}"))?
            .is_some();
        if has_key {
            return Ok(Vec::new());
        }

        Ok(vec![ns_typed.to_bytes()])
    }

    pub fn groups_member_but_keyless(&self) -> EyreResult<Vec<[u8; 32]>> {
        let ns_typed = ContextGroupId::from(self.namespace_id.to_bytes());

        // The member we'd be missing a key for is this node's namespace
        // identity. No identity ⇒ nothing to recover.
        let my_identity = match NamespaceRepository::new(self.store).identity_record(&ns_typed)? {
            Some(record) => record.public_key,
            None => return Ok(Vec::new()),
        };

        // No short-circuit on "this node holds the namespace key".
        //
        // There was one, on the reasoning that the root key decrypts the root
        // group and every `Open` subgroup, so holding it means nothing here is
        // keyless. That skips the case it is most needed for: a **Restricted**
        // subgroup is covered by its OWN key, which the namespace key does not
        // open, so a node with a direct row there is a keyless member of it
        // while holding the namespace key — and returning empty meant no key
        // request was ever emitted, leaving the membership undecryptable with
        // nothing to re-drive it and no error anywhere.
        //
        // Two ordinary ways in: a subgroup flips `Open -> Restricted`
        // (`SubgroupVisibilitySet` distributes no key — it writes the visibility
        // row and queues an event, nothing more), so a direct member that never
        // needed the group's own key suddenly does; or a `KeyDelivery` for a
        // Restricted subgroup is missed while the node is offline, which is
        // precisely what this pull is the safety net for.
        //
        // The per-group loop below already answers correctly for every group,
        // the root included: it requires a direct row, skips anything not
        // covered by its own keyring (`key_covering_group`), and then reads
        // THAT group's keyring. So dropping the early return removes a wrong
        // answer without changing any right one — it costs one descendant walk
        // in the case that used to return early.

        // Every group in the namespace: the root plus all descendants.
        let mut groups = vec![ns_typed];
        groups.extend(NamespaceRepository::new(self.store).collect_descendants(&ns_typed)?);

        let mut out = std::collections::BTreeSet::new();
        for gid in groups {
            // `my_identity` is this node's signing key; the row names its
            // account. An unresolved key holds no row anywhere, so skipping is
            // the same answer reached one step earlier.
            let Some(my_account) =
                crate::member_account_in_namespace(self.store, &gid, &my_identity)?
            else {
                continue;
            };
            if !MembershipRepository::new(self.store).has_direct_member(&gid, &my_account)? {
                continue;
            }
            // Only a group covered by its OWN keyring can be recovered here.
            //
            // A group on an Open chain is covered by the namespace key, which
            // `has_namespace_key` already established is absent — and no peer can
            // supply the group's own row for it either, because nothing is
            // encrypted under that row. Flagging it would emit a key request
            // nobody can satisfy, and the namespace key is not an answer we may
            // ask for on a subgroup's behalf: a member of the subgroup alone is
            // not entitled to it. So there is nothing recoverable here; skip.
            if crate::key_covering_group(self.store, &gid)? != gid {
                continue;
            }
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
            .map(|(pk, _sk)| pk);

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

        // Replay in causal order: an op whose ancestor is in the same batch
        // applies after it, and `(signer, nonce)` decides the rest — so a
        // signer's own ops keep publish order (a higher nonce applying first
        // would window the lower one away as a duplicate) and two replicas
        // order concurrent ops identically.
        order_causally(&op_log, candidates)
    }
}

/// Topologically order a retry batch over the causal edges between its ops.
///
/// Ancestry comes from the stored DAG rather than from a candidate's own
/// `parent_op_hashes`, because the path between two buffered ops usually runs
/// through ops that are not candidates (cleartext root ops, ops applied live).
pub(super) fn order_causally<T: Replayable>(
    op_log: &NamespaceOpLogService<'_>,
    candidates: Vec<T>,
) -> EyreResult<Vec<T>> {
    let mut index = HashMap::new();
    for (i, candidate) in candidates.iter().enumerate() {
        let hash = candidate
            .signed_op()
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        let _ = index.insert(hash, i);
    }

    let mut pending_ancestors = vec![0usize; candidates.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); candidates.len()];
    for (i, candidate) in candidates.iter().enumerate() {
        for ancestor in candidate_ancestors(op_log, &index, candidate.signed_op()) {
            dependents[ancestor].push(i);
            pending_ancestors[i] += 1;
        }
    }

    let mut ready: BinaryHeap<Reverse<(TieBreak, usize)>> = (0..candidates.len())
        .filter(|i| pending_ancestors[*i] == 0)
        .map(|i| Reverse((candidates[i].tie_break(), i)))
        .collect();
    let mut order = Vec::with_capacity(candidates.len());
    let mut placed = vec![false; candidates.len()];
    while let Some(Reverse((_, i))) = ready.pop() {
        placed[i] = true;
        order.push(i);
        for dependent in &dependents[i] {
            pending_ancestors[*dependent] -= 1;
            if pending_ancestors[*dependent] == 0 {
                ready.push(Reverse((candidates[*dependent].tie_break(), *dependent)));
            }
        }
    }
    // Unreachable while op ids are content hashes, which cannot form a cycle.
    // Appending rather than dropping keeps a corrupt store from losing an op.
    let mut stranded: Vec<usize> = (0..candidates.len()).filter(|i| !placed[*i]).collect();
    stranded.sort_by_key(|i| candidates[*i].tie_break());
    order.extend(stranded);

    let mut slots: Vec<Option<T>> = candidates.into_iter().map(Some).collect();
    Ok(order.into_iter().filter_map(|i| slots[i].take()).collect())
}

/// Indices of the candidates that are ancestors of `op`, by walking the stored
/// DAG from its parents. A path stops at the first candidate it reaches, whose
/// own ancestors are already ordered ahead of it.
fn candidate_ancestors(
    op_log: &NamespaceOpLogService<'_>,
    index: &HashMap<[u8; 32], usize>,
    op: &SignedNamespaceOp,
) -> BTreeSet<usize> {
    let mut found = BTreeSet::new();
    let mut visited: HashSet<[u8; 32]> = HashSet::new();
    let mut queue: VecDeque<[u8; 32]> = op.parent_op_hashes.iter().copied().collect();
    while let Some(hash) = queue.pop_front() {
        if !visited.insert(hash) {
            continue;
        }
        if let Some(i) = index.get(&hash) {
            let _ = found.insert(*i);
            continue;
        }
        // An absent or unreadable ancestor is ancestry this node cannot know;
        // the op-log walks report the same condition the same way.
        match op_log.get_signed_op(hash) {
            Ok(Some(parent)) => queue.extend(parent.parent_op_hashes.iter().copied()),
            Ok(None) => {}
            Err(e) => tracing::warn!(
                delta_id = %hex::encode(hash),
                error = %format!("{e:#}"),
                "skipping unreadable ancestor while ordering a retry batch"
            ),
        }
    }
    found
}
