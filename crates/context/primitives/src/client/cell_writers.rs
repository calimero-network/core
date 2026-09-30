//! Where a path with no governance cut of its own asks a `SharedStorage` cell's writers.
//!
//! A repair or a pushed leaf carries no position, so the node answers for it at its own
//! current governance heads. The fold lives where the governance projection does, above
//! this crate, so the node fills the seam in once it exists and every clone of the client
//! sees it.

use core::fmt;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use calimero_primitives::context::ContextId;
use calimero_storage::address::Id;
use calimero_storage::shared_writers::CellWriters;

/// Reads a cell's writers at this node's current governance heads.
pub trait CurrentCellWriters: Send + Sync {
    /// `None` when they cannot be read yet, which a caller treats as no writers.
    fn writers(&self, context_id: &ContextId, cell: Id) -> Option<CellWriters>;
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

    /// A host resolver for `context_id`'s cells, for a runtime env.
    #[must_use]
    pub fn resolver(&self, context_id: ContextId) -> Rc<dyn Fn(Id) -> Option<CellWriters>> {
        let slot = self.clone();
        Rc::new(move |cell| match slot.0.get() {
            Some(source) => source.writers(&context_id, cell),
            None => Some(CellWriters::Genesis),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Option<CellWriters>);

    impl CurrentCellWriters for Fixed {
        fn writers(&self, _context_id: &ContextId, _cell: Id) -> Option<CellWriters> {
            self.0.clone()
        }
    }

    #[test]
    fn an_unset_slot_leaves_every_cell_at_genesis() {
        let resolve = CurrentCellWritersSlot::default().resolver(ContextId::from([1; 32]));
        assert_eq!(resolve(Id::new([2; 32])), Some(CellWriters::Genesis));
    }

    #[test]
    fn a_clone_made_before_the_install_sees_it() {
        let slot = CurrentCellWritersSlot::default();
        let early = slot.clone();
        assert!(slot.install(Arc::new(Fixed(None))));
        assert!(!slot.install(Arc::new(Fixed(Some(CellWriters::Genesis)))));
        let resolve = early.resolver(ContextId::from([1; 32]));
        assert_eq!(resolve(Id::new([2; 32])), None, "the first install stays");
    }
}
