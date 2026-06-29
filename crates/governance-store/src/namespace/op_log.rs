use calimero_context_client::local_governance::{
    NamespaceOp, OpaqueSkeleton, SignedNamespaceOp, StoredNamespaceEntry,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use crate::metrics::{record_namespace_decode_fallback, record_namespace_decode_invalid};

/// Typed namespace group entry decoded from the namespace op-log.
pub struct StoredSignedGroupOp {
    pub signed_op: SignedNamespaceOp,
    pub key_id: [u8; 32],
}

/// Service for persisting and reading namespace governance op-log entries.
pub struct NamespaceOpLogService<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
}

impl<'a> NamespaceOpLogService<'a> {
    pub fn new(store: &'a Store, namespace_id: [u8; 32]) -> Self {
        Self {
            store,
            namespace_id,
        }
    }

    /// Direct existence check for an op by its content hash within this
    /// namespace. O(1) key lookup — does not load the op body. Used by
    /// position-aware membership lookup ([`super::membership_status`]) to
    /// detect whether a referenced governance head is present locally
    /// without paying the cost of a full op-log scan.
    pub fn contains_op(&self, delta_id: [u8; 32]) -> EyreResult<bool> {
        let handle = self.store.handle();
        let key = calimero_store::key::NamespaceGovOp::new(self.namespace_id, delta_id);
        handle
            .has(&key)
            .map_err(|e| eyre::eyre!("contains_op: {e}"))
    }

    /// Direct fetch of a `SignedNamespaceOp` by its content hash within this
    /// namespace. Returns `Ok(None)` if the op is **not present** in the
    /// local store; returns `Err` if the op is present but its stored bytes
    /// fail to decode. Used by the prefix-walk membership lookup to traverse
    /// the governance DAG by following each op's `parent_op_hashes`.
    ///
    /// The decode-failure-vs-absent distinction matters: callers (e.g. the
    /// prefix walk) treat `Ok(None)` as "missing parent → buffer + retry,"
    /// which is correct for a genuinely-absent op (partial sync) but wrong
    /// for a corrupt-store-entry case (retry will never resolve). Surfacing
    /// decode failure as `Err` instead lets callers fail loud rather than
    /// silently re-buffering the delta until the drain-attempt counter
    /// drops it with a misleading "permanently missing" log.
    pub fn get_signed_op(&self, delta_id: [u8; 32]) -> EyreResult<Option<SignedNamespaceOp>> {
        let handle = self.store.handle();
        let key = calimero_store::key::NamespaceGovOp::new(self.namespace_id, delta_id);
        let value: calimero_store::key::NamespaceGovOpValue = match handle.get(&key) {
            Ok(Some(v)) => v,
            Ok(None) => return Ok(None),
            Err(e) => return Err(eyre::eyre!("get_signed_op: {e}")),
        };
        decode_signed_namespace_op(&value.skeleton_bytes)
            .map(Some)
            .ok_or_else(|| {
                eyre::eyre!(
                    "get_signed_op: op {} present but decode failed (corrupt store entry?)",
                    hex::encode(delta_id)
                )
            })
    }

    pub fn store_signed_operation(&self, op: &SignedNamespaceOp) -> EyreResult<()> {
        if op.namespace_id != self.namespace_id {
            bail!(
                "namespace mismatch when storing op: handle={}, op={}",
                hex::encode(self.namespace_id),
                hex::encode(op.namespace_id)
            );
        }

        let delta_id = op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        let key = calimero_store::key::NamespaceGovOp::new(self.namespace_id, delta_id);
        let value = calimero_store::key::NamespaceGovOpValue {
            skeleton_bytes: borsh::to_vec(&StoredNamespaceEntry::Signed(op.clone()))
                .map_err(|e| eyre::eyre!("borsh: {e}"))?,
        };

        let mut handle = self.store.handle();
        handle.put(&key, &value)?;

        // ATOMIC unified op-store write: persist the DECODED `Op` on the SAME
        // `handle` as the gov-DAG put above, so the op-store can never lag the
        // gov-DAG (the read-flip blocker). The op-store is keyed by the op's
        // **namespace** scope ‖ op id, mirroring `unified_op_store::persist_op`
        // in `calimero-context`; that apply-time dual-write (and the per-site
        // re-persists) stay as a redundant-but-idempotent belt-and-suspenders.
        //
        // For an encrypted group op, decrypt best-effort (the key may not have
        // arrived yet); on failure fold as `Noop` (pass `None`), exactly like the
        // dual-write — the late-key case is recovered by `repersist_namespace_ops`.
        if let Err(err) = self.put_unified_op(&mut handle, op, delta_id) {
            // Never fail the gov-DAG write on a decode/op-store hiccup. The op-store
            // is a redundant projection backing here; a miss is recoverable by the
            // existing backfill/repersist paths. Logged, not propagated.
            tracing::warn!(
                %err,
                namespace_id = %hex::encode(self.namespace_id),
                delta_id = %hex::encode(delta_id),
                "unified op-store: atomic op-store write failed; gov-DAG write kept"
            );
        }
        Ok(())
    }

    /// Build the decoded unified [`Op`] for `signed` and `put` it onto the
    /// caller's `handle` (the SAME handle that just wrote the gov-DAG op), so the
    /// op-store entry is atomic with the gov-DAG entry. `delta_id` is the op's
    /// already-computed content hash (the gov-DAG key), reused to avoid re-hashing.
    fn put_unified_op(
        &self,
        handle: &mut calimero_store::Handle<Store>,
        signed: &SignedNamespaceOp,
        delta_id: [u8; 32],
    ) -> EyreResult<()> {
        // Re-derive the op's id/hlc/parents exactly as the projection backfill
        // does, so the persisted op id is byte-identical to the dual-write's.
        let delta = crate::unified_op_decode::signed_namespace_op_to_delta(signed)?;

        // Decrypt an encrypted group op so its membership change folds; a failure
        // (no key for this group yet) leaves it a `Noop` node — still persisted so
        // an ancestry walk can pass through it (the late-key case is repersisted
        // once the key arrives).
        let decrypted = match &signed.op {
            NamespaceOp::Group {
                group_id,
                key_id,
                encrypted,
                ..
            } => crate::decrypt_group_op(
                self.store,
                self.namespace_id,
                calimero_context_config::types::ContextGroupId::from(*group_id),
                key_id,
                encrypted,
            )
            .ok()
            .flatten(),
            NamespaceOp::Root(_) => None,
            // `NamespaceOp` is `#[non_exhaustive]`; nothing to decrypt for an
            // unknown future op variant.
            _ => None,
        };

        let unified_op = crate::unified_op_decode::op_from_namespace_op(
            signed,
            decrypted.as_ref(),
            delta.id,
            delta.hlc,
            &delta.parents,
        );

        let scope = calimero_op::ScopeId::from(self.namespace_id);
        let key = calimero_store::key::ScopeUnifiedOp::new(*scope.as_bytes(), unified_op.id);
        let bytes = borsh::to_vec(&unified_op).map_err(|e| eyre::eyre!("borsh op: {e}"))?;
        let value =
            calimero_store::types::ScopeUnifiedOp::from(calimero_store::slice::Slice::from(bytes));
        handle.put(&key, &value)?;
        // Sanity: the op-store and the gov-DAG must key the same op identity.
        debug_assert_eq!(unified_op.id, delta_id, "unified op id != gov-DAG delta id");
        Ok(())
    }

    pub fn collect_signed_group_ops_for_group(
        &self,
        group_id: [u8; 32],
    ) -> EyreResult<Vec<StoredSignedGroupOp>> {
        let mut entries = Vec::new();
        let handle = self.store.handle();
        let start = calimero_store::key::NamespaceGovOp::new(self.namespace_id, [0u8; 32]);
        let mut iter = handle
            .iter::<calimero_store::key::NamespaceGovOp>()
            .map_err(|e| eyre::eyre!("iter::<NamespaceGovOp>: {e}"))?;
        let first = iter.seek(start).transpose();

        // The `Group` column family is shared by several key types
        // sorted by their 1-byte prefix:
        //
        //   0x20 GroupMeta          (1+32 bytes)
        //   0x32 GroupMemberContext (1+32+32 bytes)
        //   0x38 NamespaceGovOp     (1+32+32 bytes) ← this iterator's type
        //   0x39 NamespaceGovHead   (1+32 bytes)
        //   0x3A GroupKey           (varies)
        //
        // `iter::<NamespaceGovOp>()` does NOT filter by prefix — it
        // returns the entire column. After the last 0x38 entry the
        // walk runs into 0x39 (`NamespaceGovHead`, 33 bytes) and
        // borsh's `(GroupPrefix, GroupIdComponent, GroupIdComponent)`
        // decoder (65 bytes) trips with `Unexpected length of input`.
        // The previous unconditional `key_result?` turned that
        // upper-bound signal into a propagated error that aborted the
        // whole apply_kd closure on the receiver, leaving group keys
        // un-stored and the next `join_context` failing with
        // "identity is not a member of the group".
        //
        // The first key-decode error is therefore a *legitimate*
        // upper-bound marker, not a corruption symptom: every
        // subsequent key has a different-and-higher prefix and
        // would fail the same way. Break once we hit it. The
        // walk-end namespace_id check below stays as the bound for
        // the in-prefix case (a key from a *later* namespace at the
        // same 0x38 prefix).
        let mut entries_iter = first.into_iter().chain(iter.keys());
        loop {
            let key = match entries_iter.next() {
                None => break,
                Some(Ok(k)) => k,
                Some(Err(_)) => {
                    // Past the 0x38 prefix — all remaining keys are
                    // shorter-layout types in the same column family.
                    // Recording a metric here is silent at apply time
                    // but visible in dashboards if any genuine
                    // corruption ever shows up alongside.
                    record_namespace_decode_invalid("signed_iter_end");
                    break;
                }
            };
            if key.namespace_id() != self.namespace_id {
                break;
            }
            let value: calimero_store::key::NamespaceGovOpValue = match handle.get(&key) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(e) => {
                    // Per-value decode failure is genuine corruption
                    // (the key parsed as `NamespaceGovOp`, so this is
                    // ours). Skip and warn rather than abort the walk
                    // — the rest of this namespace's ops are still
                    // valid retry candidates.
                    record_namespace_decode_invalid("signed_iter_value");
                    tracing::warn!(
                        namespace_id = %hex::encode(self.namespace_id),
                        delta_id = %hex::encode(key.delta_id()),
                        error = %e,
                        "skipping undecodable NamespaceGovOpValue during retry walk"
                    );
                    continue;
                }
            };
            let Some(signed_op) = decode_signed_namespace_op(&value.skeleton_bytes) else {
                continue;
            };
            let NamespaceOp::Group {
                group_id: op_group_id,
                key_id,
                ..
            } = signed_op.op
            else {
                continue;
            };
            if op_group_id != group_id {
                continue;
            }
            entries.push(StoredSignedGroupOp { signed_op, key_id });
        }

        Ok(entries)
    }

    /// Every buffered `NamespaceOp::Group` op for this namespace, as
    /// `(group_id, key_id)` pairs. Same column walk and prefix-collision
    /// termination as [`collect_signed_group_ops_for_group`](Self::collect_signed_group_ops_for_group)
    /// — see the long comment there — but across all groups, carrying the
    /// per-op `key_id` so the caller can decide decryptability per op
    /// (a node may hold the namespace key yet still lack a *Restricted*
    /// subgroup's own key). Used by the joiner-side direct key-delivery
    /// pull to learn which groups it has undecryptable pending ops for.
    /// Deduplicated on `(group_id, key_id)`.
    pub fn collect_buffered_group_op_keys(&self) -> EyreResult<Vec<([u8; 32], [u8; 32])>> {
        let mut seen = std::collections::BTreeSet::new();
        let handle = self.store.handle();
        let start = calimero_store::key::NamespaceGovOp::new(self.namespace_id, [0u8; 32]);
        let mut iter = handle
            .iter::<calimero_store::key::NamespaceGovOp>()
            .map_err(|e| eyre::eyre!("iter::<NamespaceGovOp>: {e}"))?;
        let first = iter.seek(start).transpose();

        let mut entries_iter = first.into_iter().chain(iter.keys());
        loop {
            let key = match entries_iter.next() {
                None => break,
                Some(Ok(k)) => k,
                Some(Err(_)) => {
                    record_namespace_decode_invalid("group_ids_iter_end");
                    break;
                }
            };
            if key.namespace_id() != self.namespace_id {
                break;
            }
            let value: calimero_store::key::NamespaceGovOpValue = match handle.get(&key) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(_) => {
                    record_namespace_decode_invalid("group_ids_iter_value");
                    continue;
                }
            };
            let Some(signed_op) = decode_signed_namespace_op(&value.skeleton_bytes) else {
                continue;
            };
            if let NamespaceOp::Group {
                group_id: op_group_id,
                key_id,
                ..
            } = signed_op.op
            {
                seen.insert((op_group_id, key_id));
            }
        }

        Ok(seen.into_iter().collect())
    }

    pub fn collect_opaque_skeleton_delta_ids_for_group(
        &self,
        group_id: [u8; 32],
    ) -> EyreResult<Vec<[u8; 32]>> {
        let mut delta_ids = Vec::new();
        let handle = self.store.handle();
        let start = calimero_store::key::NamespaceGovOp::new(self.namespace_id, [0u8; 32]);
        let mut iter = handle.iter::<calimero_store::key::NamespaceGovOp>()?;
        let first = iter.seek(start).transpose();

        // Same prefix-collision termination as
        // `collect_signed_group_ops_for_group` — see the long comment
        // there. The first key-decode error means we've walked past
        // the 0x38 prefix into a different-layout key type in the
        // shared `Group` column, not actual corruption.
        let mut entries_iter = first.into_iter().chain(iter.keys());
        loop {
            let key = match entries_iter.next() {
                None => break,
                Some(Ok(k)) => k,
                Some(Err(_)) => {
                    record_namespace_decode_invalid("opaque_iter_end");
                    break;
                }
            };
            if key.namespace_id() != self.namespace_id {
                break;
            }
            let value: calimero_store::key::NamespaceGovOpValue = match handle.get(&key) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(e) => {
                    record_namespace_decode_invalid("opaque_iter_value");
                    tracing::warn!(
                        namespace_id = %hex::encode(self.namespace_id),
                        delta_id = %hex::encode(key.delta_id()),
                        error = %e,
                        "skipping undecodable NamespaceGovOpValue during opaque walk"
                    );
                    continue;
                }
            };
            let Some(skeleton) = decode_opaque_skeleton(&value.skeleton_bytes) else {
                continue;
            };
            if skeleton.group_id == group_id {
                delta_ids.push(skeleton.delta_id);
            }
        }

        Ok(delta_ids)
    }
}

fn decode_signed_namespace_op(bytes: &[u8]) -> Option<SignedNamespaceOp> {
    if let Ok(StoredNamespaceEntry::Signed(op)) = borsh::from_slice::<StoredNamespaceEntry>(bytes) {
        return Some(op);
    }
    if let Ok(op) = borsh::from_slice::<SignedNamespaceOp>(bytes) {
        record_namespace_decode_fallback("signed");
        return Some(op);
    }
    record_namespace_decode_invalid("signed");
    None
}

fn decode_opaque_skeleton(bytes: &[u8]) -> Option<OpaqueSkeleton> {
    if let Ok(StoredNamespaceEntry::Opaque(skeleton)) =
        borsh::from_slice::<StoredNamespaceEntry>(bytes)
    {
        return Some(skeleton);
    }
    if let Ok(skeleton) = borsh::from_slice::<OpaqueSkeleton>(bytes) {
        record_namespace_decode_fallback("opaque");
        return Some(skeleton);
    }
    record_namespace_decode_invalid("opaque");
    None
}
