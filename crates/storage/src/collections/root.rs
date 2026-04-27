//! A root collection that stores a single value.

use core::fmt;
use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::ptr;

use super::{Collection, ROOT_ID};
use crate::address::Id;
use crate::delta::{push_comparison, StorageDelta};
use crate::index::DeferredAncestorScope;
use crate::integration::Comparison;
use crate::interface::{Action, Interface, StorageError};
use crate::store::{MainStorage, StorageAdaptor};
use borsh::{from_slice, BorshDeserialize, BorshSerialize};
use tracing::info;

/// A set collection that stores unqiue values once.
pub struct Root<T, S: StorageAdaptor = MainStorage> {
    inner: Collection<T, S>,
    value: RefCell<Option<T>>,
    dirty: bool,
}

impl<T, S> fmt::Debug for Root<T, S>
where
    T: BorshSerialize + BorshDeserialize + fmt::Debug,
    S: StorageAdaptor,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Root")
            .field("inner", &self.inner)
            .field("value", &self.value)
            .field("dirty", &self.dirty)
            .finish()
    }
}

impl<T> Root<T, MainStorage>
where
    T: BorshSerialize + BorshDeserialize,
{
    /// Creates a new root collection with the given value.
    pub fn new<F: FnOnce() -> T>(f: F) -> Self {
        Self::new_internal(f)
    }
}

impl<T, S> Root<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Creates a new root collection with the given value.
    #[expect(clippy::unwrap_used, reason = "fatal error if it happens")]
    pub fn new_internal<F: FnOnce() -> T>(f: F) -> Self {
        let mut inner = Collection::new(Some(*ROOT_ID));

        let id = Self::entry_id();

        let value = inner.insert(Some(id), f()).unwrap();

        Self {
            inner,
            dirty: false,
            value: RefCell::new(Some(value)),
        }
    }

    fn entry_id() -> Id {
        Id::new([118; 32])
    }

    #[expect(clippy::mut_from_ref, reason = "'tis fine")]
    #[expect(clippy::unwrap_used, reason = "fatal error if it happens")]
    fn get(&self) -> &mut T {
        let mut value = self.value.borrow_mut();

        let id = Self::entry_id();

        let value = value.get_or_insert_with(|| self.inner.get(id).unwrap().unwrap());

        #[expect(unsafe_code, reason = "necessary for caching")]
        let value = unsafe { &mut *ptr::from_mut(value) };

        value
    }

    /// Fetches the root collection.
    #[expect(
        clippy::unwrap_used,
        clippy::unwrap_in_result,
        reason = "fatal error if it happens"
    )]
    pub fn fetch() -> Option<Self> {
        let inner = <Interface<S>>::root().unwrap()?;

        Some(Self {
            inner,
            dirty: false,
            value: RefCell::new(None),
        })
    }

    /// Commits the root collection.
    #[expect(clippy::unwrap_used, reason = "fatal error if it happens")]
    pub fn commit(mut self) {
        if self.dirty {
            if let Some(value) = self.value.into_inner() {
                if let Some(mut entry) = self.inner.get_mut(Self::entry_id()).unwrap() {
                    *entry = value;
                }
            }
        }

        <Interface<S>>::commit_root(Some(self.inner)).unwrap();
    }

    /// Commits the root collection without an instance of the root state.
    #[expect(clippy::unwrap_used, reason = "fatal error if it happens")]
    pub fn commit_headless() {
        <Interface<S>>::commit_root::<Collection<T>>(None).unwrap();
    }

    /// Syncs the root collection.
    ///
    /// `ctx` is the apply-time context for the inner [`Interface::apply_action`]
    /// calls. Per #2266, the [`StorageDelta::CausalActions`] variant
    /// carries pre-resolved `effective_writers` per Shared entity from
    /// the sync layer; this function builds a per-action `ApplyContext`
    /// from that map and ignores the passed `ctx` for that variant. For
    /// [`StorageDelta::Actions`] the passed `ctx` is used as-is (typically
    /// empty — verifier falls back to v2 stored-writers).
    #[expect(clippy::missing_errors_doc, reason = "NO")]
    pub fn sync(args: &[u8], ctx: crate::interface::ApplyContext) -> Result<(), StorageError> {
        let artifact =
            from_slice::<StorageDelta>(args).map_err(StorageError::DeserializationError)?;

        match artifact {
            StorageDelta::Actions(actions) => {
                Self::apply_actions(actions, |_| ctx.clone())?;
            }
            StorageDelta::CausalActions {
                actions,
                delta_id,
                delta_hlc,
                effective_writers,
            } => {
                Self::apply_actions(actions, |action| crate::interface::ApplyContext {
                    effective_writers: effective_writers.get(&action.id()).cloned(),
                    delta_id: Some(delta_id),
                    delta_hlc: Some(delta_hlc),
                })?;
            }
            StorageDelta::Comparisons(comparisons) => {
                if comparisons.is_empty() {
                    push_comparison(Comparison {
                        data: <Interface<S>>::find_by_id_raw(Id::root()),
                        comparison_data: <Interface<S>>::generate_comparison_data(None)?,
                    });
                }

                for Comparison {
                    data,
                    comparison_data,
                } in comparisons
                {
                    <Interface<S>>::compare_affective(data, comparison_data, &ctx)?;
                }
            }
        }

        info!(
            target: "storage::root",
            "committing root after delta replay"
        );
        Self::commit_headless();

        Ok(())
    }

    /// Apply a batch of actions, deriving the per-action [`ApplyContext`]
    /// from `ctx_for_action`. Shared between the `Actions` (uniform ctx)
    /// and `CausalActions` (per-action ctx via `effective_writers` lookup)
    /// branches of [`Self::sync`].
    fn apply_actions<F>(actions: Vec<Action>, ctx_for_action: F) -> Result<(), StorageError>
    where
        F: Fn(&Action) -> crate::interface::ApplyContext,
    {
        let mut root_snapshot: Option<(Vec<u8>, crate::entities::Metadata)> = None;

        // #2238: defer ancestor-hash recomputation until the end of
        // the action loop. Many deltas in a single merge often touch
        // the same parent; without batching, each `add_child_to`
        // walks from that parent to the root redoing the same O(K)
        // hash work. The scope dedupes starting ids and flushes via
        // `finish()?` after the loop so storage errors during flush
        // propagate back to the caller (an error here means the
        // merkle tree is left inconsistent).
        let defer_scope = DeferredAncestorScope::<S>::new();

        for action in actions {
            match &action {
                Action::Add {
                    id, data, metadata, ..
                }
                | Action::Update {
                    id, data, metadata, ..
                } if id.is_root() => {
                    info!(
                        target: "storage::root",
                        payload_len = data.len(),
                        created_at = metadata.created_at,
                        updated_at = metadata.updated_at(),
                        "captured root snapshot from delta replay"
                    );
                    root_snapshot = Some((data.clone(), metadata.clone()));
                }
                _ => {}
            }

            let action_ctx = ctx_for_action(&action);

            match action {
                Action::Compare { id } => {
                    push_comparison(Comparison {
                        data: <Interface<S>>::find_by_id_raw(id),
                        comparison_data: <Interface<S>>::generate_comparison_data(Some(id))?,
                    });
                }
                Action::Add {
                    id,
                    data,
                    metadata,
                    ancestors,
                } => {
                    if !id.is_root() {
                        info!(
                            target: "storage::sync_child",
                            %id,
                            data_len = data.len(),
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            "SYNC CHILD: Applying Action::Add for child entity"
                        );
                        <Interface<S>>::apply_action(
                            Action::Add {
                                id,
                                data,
                                metadata,
                                ancestors,
                            },
                            &action_ctx,
                        )?;
                    }
                }
                Action::Update {
                    id,
                    data,
                    metadata,
                    ancestors,
                } => {
                    if !id.is_root() {
                        info!(
                            target: "storage::sync_child",
                            %id,
                            data_len = data.len(),
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            "SYNC CHILD: Applying Action::Update for child entity"
                        );
                        <Interface<S>>::apply_action(
                            Action::Update {
                                id,
                                data,
                                metadata,
                                ancestors,
                            },
                            &action_ctx,
                        )?;
                    }
                }
                Action::DeleteRef { .. } => {
                    <Interface<S>>::apply_action(action, &action_ctx)?;
                }
            };
        }

        // Flush deferred ancestor walks. Errors here indicate a
        // partially-propagated merkle tree and must surface to the
        // caller rather than being silently logged by Drop.
        defer_scope.finish()?;

        if let Some((payload, metadata)) = root_snapshot {
            if <Interface<S>>::save_raw(Id::root(), payload, metadata)?.is_some() {
                info!(
                    target: "storage::root",
                    "persisted root document from delta replay"
                );
            }
        }

        Ok(())
    }
}

impl<T, S> Deref for Root<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T, S> DerefMut for Root<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.dirty = true;

        self.get()
    }
}
