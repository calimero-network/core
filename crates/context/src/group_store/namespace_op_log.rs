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
