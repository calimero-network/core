use std::ops::Deref;

use calimero_primitives::context::ContextId;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard};

pub mod client;
pub mod group;
pub mod local_governance;
pub mod messages;

/// An owned, per-context lock guard held across a context operation.
///
/// The per-context lock is an `RwLock`. A guard is either:
/// - [`ContextGuard::Write`] — exclusive, blocks all other access; the default
///   for any call whose read/write intent is not known to be read-only.
/// - [`ContextGuard::Read`] — shared, coexists with other read guards on the
///   same context; only handed out for methods explicitly declared read-only
///   via `#[app::view]` in the module ABI.
///
/// The single notion of "holding a context" living behind this one type is what
/// keeps every call site (the execute path, the sync handler, and the
/// `ContextAtomic::Held` atomic-batch path) working without knowing which
/// variant they hold.
///
/// Derefs to the locked [`ContextId`] (identical for both variants) so existing
/// read sites keep working.
#[derive(Debug)]
pub enum ContextGuard {
    /// Exclusive guard — blocks all other access. The default for any call
    /// whose read/write intent is not known to be read-only.
    Write(OwnedRwLockWriteGuard<ContextId>),
    /// Shared guard — coexists with other read guards on the same context.
    /// Only minted for methods declared read-only in the module ABI.
    Read(OwnedRwLockReadGuard<ContextId>),
}

impl ContextGuard {
    /// Wrap an exclusive write guard.
    #[must_use]
    pub fn write(guard: OwnedRwLockWriteGuard<ContextId>) -> Self {
        Self::Write(guard)
    }

    /// Wrap a shared read guard.
    #[must_use]
    pub fn read(guard: OwnedRwLockReadGuard<ContextId>) -> Self {
        Self::Read(guard)
    }

    /// Returns `true` if this is an exclusive write guard.
    #[must_use]
    pub fn is_write(&self) -> bool {
        matches!(self, Self::Write(_))
    }
}

impl Deref for ContextGuard {
    type Target = ContextId;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Write(g) => g,
            Self::Read(g) => g,
        }
    }
}

#[derive(Debug)]
pub enum ContextAtomic {
    Lock,
    Held(ContextAtomicKey),
}

#[derive(Debug)]
pub struct ContextAtomicKey(pub ContextGuard);
