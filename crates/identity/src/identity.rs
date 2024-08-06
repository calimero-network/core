use std::collections::HashMap;

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{KeyPair, PublicKey};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IdentityHandler {
    context_identities: HashMap<ContextId, KeyPair>,
}

impl IdentityHandler {
    pub fn new() -> Self {
        Self {
            context_identities: HashMap::new(),
        }
    }

    pub fn add_context_identity(
        &mut self,
        context_id: ContextId,
        public_key: PublicKey,
        private_key: Option<[u8; 32]>,
    ) {
        self.context_identities.insert(
            context_id,
            KeyPair {
                public_key,
                private_key,
            },
        );
    }

    pub fn get_context_identity(&self, context_id: &ContextId) -> Option<&KeyPair> {
        self.context_identities.get(context_id)
    }
}
