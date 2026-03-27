use calimero_primitives::common::DIGEST_SIZE;
use calimero_runtime::logic::ContextHost;
use calimero_store::{key, Store};

use calimero_primitives::context::ContextId;

/// A bridge implementation that exposes context information from the `calimero-store`
/// to the runtime via the `ContextHost` trait.
#[derive(Debug)]
pub struct StoreContextHost {
    pub store: Store,
    pub context_id: ContextId,
}

impl ContextHost for StoreContextHost {
    fn is_member(&self, public_key: &[u8; DIGEST_SIZE]) -> bool {
        let key = key::ContextIdentity::new(self.context_id, (*public_key).into());
        self.store.handle().has(&key).unwrap_or(false)
    }

    fn members(&self) -> Vec<[u8; DIGEST_SIZE]> {
        let handle = self.store.handle();
        let mut members = Vec::new();

        if let Ok(mut iter) = handle.iter::<key::ContextIdentity>() {
            let start_key = key::ContextIdentity::new(self.context_id, [0u8; DIGEST_SIZE].into());

            let first = iter.seek(start_key).ok().flatten();

            for k in first.into_iter().chain(iter.keys().flatten()) {
                if k.context_id() != self.context_id {
                    break;
                }
                members.push(*k.public_key());
            }
        }
        members
    }
}
