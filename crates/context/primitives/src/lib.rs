use calimero_primitives::context::ContextId;
use tokio::sync::OwnedMutexGuard;

pub mod client;
pub mod messages;

#[derive(Debug)]
pub enum ContextAtomic {
    Lock,
    Held(ContextAtomicKey),
}

#[derive(Debug)]
pub struct ContextAtomicKey(pub OwnedMutexGuard<ContextId>);
