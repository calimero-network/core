//! A snapshot staged in a column of its own, checked there as a tree, and only
//! then moved into the context's state, so one that fails leaves it as it was.

use std::collections::{BTreeMap, HashMap, HashSet};

use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_storage::address::Id;
use calimero_storage::entities::{ChildInfo, StorageType};
use calimero_storage::index::{EntityIndex, Index};
use calimero_storage::store::{Key as StorageKey, MainStorage};
use calimero_store::db::Column;
use calimero_store::key::{ContextState as ContextStateKey, STATE_KEY_LEN};
use calimero_store::slice::Slice;
use calimero_store::types::ContextState as ContextStateValue;
use calimero_store::{Store, StoreBatch};
use eyre::Result;
use tracing::{error, warn};

use super::child_trie_writes;

const SCAN_PAGE: usize = 1024; // staged rows read, and moved, per pass

/// The staging area of one context's snapshot install.
///
/// Dropping it clears the area, so a refused snapshot leaves nothing behind.
pub(super) struct Stage {
    store: Store,
    context_id: ContextId,
}

/// Where a staged entity's parent chain ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
    Root,
    Detached,
}

impl Stage {
    /// The staging area for `context_id`, cleared of anything a crashed
    /// attempt left.
    pub(super) fn open(store: Store, context_id: ContextId) -> Result<Self> {
        let stage = Self { store, context_id };
        stage.clear()?;
        Ok(stage)
    }

    pub(super) fn store(&self) -> &Store {
        &self.store
    }

    pub(super) fn context_id(&self) -> ContextId {
        self.context_id
    }

    fn clear(&self) -> Result<()> {
        self.store
            .raw_delete_prefix(Column::SnapshotStage, self.context_id.as_ref())
    }

    fn key(&self, state_key: [u8; STATE_KEY_LEN]) -> Vec<u8> {
        [self.context_id.as_ref(), &state_key[..]].concat()
    }

    fn get(&self, key: StorageKey) -> Result<Option<Vec<u8>>> {
        self.store
            .raw_get(Column::SnapshotStage, &self.key(key.to_bytes()))
    }

    /// A row for the trie and hash folds, which read through `Option`: a row
    /// that cannot be read folds to a mismatch, which refuses the snapshot.
    fn fold_row(&self, key: StorageKey) -> Option<Vec<u8>> {
        self.get(key).ok().flatten()
    }

    fn put(&self, key: StorageKey, value: &[u8]) -> Result<()> {
        self.store
            .raw_put(Column::SnapshotStage, &self.key(key.to_bytes()), value)
    }

    /// Stages an entity's row: its index record and its data, as the storage
    /// layer keeps them.
    pub(super) fn put_entity(&self, id: Id, entry: &[u8], index: &[u8]) -> Result<()> {
        let row = calimero_storage::row::encode(
            id,
            &calimero_storage::row::Row {
                index: Some(index.to_vec()),
                data: Some(entry.to_vec()),
            },
        );
        self.put(StorageKey::Index(id), &row)
    }

    /// The staged row of `id`, as `(entry, index)`.
    pub(super) fn entity(&self, id: Id) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        let Some(bytes) = self.get(StorageKey::Index(id))? else {
            return Ok(None);
        };
        Ok(calimero_storage::row::decode(id, &bytes).and_then(|row| Some((row.data?, row.index?))))
    }

    fn index_of(&self, id: Id) -> Result<Option<EntityIndex>> {
        let Some(bytes) = self.get(StorageKey::Index(id))? else {
            return Ok(None);
        };
        Ok(calimero_storage::row::decode(id, &bytes).and_then(|row| row.entity_index()))
    }

    /// Calls `visit` with every staged `(state key, row)`, a page at a time.
    fn scan(
        &self,
        mut visit: impl FnMut([u8; STATE_KEY_LEN], Vec<u8>) -> Result<()>,
    ) -> Result<()> {
        let prefix = self.context_id.as_ref();
        let hi = [prefix, &[0xFF; STATE_KEY_LEN + 1][..]].concat();
        let mut lo = prefix.to_vec();
        loop {
            let page = self
                .store
                .raw_scan(Column::SnapshotStage, &lo, &hi, Some(SCAN_PAGE))?;
            let Some((last, _)) = page.last() else {
                return Ok(());
            };
            lo = [last.as_slice(), &[0]].concat();
            let full = page.len() == SCAN_PAGE;
            for (key, value) in page {
                let state_key = key[prefix.len()..]
                    .try_into()
                    .map_err(|_| eyre::eyre!("snapshot stage: a key of the wrong length"))?;
                visit(state_key, value)?;
            }
            if !full {
                return Ok(());
            }
        }
    }

    /// Calls `visit` with every staged entity and its index row.
    fn entities(&self, mut visit: impl FnMut(Id, EntityIndex) -> Result<()>) -> Result<()> {
        self.scan(|state_key, value| {
            let Some(StorageKey::Index(id)) = StorageKey::from_bytes(&state_key) else {
                return Ok(());
            };
            let index = calimero_storage::row::decode(id, &value)
                .and_then(|row| row.entity_index())
                .ok_or_else(|| {
                    eyre::eyre!(
                        "snapshot stage: entity {:?} has no index row",
                        id.as_bytes()
                    )
                })?;
            visit(id, index)
        })
    }

    /// Checks the staged tree against `claimed`: every entity's children fold
    /// to the `full_hash` it ships, no parent chain loops, and the root folds
    /// to the claim.
    ///
    /// Returns the unsigned entities the root does not reach: nothing binds
    /// them to the claim, so they are not installed.
    pub(super) fn verify(&self, claimed: Hash) -> Result<HashSet<Id>> {
        let root = Id::new(*self.context_id);
        self.link_children()?;

        let mut staged = 0_usize;
        self.entities(|id, index| {
            staged += 1;
            let folded = Index::<MainStorage>::full_hash_with(id, index.own_hash(), |key| {
                self.fold_row(key)
            })
            .ok_or_else(|| {
                eyre::eyre!(
                    "snapshot entity {:?}: its child rows are unreadable",
                    id.as_bytes()
                )
            })?;
            if folded != index.full_hash() {
                warn!(
                    context_id = %self.context_id,
                    id = ?id.as_bytes(),
                    parent = ?index.parent_id().map(|parent| *parent.as_bytes()),
                    computed = %Hash::from(folded),
                    shipped = %Hash::from(index.full_hash()),
                    "snapshot entity does not fold to the hash it ships"
                );
                eyre::bail!(
                    "snapshot entity {:?}: its children do not fold to the hash it ships",
                    id.as_bytes()
                );
            }
            Ok(())
        })?;

        if staged == 0 {
            // The state of an empty context is the only one with no root.
            eyre::ensure!(
                *claimed == [0; 32],
                "snapshot verify: no entities, but the boundary names root {claimed}"
            );
            return Ok(HashSet::new());
        }
        let Some(top) = self.index_of(root)? else {
            eyre::bail!("snapshot verify: the snapshot holds no root entity");
        };
        eyre::ensure!(
            top.parent_id().is_none(),
            "snapshot verify: the root entity names a parent"
        );
        eyre::ensure!(
            top.full_hash() == *claimed,
            "snapshot verify: the tree folds to {}, not the boundary {claimed}",
            Hash::from(top.full_hash())
        );

        self.unreachable_public(root)
    }

    /// Links every staged entity into its parent's staged child trie.
    fn link_children(&self) -> Result<()> {
        let mut by_parent: BTreeMap<Id, Vec<ChildInfo>> = BTreeMap::new();
        self.entities(|id, index| {
            if let Some(parent) = index.parent_id() {
                by_parent.entry(parent).or_default().push(ChildInfo::new(
                    id,
                    index.full_hash(),
                    index.metadata.clone(),
                ));
            }
            Ok(())
        })?;
        for (parent, children) in by_parent {
            for (key, bytes) in child_trie_writes(|key| self.fold_row(key), parent, children) {
                self.put(key, &bytes)?;
            }
        }
        Ok(())
    }

    /// The `Public` entities whose parent chain does not end at `root`.
    fn unreachable_public(&self, root: Id) -> Result<HashSet<Id>> {
        let mut reach: HashMap<Id, Reach> = HashMap::new();
        let mut unreachable = HashSet::new();
        self.entities(|id, index| {
            if self.reach(root, id, &mut reach)? == Reach::Detached
                && matches!(index.metadata.storage_type, StorageType::Public)
            {
                let _new = unreachable.insert(id);
            }
            Ok(())
        })?;
        Ok(unreachable)
    }

    /// Where the parent chain of `start` ends. Errors on a chain that loops.
    fn reach(&self, root: Id, start: Id, known: &mut HashMap<Id, Reach>) -> Result<Reach> {
        let mut path: Vec<Id> = Vec::new();
        let mut current = start;
        let end = loop {
            if let Some(end) = known.get(&current) {
                break *end;
            }
            if path.contains(&current) {
                eyre::bail!(
                    "snapshot entity {:?}: its parent chain loops",
                    current.as_bytes()
                );
            }
            path.push(current);
            match self.index_of(current)?.and_then(|index| index.parent_id()) {
                Some(parent) if self.index_of(parent)?.is_some() => current = parent,
                Some(_) => break Reach::Detached,
                None if current == root => break Reach::Root,
                None => break Reach::Detached,
            }
        };
        for id in path {
            let _previous = known.insert(id, end);
        }
        Ok(end)
    }

    /// Moves the staged entities into the context's state, except `skip`, and
    /// deletes the keys of `replaced` the snapshot does not carry.
    ///
    /// Not atomic: the caller sets the sync-in-progress marker first, so a crash
    /// part way is recovered by the next attempt. Returns how many were moved.
    pub(super) fn promote(
        &self,
        replaced: &HashSet<[u8; STATE_KEY_LEN]>,
        skip: &HashSet<Id>,
    ) -> Result<usize> {
        let mut moved = 0;
        let mut batch = StoreBatch::new(&self.store);
        self.scan(|state_key, row| {
            let Some(StorageKey::Index(id)) = StorageKey::from_bytes(&state_key) else {
                return Ok(());
            };
            if skip.contains(&id) {
                return Ok(());
            }
            let _batch = batch.put(
                &ContextStateKey::new(self.context_id, state_key),
                &ContextStateValue::from(Slice::from(row)),
            )?;
            moved += 1;
            if batch.len() >= SCAN_PAGE {
                std::mem::replace(&mut batch, StoreBatch::new(&self.store)).commit()?;
            }
            Ok(())
        })?;
        batch.commit()?;

        let mut batch = StoreBatch::new(&self.store);
        for stale in replaced {
            let carried = match StorageKey::from_bytes(stale) {
                Some(StorageKey::Index(id)) if !skip.contains(&id) => {
                    self.get(StorageKey::Index(id))?.is_some()
                }
                _ => false,
            };
            if carried {
                continue;
            }
            let _batch = batch.delete(&ContextStateKey::new(self.context_id, *stale))?;
            if batch.len() >= SCAN_PAGE {
                std::mem::replace(&mut batch, StoreBatch::new(&self.store)).commit()?;
            }
        }
        batch.commit()?;
        Ok(moved)
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        if let Err(e) = self.clear() {
            error!(context_id = %self.context_id, error = %e, "failed to clear the snapshot staging area");
        }
    }
}
