//! `Frozen<T>`: one value, written once by the account that created it, that
//! nobody can ever change.
//!
//! ```ignore
//! #[app::state]
//! pub struct Forum {
//!     charter: Frozen<String>,
//! }
//!
//! #[app::init]
//! fn init() -> Self {
//!     Self { charter: Frozen::new("be kind".to_owned()) }
//! }
//!
//! self.charter.get()?; // there is no set
//! ```
//!
//! # What holds it
//!
//! Every node, not the API. A `Frozen<T>` is a [`WriterSetCell`] whose one
//! writer is the account that created it, holding [`OpMask::WRITE_ONCE`] and
//! nothing else:
//!
//! * the value is written at creation. A later write with different bytes is
//!   refused on apply, even one that account signs; a redelivery of the same
//!   bytes is accepted, so sync converges;
//! * no `DELETE` bit, so no node accepts removing it;
//! * no `ADMIN` bit and a frozen cell, so the writer set cannot be rotated to
//!   someone who could;
//! * anyone else's write fails the writer-set check, before it is stored.
//!
//! The one trust assumption is the same as `SharedStorage`'s: the writer set
//! comes from genesis, which the creator writes, so a node trusts the peer it
//! first takes genesis from. A node that received a forged genesis would hold a
//! forged value, exactly as it would hold a forged writer set.
//!
//! # What it is for
//!
//! Plain data fixed at creation: a charter, a policy version, a founding date,
//! a configuration nobody should change. For a set of immutable entries, use
//! [`WriteOnce`](super::WriteOnce) (owned, keyed by the app) or
//! [`ContentAddressed`](super::ContentAddressed) (keyed by content hash).

use core::fmt;
use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;

use super::crdt_meta::{CrdtMeta, CrdtType, MergeError, MergeStrategy, Mergeable, StorageStrategy};
use super::{FrozenValue, StoreError, WriterSetCell};
use crate::address::Id;
use crate::entities::{ChildInfo, Data, Element};

/// One value, written once by the account that created it; see the
/// [module documentation](self).
#[derive(BorshSerialize, BorshDeserialize)]
pub struct Frozen<T: BorshSerialize + BorshDeserialize + Default + 'static> {
    #[borsh(bound(serialize = "", deserialize = ""))]
    cell: WriterSetCell<FrozenValue<T>>,
}

impl<T: BorshSerialize + BorshDeserialize + Default + 'static> Frozen<T> {
    /// Freeze `value`, with the calling account as its only writer.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            cell: WriterSetCell::new_write_once(FrozenValue(value)),
        }
    }

    /// The value.
    ///
    /// # Errors
    /// Returns any storage error from loading it.
    pub fn get(&self) -> Result<&T, StoreError> {
        Ok(&self.cell.get()?.0)
    }

    /// The account that wrote it.
    #[must_use]
    pub fn writer(&self) -> Option<AccountId> {
        self.cell.writers().into_iter().next()
    }

    /// The cell's anchor and value-entry ids, for tests that play a peer.
    #[cfg(test)]
    pub(crate) fn ids(&self) -> (Id, Id) {
        (self.cell.anchor(), self.cell.value_id())
    }

    /// Give the cell a deterministic id from `field_name`. Called by the
    /// `#[app::state]` macro after `init()`, before anything is broadcast.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        self.cell.reassign_deterministic_id(field_name);
    }
}

impl<T> fmt::Debug for Frozen<T>
where
    T: BorshSerialize + BorshDeserialize + Default + fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.get() {
            Ok(value) => f.debug_tuple("Frozen").field(value).finish(),
            Err(e) => f.debug_struct("Frozen").field("read_error", &e).finish(),
        }
    }
}

impl<T: BorshSerialize + BorshDeserialize + Default + 'static> Data for Frozen<T> {
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        self.cell.collections()
    }

    fn element(&self) -> &Element {
        self.cell.element()
    }

    fn element_mut(&mut self) -> &mut Element {
        self.cell.element_mut()
    }
}

impl<T: BorshSerialize + BorshDeserialize + Default + 'static> super::rekey::RekeyTarget
    for Frozen<T>
{
    fn rekey_relative_to(&mut self, parent_id: Id) {
        self.cell.rekey_relative_to(parent_id);
    }
}

/// Nothing to merge: the value never changes, and the cell's writer set is
/// frozen. Adopting a peer's bytes here would be the one way around the
/// write-once check.
#[diagnostic::do_not_recommend]
impl<T: BorshSerialize + BorshDeserialize + Default + 'static> Mergeable for Frozen<T> {
    fn merge(&mut self, _other: &Self) -> Result<(), MergeError> {
        Ok(())
    }
}

impl<T: BorshSerialize + BorshDeserialize + Default + 'static> CrdtMeta for Frozen<T> {
    fn crdt_type() -> CrdtType {
        CrdtType::SharedStorage
    }
    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Structured
    }
    fn can_contain_crdts() -> bool {
        false
    }
}

/// Structural: see [`MergeStrategy`].
#[diagnostic::do_not_recommend]
impl<T: BorshSerialize + BorshDeserialize + Default + 'static> MergeStrategy for Frozen<T> {
    const DISPATCHED: bool = false;
}
