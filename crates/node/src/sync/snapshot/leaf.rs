//! The checks every entity a snapshot ships must pass on arrival.

use calimero_storage::address::Id;
use calimero_storage::constants::DRIFT_TOLERANCE_NANOS;
use calimero_storage::index::EntityIndex;
use sha2::{Digest, Sha256};

/// What is wrong with an entity a snapshot ships, if anything: an index row
/// naming another entity or dated ahead of the clock, or bytes its `own_hash` does not commit to.
pub(super) fn leaf_defect(
    id: Id,
    entry: &[u8],
    index: &EntityIndex,
    now: u64,
) -> Option<&'static str> {
    if index.id() != id {
        return Some("its index names another entity");
    }
    if index.metadata.updated_at() > now.saturating_add(DRIFT_TOLERANCE_NANOS) {
        return Some("its update time is ahead of the clock");
    }
    let hash: [u8; 32] = Sha256::digest(entry).into();
    (hash != index.own_hash()).then_some("its bytes do not match its own hash")
}

/// Index rows that tests edit or write by hand, over the real `EntityIndex`.
#[cfg(test)]
pub(super) mod rows {
    use borsh::BorshDeserialize;
    use calimero_storage::address::Id;
    use calimero_storage::entities::Metadata;
    use calimero_storage::index::EntityIndex;
    use sha2::{Digest, Sha256};

    /// The fields of an `EntityIndex`, open to editing.
    pub(in crate::sync::snapshot) struct IndexRow {
        pub id: [u8; 32],
        pub parent_id: Option<[u8; 32]>,
        pub full_hash: [u8; 32],
        pub own_hash: [u8; 32],
        pub metadata: Metadata,
        pub deleted_at: Option<u64>,
        pub deleted_children: Vec<[u8; 32]>,
    }

    impl IndexRow {
        /// The row as `EntityIndex` encodes it.
        pub(in crate::sync::snapshot) fn encode(&self) -> Vec<u8> {
            let id = Id::new(self.id);
            let mut index = match self.parent_id {
                Some(parent) => {
                    EntityIndex::minimal_for_test_with_parent(id, Id::new(parent), self.full_hash)
                }
                None => EntityIndex::minimal_for_test_with_full_hash(id, self.full_hash),
            }
            .with_own_hash_for_test(self.own_hash);
            index.metadata = self.metadata.clone();
            index.deleted_at = self.deleted_at;
            index.deleted_children = self.deleted_children.iter().copied().map(Id::new).collect();
            borsh::to_vec(&index).expect("an index row encodes")
        }
    }

    pub(in crate::sync::snapshot) fn decode(bytes: &[u8]) -> IndexRow {
        let index = EntityIndex::try_from_slice(bytes).expect("an index row");
        let row = IndexRow {
            id: *index.id().as_bytes(),
            parent_id: index.parent_id().map(|parent| *parent.as_bytes()),
            full_hash: index.full_hash(),
            own_hash: index.own_hash(),
            metadata: index.metadata.clone(),
            deleted_at: index.deleted_at,
            deleted_children: index
                .deleted_children()
                .iter()
                .map(|id| *id.as_bytes())
                .collect(),
        };
        assert_eq!(row.encode(), bytes, "the row round-trips");
        row
    }

    /// `index` with an `own_hash` that is the hash of `entry`.
    pub(in crate::sync::snapshot) fn with_own_hash(index: &[u8], entry: &[u8]) -> Vec<u8> {
        let mut row = decode(index);
        row.own_hash = Sha256::digest(entry).into();
        row.encode()
    }
}
