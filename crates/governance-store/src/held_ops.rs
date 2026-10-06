//! Group ops this node holds unapplied because their group's history is sealed
//! under a key it lacks (core#4511), listed per namespace.
//!
//! A held op is logged and the namespace's head moves past it, but its effect is
//! missing on this node until the group's key arrives - for a node outside the
//! group, never. Nothing else records that, so without this list a folder that
//! looks stale on one node and fine on another is explained only by a log line.
//! The list is also what lets the namespace-key re-drive skip an op a namespace
//! key cannot open, instead of failing it, and warning, on every delivery.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_governance_types::NamespaceId;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::Result as EyreResult;

/// 16-byte `Generic` scope of the rows: one row per namespace.
const SCOPE: [u8; 16] = *b"calimero-heldops";

/// Most held ops listed per namespace. Ops come from peers, so the list is
/// bounded; past it a hold is only counted, in [`HeldOpsRecord::untracked`].
pub const MAX_HELD_OPS_LISTED: usize = 1024;

/// One held op.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct HeldOp {
    /// The namespace op's content hash, its id in the namespace DAG.
    pub delta_id: [u8; 32],
    /// The group whose sealed history it waits on.
    pub group_id: [u8; 32],
}

/// The held ops of one namespace.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct HeldOpsRecord {
    pub ops: Vec<HeldOp>,
    /// Holds past [`MAX_HELD_OPS_LISTED`], counted but not listed.
    pub untracked: u64,
}

pub struct HeldOps<'a> {
    store: &'a Store,
    namespace: NamespaceId,
}

impl<'a> HeldOps<'a> {
    #[must_use]
    pub fn new(store: &'a Store, namespace: NamespaceId) -> Self {
        Self { store, namespace }
    }

    /// Record that `delta_id` is held on `group_id`'s sealed history.
    pub(crate) fn hold(&self, delta_id: [u8; 32], group_id: [u8; 32]) -> EyreResult<()> {
        let mut record = self.read()?;
        if record.ops.iter().any(|op| op.delta_id == delta_id) {
            return Ok(());
        }
        if record.ops.len() >= MAX_HELD_OPS_LISTED {
            record.untracked = record.untracked.saturating_add(1);
        } else {
            record.ops.push(HeldOp { delta_id, group_id });
        }
        self.write(&record)
    }

    /// `delta_id` applied: it is no longer held.
    pub(crate) fn release(&self, delta_id: [u8; 32]) -> EyreResult<()> {
        let mut record = self.read()?;
        let before = record.ops.len();
        record.ops.retain(|op| op.delta_id != delta_id);
        if record.ops.len() == before {
            return Ok(());
        }
        self.write(&record)
    }

    pub(crate) fn contains(&self, delta_id: [u8; 32]) -> EyreResult<bool> {
        Ok(self.read()?.ops.iter().any(|op| op.delta_id == delta_id))
    }

    /// Every held op of the namespace, in the order they were held.
    pub fn read(&self) -> EyreResult<HeldOpsRecord> {
        let handle = self.store.handle();
        let Some(data) = handle.get(&self.key())? else {
            return Ok(HeldOpsRecord::default());
        };
        let bytes: &[u8] = data.as_ref();
        HeldOpsRecord::try_from_slice(bytes)
            .map_err(|e| eyre::eyre!("held-ops row does not decode: {e}"))
    }

    fn write(&self, record: &HeldOpsRecord) -> EyreResult<()> {
        let mut handle = self.store.handle();
        if record.ops.is_empty() && record.untracked == 0 {
            handle.delete(&self.key())?;
            return Ok(());
        }
        let bytes = borsh::to_vec(record)?;
        handle.put(&self.key(), &GenericData::from(Slice::from(bytes)))?;
        Ok(())
    }

    fn key(&self) -> GenericKey {
        GenericKey::new(SCOPE, *self.namespace.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::test_store;

    #[test]
    fn a_held_op_is_listed_until_it_is_released() {
        let store = test_store();
        let held = HeldOps::new(&store, NamespaceId::from([7; 32]));

        held.hold([1; 32], [9; 32]).expect("hold");
        held.hold([1; 32], [9; 32])
            .expect("a second hold of the same op is a no-op");
        assert_eq!(
            held.read().expect("read").ops,
            vec![HeldOp {
                delta_id: [1; 32],
                group_id: [9; 32]
            }]
        );
        assert!(held.contains([1; 32]).expect("contains"));

        held.release([1; 32]).expect("release");
        assert_eq!(held.read().expect("read"), HeldOpsRecord::default());
        assert!(!held.contains([1; 32]).expect("contains"));
    }

    #[test]
    fn the_list_is_bounded_and_counts_what_it_cannot_hold() {
        let store = test_store();
        let held = HeldOps::new(&store, NamespaceId::from([7; 32]));
        for i in 0..=MAX_HELD_OPS_LISTED {
            let mut delta = [0; 32];
            delta[..8].copy_from_slice(&(i as u64).to_le_bytes());
            held.hold(delta, [9; 32]).expect("hold");
        }
        let record = held.read().expect("read");
        assert_eq!(record.ops.len(), MAX_HELD_OPS_LISTED);
        assert_eq!(record.untracked, 1);
    }

    #[test]
    fn namespaces_keep_separate_lists() {
        let store = test_store();
        HeldOps::new(&store, NamespaceId::from([7; 32]))
            .hold([1; 32], [9; 32])
            .expect("hold");
        assert!(HeldOps::new(&store, NamespaceId::from([8; 32]))
            .read()
            .expect("read")
            .ops
            .is_empty());
    }
}
