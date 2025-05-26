use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key;
use eyre::bail;

use super::ContextClient;

#[derive(Copy, Clone, Debug)]
pub struct ContextIdentity {
    pub public_key: PublicKey,
    pub private_key: Option<PrivateKey>,
    pub sender_key: Option<PrivateKey>,
}

impl ContextIdentity {
    pub fn private_key(&self) -> eyre::Result<PrivateKey> {
        let Some(private_key) = self.private_key else {
            bail!(
                "the identity '{}' is not managed by this node",
                self.public_key
            );
        };

        Ok(private_key)
    }
}

impl ContextClient {
    // fixme! refactor as part of #1066
    pub fn new_private_key(&self) -> PrivateKey {
        PrivateKey::random(&mut rand::thread_rng())
    }

    pub fn get_identity(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
    ) -> eyre::Result<Option<ContextIdentity>> {
        let handle = self.datastore.handle();

        let key = key::ContextIdentity::new(*context_id, *public_key);

        let Some(identity) = handle.get(&key)? else {
            return Ok(None);
        };

        let identity = ContextIdentity {
            public_key: *public_key,
            private_key: identity.private_key.map(PrivateKey::from),
            sender_key: identity.sender_key.map(PrivateKey::from),
        };

        Ok(Some(identity))
    }

    pub fn update_identity(
        &self,
        context_id: &ContextId,
        new_identity: &ContextIdentity,
    ) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        let key = key::ContextIdentity::new(*context_id, new_identity.public_key);

        let Some(mut identity) = handle.get(&key)? else {
            bail!(
                "the identity '{}' is not managed on this node for context '{}'",
                new_identity.public_key,
                context_id
            );
        };

        identity.sender_key = new_identity.sender_key.as_deref().copied();
        identity.private_key = new_identity.private_key.as_deref().copied();

        handle.put(&key, &identity)?;

        Ok(())
    }
}
