use calimero_primitives::context::ContextId;
use tokio::sync::OwnedMutexGuard;

#[derive(Debug)]
pub enum ContextAtomic {
    Lock,
    Held(ContextAtomicKey),
}

#[derive(Debug)]
pub struct ContextAtomicKey(pub OwnedMutexGuard<ContextId>);


