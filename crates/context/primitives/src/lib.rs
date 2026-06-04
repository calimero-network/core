use std::ops::Deref;

use calimero_primitives::context::ContextId;
use tokio::sync::OwnedRwLockWriteGuard;

pub mod client;
pub mod group;
pub mod local_governance;
pub mod messages;

/// An owned, exclusive per-context lock guard held across a context operation.
///
/// The per-context lock is an `RwLock`, but every caller currently takes the
/// exclusive *write* guard, so the lock behaves exactly like the prior mutex.
/// Keeping the single notion of "holding a context" behind this one type —
/// acquired in the manager, threaded through the entire WASM execution outside
/// the actor, and handed back in as [`ContextAtomic::Held`] for a multi-call
/// atomic batch — is what will let a shared read guard be introduced (parallel
/// reads, gated on declared read-only method intent) without touching every
/// call site.
///
/// Derefs to the locked [`ContextId`] so existing read sites keep working.
#[derive(Debug)]
pub struct ContextGuard(OwnedRwLockWriteGuard<ContextId>);

impl ContextGuard {
    /// Wrap an exclusive write guard. Keeps the lock held exclusively for the
    /// lifetime of this value.
    #[must_use]
    pub fn new(guard: OwnedRwLockWriteGuard<ContextId>) -> Self {
        Self(guard)
    }
}

impl Deref for ContextGuard {
    type Target = ContextId;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug)]
pub enum ContextAtomic {
    Lock,
    Held(ContextAtomicKey),
}

#[derive(Debug)]
pub struct ContextAtomicKey(pub ContextGuard);
