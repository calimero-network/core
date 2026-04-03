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
        let mut iter = handle.iter::<calimero_store::key::NamespaceGovOp>()?;
        let first = iter.seek(start).transpose();

        for key_result in first.into_iter().chain(iter.keys()) {
            let key = key_result?;
            if key.namespace_id() != self.namespace_id {
                break;
            }
            let Some(value): Option<calimero_store::key::NamespaceGovOpValue> = handle.get(&key)?
            else {
                continue;
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

        for key_result in first.into_iter().chain(iter.keys()) {
            let key = key_result?;
            if key.namespace_id() != self.namespace_id {
                break;
            }
            let Some(value): Option<calimero_store::key::NamespaceGovOpValue> = handle.get(&key)?
            else {
                continue;
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
