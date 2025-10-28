use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::{key, types};
use eyre::bail;

use super::ContextClient;

/// Represents a user's identity within a specific context.
///
/// An identity is defined by a public key. If the node manages this identity,
/// it will also hold the corresponding private key(s).
#[derive(Debug)]
pub struct ContextIdentity {
    /// The primary public key for this identity, used for identification and signing.
    pub public_key: PublicKey,
    /// The optional private key corresponding to `public_key`. If `Some`, this node
    /// "owns" or "manages" this identity and can sign transactions on its behalf.
    /// If `None`, this node only knows about the identity but cannot act as it.
    pub private_key: Option<PrivateKey>,
    /// An optional, secondary private key used for a specific purpose, such as a
    /// dedicated key for sending messages or transactions to reduce exposure of the primary key.
    pub sender_key: Option<PrivateKey>,
}

impl ContextIdentity {
    /// Returns a reference to the private key if it exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the identity is not managed by this node (i.e., `private_key` is `None`).
    pub fn private_key(&self) -> eyre::Result<&PrivateKey> {
        let Some(private_key) = &self.private_key else {
            bail!(
                "the identity '{}' is not managed by this node",
                self.public_key
            );
        };

        Ok(private_key)
    }
}

impl ContextClient {
    /// Creates a new cryptographic identity (key pair) and stores it in the datastore.
    /// The private key is randomly generated.
    /// The new identity doesn't have any `sender_key`. If needed, the `sender_key` could be set via
    /// `update_identity()` method later.
    ///
    /// # Note
    ///
    /// This identity is not initially tied to a specific context (it is stored under a
    /// zeroed-out `ContextId`). It can be seen as a "global" identity within the node
    /// that can later be associated with one or more contexts.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `PublicKey` of the newly created identity.
    ///
    /// # Errors
    ///
    /// Returns an error if there is an issue writing the new identity to the datastore.
    pub fn new_identity(&self) -> eyre::Result<PublicKey> {
        let mut handle = self.datastore.handle();

        let private_key = PrivateKey::random(&mut rand::thread_rng());
        let public_key = private_key.public_key();

        handle.put(
            &key::ContextIdentity::new(ContextId::zero(), public_key),
            &types::ContextIdentity {
                private_key: Some(*private_key),
                sender_key: None,
            },
        )?;

        Ok(public_key)
    }

    /// Retrieves an identity from the datastore for a given context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The context in which the identity is being retrieved.
    /// * `public_key` - The public key of the identity to fetch.
    ///
    /// # Returns
    ///
    /// An `Option` containing the `ContextIdentity` if found, otherwise `None`.
    ///
    /// # Errors
    ///
    /// Returns an error if there is an issue reading from the datastore.
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

    /// Updates an existing identity in the datastore.
    ///
    /// This is typically used to add or change the `sender_key` or `private_key`
    /// for an identity that the node already knows about.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The context of the identity to update.
    /// * `new_identity` - The `ContextIdentity` object containing the updated fields.
    ///
    /// # Errors
    ///
    /// Returns an error if the identity does not exist or if there is a datastore issue.
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
        // TODO: what we are updating the private key for? if we got here, the datastore already
        // has the `identity.private_key` set.
        identity.private_key = new_identity.private_key.as_deref().copied();

        handle.put(&key, &identity)?;

        Ok(())
    }

    /// Deletes an identity from the datastore for a given context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The context from which to delete the identity.
    /// * `public_key` - The public key of the identity to delete.
    ///
    /// # Errors
    ///
    /// Returns an error if there is an issue writing to the datastore.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ContextClient;
    use calimero_blobstore::{config::BlobStoreConfig, BlobManager, FileSystem};
    use calimero_context_config::client::{
        config::{ClientConfig, ClientRelayerSigner, ClientSigner, LocalConfig},
        Client as ExternalClient,
    };
    use calimero_network_primitives::client::NetworkClient;
    use calimero_node_primitives::{client::NodeClient, messages::NodeMessage};
    use calimero_primitives::common::DIGEST_SIZE;
    use calimero_store::{db::InMemoryDB, key, types, Store};
    use calimero_utils_actix::LazyRecipient;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use tokio;
    use tokio::sync::{broadcast, mpsc};

    /// Correctly initializes all dependencies using the public `from_config` constructor.
    async fn setup_context_client() -> ContextClient {
        // 1. Create the InMemoryDB directly.
        let db = InMemoryDB::owned();
        let store = Store::new(Arc::new(db));

        // 2. BlobManager setup.
        let tmp_dir = tempfile::tempdir().unwrap();
        let blob_store_config =
            BlobStoreConfig::new(tmp_dir.path().to_path_buf().try_into().unwrap());
        let file_system = FileSystem::new(&blob_store_config).await.unwrap();
        let blob_manager = BlobManager::new(store.clone(), file_system);

        // 3. Mock/dummy network and actor dependencies.
        let network_client = NetworkClient::new(LazyRecipient::new());
        let (event_sender, _) = broadcast::channel(16);
        let (ctx_sync_tx, _) = mpsc::channel(16);
        let node_manager = LazyRecipient::<NodeMessage>::new();

        // 4. Construct the real NodeClient.
        let node_client = NodeClient::new(
            store.clone(),
            blob_manager,
            network_client,
            node_manager,
            event_sender,
            ctx_sync_tx,
        );

        // 5. Create a minimal, valid ClientConfig.
        let client_config = ClientConfig {
            params: BTreeMap::new(),
            signer: ClientSigner {
                relayer: ClientRelayerSigner {
                    url: "http://127.0.0.1:3030".parse().unwrap(),
                },
                local: LocalConfig {
                    protocols: BTreeMap::new(),
                },
            },
        };

        // 6. Construct the ExternalClient using the intended public API.
        // This is much cleaner and more robust than manual construction.
        let external_client = ExternalClient::from_config(&client_config);

        // 7. Construct the final ContextClient.
        let context_manager = LazyRecipient::new();
        ContextClient::new(store, node_client, external_client, context_manager)
    }

    #[tokio::test]
    async fn test_new_and_get_identity() {
        let client = setup_context_client().await;
        let public_key = client.new_identity().expect("Should create identity");
        let context_id = ContextId::zero();
        let identity = client
            .get_identity(&context_id, &public_key)
            .unwrap()
            .expect("Identity should be found in the datastore");

        assert_eq!(identity.public_key, public_key);
        assert!(identity.private_key.is_some(), "Identity should be owned");
        assert!(identity.sender_key.is_none());
    }

    #[tokio::test]
    async fn test_update_and_delete_identity() {
        let client = setup_context_client().await;
        let context_id = ContextId::from([1; DIGEST_SIZE]);
        let public_key = client.new_identity().unwrap();

        let private_key_bytes: [u8; DIGEST_SIZE] = [1; DIGEST_SIZE];
        let mut handle = client.datastore.handle();
        let key = key::ContextIdentity::new(context_id, public_key);
        let id_data = types::ContextIdentity {
            private_key: Some(private_key_bytes.into()),
            sender_key: None,
        };
        handle.put(&key, &id_data).unwrap();

        let sender_private_key = PrivateKey::from([2; DIGEST_SIZE]);
        let mut identity_to_update = client
            .get_identity(&context_id, &public_key)
            .unwrap()
            .unwrap();
        identity_to_update.sender_key = Some(sender_private_key);

        client
            .update_identity(&context_id, &identity_to_update)
            .unwrap();

        let updated_identity = client
            .get_identity(&context_id, &public_key)
            .unwrap()
            .unwrap();
        assert!(
            updated_identity.sender_key.is_some(),
            "Sender key should have been updated"
        );

        client.delete_identity(&context_id, &public_key).unwrap();

        let final_identity = client.get_identity(&context_id, &public_key).unwrap();
        assert!(
            final_identity.is_none(),
            "Identity should be None after deletion"
        );
    }
}
