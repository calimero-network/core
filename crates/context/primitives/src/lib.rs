use std::ops::Deref;

use calimero_primitives::context::ContextId;
use tokio::sync::OwnedMutexGuard;

pub mod client;
pub mod group;
pub mod local_governance;
pub mod messages;

/// An owned, per-context lock guard held across a context operation.
///
/// This is a newtype over `OwnedMutexGuard<ContextId>` rather than the raw
/// guard so that the single notion of "holding a context" lives behind one
/// type: the guard is acquired in the manager, threaded through the entire
/// WASM execution outside the actor, and can be handed back in as
/// [`ContextAtomic::Held`] for a multi-call atomic batch. Centralizing it here
/// is what lets the read/write-intent split (parallel reads) be introduced
/// later without touching every call site.
///
/// Derefs to the locked [`ContextId`] so existing read sites keep working.
#[derive(Debug)]
pub struct ContextGuard(OwnedMutexGuard<ContextId>);

impl ContextGuard {
    /// Wrap an owned mutex guard. The guard keeps the per-context lock held for
    /// the lifetime of this value.
    #[must_use]
    pub fn new(guard: OwnedMutexGuard<ContextId>) -> Self {
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
