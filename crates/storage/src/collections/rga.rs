//! Replicated Growable Array (RGA) - A CRDT for collaborative text editing
//!
//! RGA provides a conflict-free way to edit text collaboratively by storing
//! each character with ordering metadata in an UnorderedMap.
//!
//! ## Architecture
//!
//! Built on top of existing CRDT infrastructure:
//! - **UnorderedMap**: Provides storage, tombstone deletion, and CRDT merging
//! - **CharId**: Unique identifier combining HLC timestamp + sequence number
//! - **RGA Ordering**: Characters ordered by (left_neighbor, char_id)
//!
//! ## Example
//!
//! ```ignore
//! use calimero_storage::collections::ReplicatedGrowableArray;
//!
//! let mut rga = ReplicatedGrowableArray::new();
//! rga.insert(0, 'H').unwrap();
//! rga.insert(1, 'i').unwrap();
//! assert_eq!(rga.get_text().unwrap(), "Hi");
//!
//! rga.delete(0).unwrap();
//! assert_eq!(rga.get_text().unwrap(), "i");
//! ```

use borsh::{BorshDeserialize, BorshSerialize};

use super::{CrdtType, UnorderedMap};
use crate::collections::error::StoreError;
use crate::env;
use crate::store::{MainStorage, StorageAdaptor};

/// Unique identifier for a character in the RGA
///
/// Combines HLC timestamp with sequence number for global uniqueness.
/// Ordered lexicographically for deterministic conflict resolution.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
struct CharId {
    /// HLC timestamp when character was inserted
    timestamp: crate::logical_clock::HybridTimestamp,
    /// Sequence number for characters inserted in same operation
    seq: u32,
}

impl CharId {
    fn new(timestamp: crate::logical_clock::HybridTimestamp, seq: u32) -> Self {
        Self { timestamp, seq }
    }

    /// Root ID representing the beginning of the document (sentinel)
    fn root() -> Self {
        Self {
            timestamp: crate::logical_clock::HybridTimestamp::default(),
            seq: 0,
        }
    }
}

/// Storage key for a character (owns serialized bytes for AsRef<[u8]>)
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CharKey {
    id: CharId,
    bytes: Vec<u8>,
}

impl BorshSerialize for CharKey {
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        // Only serialize the id, bytes can be reconstructed
        self.id.serialize(writer)
    }
}

impl BorshDeserialize for CharKey {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        let id = CharId::deserialize_reader(reader)?;
        // Reconstruct bytes from id
        let bytes = borsh::to_vec(&id).map_err(borsh::io::Error::other)?;
        Ok(Self { id, bytes })
    }
}

impl CharKey {
    fn new(id: CharId) -> Self {
        // CharId is a simple fixed-size struct, serialization is infallible in practice
        // Use unwrap_or_default as a safety fallback (should never occur)
        let bytes = borsh::to_vec(&id).unwrap_or_default();
        Self { id, bytes }
    }

    fn id(&self) -> CharId {
        self.id
    }
}

impl AsRef<[u8]> for CharKey {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl From<CharId> for CharKey {
    fn from(id: CharId) -> Self {
        Self::new(id)
    }
}

/// A character in the RGA with ordering metadata
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub(crate) struct RgaChar {
    /// The actual character content (stored as u32 for Borsh compatibility)
    content: u32,
    /// ID of the character to the left (for RGA ordering)
    left: CharId,
}

impl RgaChar {
    fn new(content: char, left: CharId) -> Self {
        Self {
            content: content as u32,
            left,
        }
    }

    fn as_char(&self) -> char {
        char::from_u32(self.content).unwrap_or('�') // Replacement character for invalid
    }
}

/// Replicated Growable Array - A CRDT for collaborative text editing
///
/// Built on UnorderedMap for automatic CRDT behavior, with RGA ordering logic.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct ReplicatedGrowableArray<S: StorageAdaptor = MainStorage> {
    /// Characters stored by CharKey with ordering metadata
    #[borsh(bound(serialize = "", deserialize = ""))]
    pub(crate) chars: UnorderedMap<CharKey, RgaChar, S>,
}

/// Re-key the RGA's char map relative to its storage parent so an RGA stored as
/// a collection value converges across nodes. See [`super::rekey`].
impl<S: StorageAdaptor> super::rekey::RekeyTarget for ReplicatedGrowableArray<S> {
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        self.chars
            .reassign_deterministic_id_under(parent_id, "__rga_chars", CrdtType::Rga);
        self.chars.set_collection_crdt_type(CrdtType::Rga);
    }
}

impl ReplicatedGrowableArray<MainStorage> {
    /// Create a new empty RGA with a random ID.
    ///
    /// Use this for nested collections stored as values in other maps.
    /// Merge happens by the parent map's key, so the nested collection's ID
    /// doesn't affect sync semantics.
    ///
    /// For top-level state fields, use `new_with_field_name` instead.
    #[must_use]
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new RGA with a deterministic ID.
    ///
    /// The `field_name` is used to generate a deterministic collection ID,
    /// ensuring the same code produces the same ID across all nodes.
    ///
    /// Use this for top-level state fields (the `#[app::state]` macro does this
    /// automatically).
    ///
    /// # Example
    /// ```ignore
    /// let document = ReplicatedGrowableArray::new_with_field_name("document");
    /// ```
    #[must_use]
    pub fn new_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name_internal(None, field_name)
    }
}

impl Default for ReplicatedGrowableArray<MainStorage> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: StorageAdaptor> ReplicatedGrowableArray<S> {
    fn new_internal() -> Self {
        Self {
            chars: UnorderedMap::new_internal(),
        }
    }

    /// Create a new RGA with deterministic ID (internal)
    pub(super) fn new_with_field_name_internal(
        parent_id: Option<crate::address::Id>,
        field_name: &str,
    ) -> Self {
        Self {
            chars: UnorderedMap::new_with_field_name_and_crdt_type(
                parent_id,
                field_name,
                CrdtType::Rga,
            ),
        }
    }

    /// Reassigns the RGA's ID to a deterministic ID based on field name.
    ///
    /// This is called by the `#[app::state]` macro after `init()` returns to ensure
    /// all top-level collections have deterministic IDs regardless of how they were
    /// created in `init()`.
    ///
    /// # Arguments
    /// * `field_name` - The name of the struct field containing this RGA
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        self.chars.reassign_deterministic_id(field_name);
        self.chars.set_collection_crdt_type(CrdtType::Rga);
    }

    /// Insert a character at the given visible position
    ///
    /// # Panics
    /// Panics if called inside a state migration (`#[app::migrate]`, i.e. storage
    /// merge mode): `insert` stamps a node-local HLC timestamp, minting a different
    /// `CharId` on every node and diverging the network. Carry the RGA across
    /// unchanged or seed with [`insert_str_at_timestamp`](Self::insert_str_at_timestamp).
    ///
    /// # Errors
    ///
    /// Returns error if position is out of bounds or storage operation fails
    #[expect(
        clippy::panic,
        reason = "non-deterministic during migrate (node-local HLC); a loud panic is the \
                  intended, unmissable guard against a silent network divergence"
    )]
    pub fn insert(&mut self, pos: usize, content: char) -> Result<(), StoreError> {
        if env::in_merge_mode() {
            panic!(
                "ReplicatedGrowableArray::insert() is non-deterministic during a state \
                 migration: it stamps a node-local timestamp, minting a different CharId \
                 per node and diverging the network. Carry the RGA across unchanged or \
                 seed with `insert_str_at_timestamp(pos, fixed_timestamp, s)`."
            );
        }
        // Register this RGA type's nested-id re-key thunk so an RGA stored as a
        // collection value is re-keyed when the outer collection is stored.
        super::rekey::register_rekey::<Self>();
        let timestamp = env::hlc_timestamp();

        // Find the left neighbor at the visible position
        let ordered = self.get_ordered_chars()?;

        let left = if pos == 0 {
            CharId::root()
        } else if pos <= ordered.len() {
            ordered
                .get(pos - 1)
                .map(|(id, _)| *id)
                .ok_or(StoreError::StorageError(
                    crate::interface::StorageError::InvalidData("position out of bounds".into()),
                ))?
        } else {
            return Err(StoreError::StorageError(
                crate::interface::StorageError::InvalidData("position out of bounds".into()),
            ));
        };

        let char_id = CharId::new(timestamp, 0);
        let new_char = RgaChar::new(content, left);

        let _ = self.chars.insert(CharKey::new(char_id), new_char)?;

        Ok(())
    }

    /// Delete the character at the given visible position
    ///
    /// # Errors
    ///
    /// Returns error if position is out of bounds or storage operation fails
    pub fn delete(&mut self, pos: usize) -> Result<(), StoreError> {
        let ordered = self.get_ordered_chars()?;

        let (char_id, _) = ordered.get(pos).ok_or(StoreError::StorageError(
            crate::interface::StorageError::InvalidData("position out of bounds".into()),
        ))?;

        let _ = self.chars.remove(&CharKey::new(*char_id))?;

        Ok(())
    }

    /// Get the current text (excluding deleted characters)
    ///
    /// # Errors
    ///
    /// Returns error if storage operation fails
    pub fn get_text(&self) -> Result<String, StoreError> {
        let ordered = self.get_ordered_chars()?;
        Ok(ordered.iter().map(|(_, c)| c.as_char()).collect())
    }

    /// Get the length of visible text (excluding deleted characters)
    ///
    /// # Errors
    ///
    /// Returns error if storage operation fails
    pub fn len(&self) -> Result<usize, StoreError> {
        self.get_ordered_chars().map(|chars| chars.len())
    }

    /// Check if the text is empty
    ///
    /// # Errors
    ///
    /// Returns error if storage operation fails
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        self.len().map(|len| len == 0)
    }

    /// Insert multiple characters at once (more efficient for strings).
    ///
    /// Allocates a fresh HLC timestamp from the environment; for tests that
    /// need byte-for-byte reproducibility across replicas, see
    /// [`insert_str_at_timestamp`](Self::insert_str_at_timestamp).
    ///
    /// # Panics
    /// Panics if called inside a state migration (`#[app::migrate]`, i.e. storage
    /// merge mode) — it allocates a node-local HLC timestamp. Seed with
    /// [`insert_str_at_timestamp`](Self::insert_str_at_timestamp) instead.
    ///
    /// # Errors
    ///
    /// Returns error if position is out of bounds or storage operation fails
    #[expect(
        clippy::panic,
        reason = "non-deterministic during migrate (node-local HLC); a loud panic is the \
                  intended, unmissable guard against a silent network divergence"
    )]
    pub fn insert_str(&mut self, pos: usize, s: &str) -> Result<(), StoreError> {
        if env::in_merge_mode() {
            panic!(
                "ReplicatedGrowableArray::insert_str() is non-deterministic during a state \
                 migration: it allocates a node-local HLC timestamp, diverging CharIds \
                 across nodes. Seed with `insert_str_at_timestamp(pos, fixed_timestamp, s)`."
            );
        }
        let timestamp = env::hlc_timestamp();
        self.insert_str_at_timestamp(pos, timestamp, s)
    }

    /// Insert multiple characters at `pos` using `timestamp` as the HLC
    /// component of every new `CharId`.
    ///
    /// Identical to [`insert_str`](Self::insert_str) except the HLC
    /// timestamp is supplied by the caller rather than read from
    /// `env::hlc_timestamp()`. With a fixed `timestamp` the resulting
    /// `CharId` set is fully deterministic, which is what test fixtures
    /// (and the CRDT contract tests in `tests/crdt_contract.rs`) need to
    /// satisfy the determinism requirement of the structural-equality
    /// laws — two `make_a()` calls in `assert_mergeable_laws` must
    /// produce identical character sets, which the HLC-driven path
    /// can't guarantee because each call advances the clock.
    ///
    /// Production callers should prefer `insert_str` — supplying your
    /// own timestamp from outside the HLC chain bypasses the causal
    /// ordering guarantees the HLC provides between concurrent
    /// inserters.
    ///
    /// # Precondition: timestamp uniqueness per RGA
    ///
    /// Each character's `CharId` is `(timestamp, seq)` where `seq` is the
    /// byte offset within the inserted string (always starting at 0). If a
    /// caller invokes this method twice on the same `ReplicatedGrowableArray`
    /// with the same `timestamp`, the second call's characters will collide
    /// with the first's on `(timestamp, 0..)` and silently overwrite them in
    /// the underlying `chars` map. Live `insert_str` is safe by HLC
    /// monotonicity — every call gets a fresh logical timestamp. Test
    /// fixtures that want to insert multiple times on the same RGA must use
    /// distinct timestamps per call (the contract test in
    /// `tests/crdt_contract.rs` does exactly one insert per replica).
    ///
    /// # Errors
    ///
    /// Returns error if position is out of bounds or storage operation fails
    pub fn insert_str_at_timestamp(
        &mut self,
        pos: usize,
        timestamp: crate::logical_clock::HybridTimestamp,
        s: &str,
    ) -> Result<(), StoreError> {
        // Find the left neighbor
        let ordered = self.get_ordered_chars()?;
        let mut left = if pos == 0 {
            CharId::root()
        } else if pos <= ordered.len() {
            ordered
                .get(pos - 1)
                .map(|(id, _)| *id)
                .ok_or(StoreError::StorageError(
                    crate::interface::StorageError::InvalidData("position out of bounds".into()),
                ))?
        } else {
            return Err(StoreError::StorageError(
                crate::interface::StorageError::InvalidData("position out of bounds".into()),
            ));
        };

        // Insert each character
        for (seq, content) in s.chars().enumerate() {
            let char_id = CharId::new(timestamp, seq as u32);
            let new_char = RgaChar::new(content, left);

            let _ = self.chars.insert(CharKey::new(char_id), new_char)?;

            // Next char's left is this char
            left = char_id;
        }

        Ok(())
    }

    /// Delete a range of characters
    ///
    /// This operation is idempotent - if the range exceeds the current document length,
    /// it deletes up to the end of the document without error. This ensures delete
    /// operations can be safely applied even when received out of order during sync.
    ///
    /// # Errors
    ///
    /// Returns error if start > end or storage operation fails
    pub fn delete_range(&mut self, start: usize, end: usize) -> Result<(), StoreError> {
        if start > end {
            return Err(StoreError::StorageError(
                crate::interface::StorageError::InvalidData("start must be <= end".into()),
            ));
        }

        let ordered = self.get_ordered_chars()?;

        // Clamp end to the actual length - makes delete idempotent
        // This prevents "out of bounds" errors when operations arrive out of order
        let actual_end = end.min(ordered.len());

        // Delete each character in range (may be empty if start >= ordered.len())
        for (char_id, _) in &ordered[start..actual_end] {
            let _ = self.chars.remove(&CharKey::new(*char_id))?;
        }

        Ok(())
    }

    /// Tombstone-aware RGA character merge (the root-level blob / full-state
    /// conflict path; see `crdt_impls`'s `Mergeable` impl).
    ///
    /// Copies each char from `other` into `self` unless `self` already holds it
    /// live or has tombstoned it. A char `self` concurrently deleted stays
    /// deleted — delete wins, like the `DeleteRef` LWW path. RGA chars are
    /// immutable, so the only conflict is presence-vs-tombstone (no
    /// update-vs-delete race). Still commutative/associative/idempotent: a char
    /// is live iff live somewhere and tombstoned nowhere, independent of merge
    /// order. The prior `is_none()`-only gate resurrected deleted chars (#D2).
    pub(crate) fn merge_chars_from<S2: StorageAdaptor>(
        &mut self,
        other: &ReplicatedGrowableArray<S2>,
    ) -> Result<(), StoreError> {
        let other_chars = other.chars.entries()?;

        for (key, char_data) in other_chars {
            // Propagate a read error instead of swallowing it: `.ok().flatten()`
            // would treat a transient storage failure as "char absent" and
            // re-insert, corrupting the array. A genuine absence is `Ok(None)`.
            if self.chars.get(&key)?.is_some() {
                // Live in both — keep ours (chars are immutable, so identical).
                continue;
            }

            // Absent from `self`'s live set: distinguish "never seen" from
            // "concurrently deleted". A tombstone means `self` deleted this
            // char; delete wins, so do NOT resurrect it.
            let entry_id = self.chars.entry_id(&key);
            if crate::index::Index::<S>::is_deleted(entry_id)? {
                continue;
            }

            // Genuinely new char from `other` — add it (add-wins).
            let _ = self.chars.insert(key, char_data)?;
        }

        Ok(())
    }

    // Helper: Get all characters in RGA order (excludes deleted automatically via UnorderedMap)
    //
    // Linearizes the RGA into document order. The result must be a pure
    // function of the character set — `(CharId, content, left)` for every
    // live char — and nothing else. In particular it must NOT depend on
    // the order `self.chars.entries()` happens to yield, because that is
    // storage-iteration order and differs between replicas that hold the
    // same logical set (insertion history, compaction). Two replicas with
    // an identical character set therefore produce identical text — the
    // property that lets the synced Merkle root (which hashes the same
    // set) agree with what `get_text` returns. The previous linear walk
    // broke this twice: its gap fallback picked "any unplaced char" in
    // `entries()` order (replica-divergent), and the single-pointer walk
    // couldn't place sibling subtrees of a branching node in order.
    //
    // Algorithm: standard RGA pre-order DFS over the tree induced by
    // `left` edges. Siblings sharing a `left` origin are ordered by
    // descending `CharId` (a later HLC at the same position sorts first,
    // so sequential mid-document inserts land before the existing
    // right-neighbour). Each node's whole subtree is emitted before its
    // next sibling.
    fn get_ordered_chars(&self) -> Result<Vec<(CharId, RgaChar)>, StoreError> {
        use std::collections::{BTreeMap, BTreeSet};

        // Get all non-deleted characters from UnorderedMap
        let chars: Vec<(CharId, RgaChar)> = self
            .chars
            .entries()?
            .map(|(key, char)| (key.id(), char))
            .collect();

        if chars.is_empty() {
            return Ok(Vec::new());
        }

        // Group children by their `left` origin. `BTreeMap` keys + the
        // per-bucket sort below make the linearization independent of
        // `entries()` order.
        let mut children_by_left: BTreeMap<CharId, Vec<(CharId, RgaChar)>> = BTreeMap::new();
        let present: BTreeSet<CharId> = chars.iter().map(|(id, _)| *id).collect();
        for (id, c) in chars {
            children_by_left.entry(c.left).or_default().push((id, c));
        }
        // Within each sibling group, highest CharId first (RGA tie-break).
        for bucket in children_by_left.values_mut() {
            bucket.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
        }

        // Traversal forest roots: the document root sentinel, plus any
        // dangling origin (a `left` that references a char not in the set
        // — a causal-delivery gap). Dangling origins are visited in
        // ascending id order so the output stays deterministic even when
        // the tree is malformed.
        let mut roots: Vec<CharId> = vec![CharId::root()];
        roots.extend(
            children_by_left
                .keys()
                .copied()
                .filter(|left| *left != CharId::root() && !present.contains(left)),
        );

        // Iterative pre-order DFS. Seed the stack with the forest's
        // top-level children in reverse emit order so the first one pops
        // first; on each pop, push the node's children in reverse so the
        // highest-id sibling is processed (and its subtree fully emitted)
        // before the next.
        let mut stack: Vec<(CharId, RgaChar)> = roots
            .iter()
            .filter_map(|origin| children_by_left.get(origin))
            .flat_map(|bucket| bucket.iter().cloned())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let mut ordered = Vec::with_capacity(present.len());
        let mut emitted: BTreeSet<CharId> = BTreeSet::new();
        while let Some((id, c)) = stack.pop() {
            // Guard against pathological cycles in `left` edges.
            if !emitted.insert(id) {
                continue;
            }
            ordered.push((id, c));
            if let Some(bucket) = children_by_left.get(&id) {
                for child in bucket.iter().rev() {
                    stack.push(child.clone());
                }
            }
        }

        Ok(ordered)
    }
}

#[cfg(test)]
mod merge_mode_tests {
    use super::ReplicatedGrowableArray;
    use crate::collections::Root;
    use crate::env;
    use crate::logical_clock::HybridTimestamp;

    #[test]
    #[should_panic(expected = "migration")]
    fn insert_panics_during_migration() {
        env::reset_for_testing();
        let mut rga = Root::new(ReplicatedGrowableArray::new);
        // `insert()` stamps a node-local HLC; inside a migrate body that mints
        // a different CharId on every node and diverges. It must refuse.
        env::with_merge_mode(|| {
            let _ = rga.insert(0, 'H');
        });
    }

    #[test]
    #[should_panic(expected = "migration")]
    fn insert_str_panics_during_migration() {
        env::reset_for_testing();
        let mut rga = Root::new(ReplicatedGrowableArray::new);
        env::with_merge_mode(|| {
            let _ = rga.insert_str(0, "Hi");
        });
    }

    #[test]
    fn insert_str_at_timestamp_is_allowed_during_migration() {
        env::reset_for_testing();
        let mut rga = Root::new(ReplicatedGrowableArray::new);
        // The deterministic replay API (explicit, input-derived timestamp) is
        // the sanctioned way to seed an RGA in a migrate — it stays usable.
        env::with_merge_mode(|| {
            rga.insert_str_at_timestamp(0, HybridTimestamp::zero(), "H")
                .unwrap();
        });
        assert_eq!(rga.len().unwrap(), 1);
    }
}

/// D2 — tombstone-aware blob merge, across two isolated storage scopes so the
/// deleter and the holder have separate stores (as two replicas do). A shared
/// store would mask the bug: `other.chars.entries()` would read the deleter's
/// own post-delete child list.
#[cfg(test)]
mod tombstone_merge_tests {
    use super::ReplicatedGrowableArray;
    use crate::collections::UnorderedMap;
    use crate::index::Index;
    use crate::logical_clock::{HybridTimestamp, Timestamp, NTP64};
    use crate::store::StorageAdaptor;

    /// Non-zero HLC at physical tick `tick`, so a `CharId` never collides with
    /// the root sentinel `(HybridTimestamp::default(), 0)`.
    fn ts_at(tick: u64) -> HybridTimestamp {
        let id = *HybridTimestamp::zero().get_id();
        HybridTimestamp::new(Timestamp::new(NTP64(tick << 32), id))
    }

    /// Build a generic-`S` RGA whose `chars` map has a deterministic id (same
    /// across scopes for the same `field_name`), so two replicas share CharIds.
    fn rga_in<S: StorageAdaptor>(field_name: &str) -> ReplicatedGrowableArray<S> {
        ReplicatedGrowableArray {
            chars: UnorderedMap::new_with_field_name_and_crdt_type(
                None,
                field_name,
                crate::collections::CrdtType::Rga,
            ),
        }
    }

    #[test]
    fn blob_merge_does_not_resurrect_concurrently_deleted_char() {
        // Two isolated stores standing in for two replicas.
        type A = crate::store::MockedStorage<811>;
        type B = crate::store::MockedStorage<812>;

        // Deterministic non-zero timestamp → identical CharIds for "Hi" in both
        // replicas (and distinct from the root sentinel).
        let ts = ts_at(1);

        // Replica A: seed "Hi", then delete 'H' (tombstones that char entity).
        let mut a = rga_in::<A>("content");
        a.insert_str_at_timestamp(0, ts, "Hi").unwrap();
        assert_eq!(a.get_text().unwrap(), "Hi");
        a.delete(0).unwrap(); // delete 'H'
        assert_eq!(a.get_text().unwrap(), "i");

        // Replica B: seed the SAME "Hi" (identical CharIds), keep all live.
        let mut b = rga_in::<B>("content");
        b.insert_str_at_timestamp(0, ts, "Hi").unwrap();
        assert_eq!(b.get_text().unwrap(), "Hi");

        // Sanity: A genuinely tombstoned the 'H' char entity, and B still holds
        // the SAME entity id live — i.e. the resurrection scenario is real.
        let h_key = {
            // The first char's id is (ts, seq=0); reconstruct its entry id.
            let id = super::CharId::new(ts, 0);
            super::CharKey::new(id)
        };
        let a_entry = a.chars.entry_id(&h_key);
        let b_entry = b.chars.entry_id(&h_key);
        assert_eq!(a_entry, b_entry, "replicas must share the char entity id");
        assert!(
            Index::<A>::is_deleted(a_entry).unwrap(),
            "'H' must be tombstoned on replica A"
        );
        assert!(
            !Index::<B>::is_deleted(b_entry).unwrap(),
            "'H' must be live on replica B"
        );

        // Resurrection is observable on the sync wire (parent `children` /
        // `deleted_children` / `full_hash`), not in `get_text` — `find_by_id`
        // filters tombstoned ids, so a resurrected char is silently dropped by
        // the map iterator. The lost tombstone + diverged hash is the damage.
        let parent = Index::<A>::get_parent_id(a_entry).unwrap().unwrap();
        let pre = Index::<A>::get_index(parent).unwrap().unwrap();
        let pre_hash = pre.full_hash();
        assert!(
            pre.deleted_children().contains(&a_entry),
            "precondition: 'H' must be advertised as deleted before the merge"
        );
        assert!(
            pre.children()
                .map(|c| c.iter().all(|ci| ci.id() != a_entry))
                .unwrap_or(true),
            "precondition: 'H' must NOT be a live child before the merge"
        );

        // Merge B (live 'H') INTO A (deleted 'H'). Delete must win — the blob
        // merge must NOT resurrect the char A concurrently deleted.
        a.merge_chars_from(&b).unwrap();

        let post = Index::<A>::get_index(parent).unwrap().unwrap();
        assert!(
            Index::<A>::is_deleted(a_entry).unwrap(),
            "blob merge un-tombstoned the concurrently-deleted 'H' (D2)"
        );
        assert!(
            post.deleted_children().contains(&a_entry),
            "blob merge dropped 'H' from the wire tombstone advertisement (D2)"
        );
        assert!(
            post.children()
                .map(|c| c.iter().all(|ci| ci.id() != a_entry))
                .unwrap_or(true),
            "blob merge re-added 'H' as a live child — resurrection (D2)"
        );
        assert_eq!(
            post.full_hash(),
            pre_hash,
            "blob merge changed the chars-map hash, diverging from peers that \
             saw the delete (D2)"
        );
        assert_eq!(
            a.get_text().unwrap(),
            "i",
            "visible text must stay 'i' after the merge"
        );

        // Idempotent: re-merging the same live 'H' must leave A unchanged.
        a.merge_chars_from(&b).unwrap();
        assert_eq!(
            Index::<A>::get_index(parent).unwrap().unwrap().full_hash(),
            pre_hash,
            "repeated blob merge must remain idempotent (delete still wins)"
        );

        // A deletion propagates B→A via the DeleteRef path (which carries the
        // tombstone on the wire), not via this blob merge — `entries()` yields
        // only live chars. The blob-merge fix is scoped to non-resurrection;
        // convergence of the delete is covered by the delta-sync tests.
    }

    #[test]
    fn blob_merge_still_adds_genuinely_new_chars() {
        // The tombstone guard must NOT suppress add-wins for chars `self` has
        // simply never seen.
        type A = crate::store::MockedStorage<813>;
        type B = crate::store::MockedStorage<814>;

        let ts = ts_at(1);
        let mut a = rga_in::<A>("doc");
        a.insert_str_at_timestamp(0, ts, "Hi").unwrap();

        let mut b = rga_in::<B>("doc");
        b.insert_str_at_timestamp(0, ts, "Hi").unwrap();
        // B appends a genuinely new char at a strictly later timestamp.
        b.insert_str_at_timestamp(2, ts_at(2), "!").unwrap();
        assert_eq!(b.get_text().unwrap(), "Hi!");

        a.merge_chars_from(&b).unwrap();
        assert_eq!(
            a.get_text().unwrap(),
            "Hi!",
            "merge must add the genuinely-new '!' char (add-wins still holds)"
        );
    }
}
