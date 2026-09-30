//! Where a path with no governance cut of its own asks a `SharedStorage` cell's writers.
//!
//! A repair or a pushed leaf carries no position, so the node answers for it with every
//! writer the cell has had by its own current governance heads: a since-removed writer's
//! earlier write must still reach a node that repairs. The fold lives where the governance
//! projection does, above this crate, so the node fills the seam in once it exists and every
//! clone of the client sees it.

use core::fmt;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use calimero_primitives::context::ContextId;
use calimero_storage::address::Id;
use calimero_storage::shared_writers::{CellWriters, WritersUnavailable};

/// Reads a cell's writers at this node's current governance heads.
pub trait CurrentCellWriters: Send + Sync {
    /// Every writer the cell has had by those heads: the set its id commits to, plus
    /// whoever a counted rotation put in it. `Err` when they cannot be read yet.
    fn ever_writers(
        &self,
        context_id: &ContextId,
        cell: Id,
    ) -> Result<CellWriters, WritersUnavailable>;
}

/// Write-once holder for the [`CurrentCellWriters`] implementation.
///
/// Unset is a working state: every cell then stands at the writers its id commits to,
/// which is what a harness with no governance wants.
#[derive(Clone, Default)]
pub struct CurrentCellWritersSlot(Arc<OnceLock<Arc<dyn CurrentCellWriters>>>);

impl fmt::Debug for CurrentCellWritersSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CurrentCellWritersSlot")
            .field(&if self.0.get().is_some() {
                "installed"
            } else {
                "unset"
            })
            .finish()
    }
}

impl CurrentCellWritersSlot {
    /// Install the implementation. Returns `false` if one was already installed, in which
    /// case this call had no effect.
    pub fn install(&self, source: Arc<dyn CurrentCellWriters>) -> bool {
        self.0.set(source).is_ok()
    }

    /// The ever-writers of `cell` in `context_id`; `Genesis` while no implementation is
    /// installed.
    ///
    /// # Errors
    /// The heads cannot be read yet, or the cell has more rotations than the fold takes.
    pub fn ever_writers(
        &self,
        context_id: &ContextId,
        cell: Id,
    ) -> Result<CellWriters, WritersUnavailable> {
        match self.0.get() {
            Some(source) => source.ever_writers(context_id, cell),
            None => Ok(CellWriters::Genesis),
        }
    }

    /// A host resolver for `context_id`'s cells, for a runtime env. A cell it cannot read
    /// has no writers, so a write to it is refused and a later repair retries it.
    #[must_use]
    pub fn ever_resolver(&self, context_id: ContextId) -> Rc<dyn Fn(Id) -> Option<CellWriters>> {
        let slot = self.clone();
        Rc::new(move |cell| slot.ever_writers(&context_id, cell).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Result<CellWriters, WritersUnavailable>);

    impl CurrentCellWriters for Fixed {
        fn ever_writers(
            &self,
            _context_id: &ContextId,
            _cell: Id,
        ) -> Result<CellWriters, WritersUnavailable> {
            self.0.clone()
        }
    }

    #[test]
    fn an_unset_slot_leaves_every_cell_at_genesis() {
        let slot = CurrentCellWritersSlot::default();
        let context = ContextId::from([1; 32]);
        assert_eq!(
            slot.ever_writers(&context, Id::new([2; 32])),
            Ok(CellWriters::Genesis)
        );
        assert_eq!(
            slot.ever_resolver(context)(Id::new([2; 32])),
            Some(CellWriters::Genesis)
        );
    }

    #[test]
    fn a_clone_made_before_the_install_sees_it() {
        let slot = CurrentCellWritersSlot::default();
        let early = slot.clone();
        assert!(slot.install(Arc::new(Fixed(Err(WritersUnavailable::Cut)))));
        assert!(!slot.install(Arc::new(Fixed(Ok(CellWriters::Genesis)))));
        let context = ContextId::from([1; 32]);
        assert_eq!(
            early.ever_writers(&context, Id::new([2; 32])),
            Err(WritersUnavailable::Cut),
            "the first install stays"
        );
        assert_eq!(
            early.ever_resolver(context)(Id::new([2; 32])),
            None,
            "a cell that cannot be read has no writers"
        );
    }
}
