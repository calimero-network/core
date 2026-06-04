use std::ops::Deref;

use calimero_primitives::context::ContextId;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard};

pub mod client;
pub mod group;
pub mod local_governance;
pub mod messages;

/// An owned, per-context lock guard held across a context operation.
///
/// The per-context lock is an `RwLock`, so a held guard is either an exclusive
/// *write* guard or a shared *read* guard. Keeping the single notion of
/// "holding a context" behind this one type — acquired in the manager, threaded
/// through the entire WASM execution outside the actor, and handed back in as
/// [`ContextAtomic::Held`] for a multi-call atomic batch — is what lets the
/// read/write-intent split (parallel reads) be introduced without touching
/// every call site.
///
/// Read guards are only ever minted once a method is *declared* read-only via
/// the module ABI; until then every caller takes a write guard, so the lock
/// behaves exactly like the prior exclusive mutex.
///
/// Derefs to the locked [`ContextId`] (identical for both variants) so existing
/// read sites keep working.
#[derive(Debug)]
pub enum ContextGuard {
    /// Exclusive guard — blocks all other access. The default for any call
    /// whose read/write intent is not known to be read-only.
    Write(OwnedRwLockWriteGuard<ContextId>),
    /// Shared guard — coexists with other read guards on the same context.
    Read(OwnedRwLockReadGuard<ContextId>),
}

impl ContextGuard {
    /// Wrap an exclusive write guard. Keeps the lock held exclusively for the
    /// lifetime of this value.
    #[must_use]
    pub fn write(guard: OwnedRwLockWriteGuard<ContextId>) -> Self {
        Self::Write(guard)
    }

    /// Wrap a shared read guard. Keeps the lock held in shared mode for the
    /// lifetime of this value.
    #[must_use]
    pub fn read(guard: OwnedRwLockReadGuard<ContextId>) -> Self {
        Self::Read(guard)
    }
}

impl Deref for ContextGuard {
    type Target = ContextId;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Write(guard) => guard,
            Self::Read(guard) => guard,
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
