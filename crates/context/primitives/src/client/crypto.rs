use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::{key, types};
use eyre::bail;

use crate::client::keypairs::KeypairManager;

use super::ContextClient;

#[derive(Debug)]
pub struct ContextIdentity {
    pub public_key: PublicKey,
    pub keypair_ref: Option<PublicKey>, // Reference to global keypair
    pub sender_key: Option<PrivateKey>,
}

impl ContextIdentity {
    pub fn private_key(&self, client: &ContextClient) -> eyre::Result<Option<PrivateKey>> {
        let Some(keypair_ref) = self.keypair_ref else {
            return Ok(None);
        };

        let keypair_manager = KeypairManager::new(client.datastore.clone());
        let Some(keypair) = keypair_manager.get(&keypair_ref)? else {
            bail!("Referenced keypair not found: {}", keypair_ref);
        };

        Ok(Some(PrivateKey::from(keypair.private_key)))
    }
}

impl ContextClient {
    pub fn new_identity(&self, alias: Option<String>) -> eyre::Result<PublicKey> {
        let mut keypair_manager = KeypairManager::new(self.datastore.clone());
        let public_key = keypair_manager.generate(alias)?;

        // Create context identity reference
        let context_identity = types::ContextIdentity {
            keypair_ref: Some(*public_key),
            sender_key: None,
        };

        // Store in default context (or could be context-agnostic)
        let mut handle = self.datastore.handle();
        handle.put(
            &key::ContextIdentity::new(ContextId::from([0u8; 32]), public_key),
            &context_identity,
        )?;

        Ok(public_key)
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
            keypair_ref: identity.keypair_ref.map(PublicKey::from),
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

        identity.sender_key = new_identity.sender_key.as_ref().map(|k| **k);
        identity.keypair_ref = new_identity.keypair_ref.as_ref().map(|k| **k);

        handle.put(&key, &identity)?;

        Ok(())
    }

    pub fn delete_identity(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
    ) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        let key = key::ContextIdentity::new(*context_id, *public_key);

        handle.delete(&key)?;

        Ok(())
    }
}
