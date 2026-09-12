//! `EnsureAccountNamespaceRequest` handler - create this account's namespace
//! the first time the holder needs to write to it, and name it.
//!
//! The creation is skipped when a meta row already exists, so a crash between
//! the row and the creation heals on the next call, and so does losing the race
//! to a concurrent first pairing.
//!
//! Every call also records the holder's own device in the namespace's registry
//! when no row is there, so a namespace created before the registry existed, or
//! one whose publish failed, heals the same way.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{CreateGroupRequest, EnsureAccountNamespaceRequest};
use calimero_context_client::local_governance::AckRouter;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    AccountDeviceRegistry, AccountRoot, MetaRepository, NodeDeviceRepository,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use tracing::{debug, info, warn};

use crate::account_namespace::publish_device_certified;
use crate::ContextManager;

/// The holder's own registry row, published whenever it is missing.
///
/// A device paired later binds the holder into a namespace it founds, and only
/// this row carries the root signature that needs.
async fn record_holder_device(
    datastore: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace_id: ContextGroupId,
    signer_pk: &PublicKey,
    signer_sk: &PrivateKey,
    root: &AccountRoot,
) {
    // Read before the credential is built, so the common case where the row is
    // already there costs one lookup rather than a certificate signature.
    let devices = NodeDeviceRepository::new(datastore);
    match devices.get().map(|own| own.map(|own| own.device())) {
        Ok(Some(device)) => {
            match AccountDeviceRegistry::new(datastore, namespace_id).device(device) {
                Ok(Some(_recorded)) => return,
                Ok(None) => {}
                Err(err) => {
                    warn!(
                        ?err,
                        ?namespace_id,
                        "could not read the holder's registry row"
                    );
                    return;
                }
            }
        }
        Ok(None) => {}
        Err(err) => {
            warn!(?err, ?namespace_id, "could not read this node's device row");
            return;
        }
    }

    let credential = match crate::join_credential::build(datastore, &namespace_id, signer_pk) {
        Ok(credential) => credential,
        Err(err) => {
            warn!(
                ?err,
                ?namespace_id,
                "could not build this device's credential, so the holder is not in its own registry"
            );
            return;
        }
    };
    publish_device_certified(
        datastore,
        node_client,
        ack_router,
        namespace_id,
        signer_sk,
        root,
        &credential,
        &[],
        "ensure_account_namespace",
    )
    .await;
}

impl Handler<EnsureAccountNamespaceRequest> for ContextManager {
    type Result = ActorResponse<Self, <EnsureAccountNamespaceRequest as Message>::Result>;

    fn handle(
        &mut self,
        EnsureAccountNamespaceRequest: EnsureAccountNamespaceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let devices = NodeDeviceRepository::new(&self.datastore);
        let root = match devices.holder_root() {
            Ok(Some(root)) => root,
            Ok(None) => return ActorResponse::reply(Ok(None)),
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let namespace_id = root.account_namespace();

        if let Err(err) = devices.store_account_namespace(&namespace_id) {
            return ActorResponse::reply(Err(err));
        }
        let exists = match MetaRepository::new(&self.datastore).load(&namespace_id) {
            Ok(meta) => meta.is_some(),
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // Resolved here because the node's signing key is a singleton, so the
        // identity this mints is the one `create_group` will find.
        let (_namespace, signer_pk, signer_sk_bytes) =
            match self.get_or_create_namespace_identity(&namespace_id) {
                Ok(identity) => identity,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
        let signer_sk = PrivateKey::from(signer_sk_bytes);

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let context_client = self.context_client.clone();
        ActorResponse::r#async(
            async move {
                if !exists {
                    match context_client
                        .create_group(CreateGroupRequest {
                            group_id: Some(namespace_id),
                            bytecode_id: None,
                            application_id: None,
                            name: None,
                            parent_group_id: None,
                            restricted: true,
                        })
                        .await
                    {
                        Ok(_created) => info!(?namespace_id, "created this account's namespace"),
                        // Two first pairings race here; the one that loses finds
                        // the namespace created and only has the row left to do.
                        Err(err) => {
                            if MetaRepository::new(&datastore)
                                .load(&namespace_id)?
                                .is_none()
                            {
                                return Err(err);
                            }
                            debug!(
                                ?namespace_id,
                                "another call created this account's namespace"
                            );
                        }
                    }
                }

                record_holder_device(
                    &datastore,
                    &node_client,
                    &ack_router,
                    namespace_id,
                    &signer_pk,
                    &signer_sk,
                    &root,
                )
                .await;
                Ok(Some(namespace_id))
            }
            .into_actor(self),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::AccountGenesis;
    use calimero_context_client::group::EnsureAccountNamespaceRequest;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{AccountDeviceRegistry, MetaRepository, NodeDeviceRepository};
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupAccountDevice, GroupTarget};
    use calimero_store::Store;

    use crate::test_support::actor;

    const NS: [u8; 32] = [0xA1; 32];

    fn holder_store() -> Store {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the holder's root");
        store
    }

    /// The first call creates it, every later call names the same one, and the
    /// namespace targets nothing.
    #[actix::test]
    async fn the_holder_creates_its_account_namespace_once() {
        let store = holder_store();
        let devices = NodeDeviceRepository::new(&store);
        let expected = devices
            .account_root()
            .expect("read")
            .expect("root")
            .account_namespace();

        let harness = actor::over(store.clone()).await;
        let first = harness
            .manager
            .send(EnsureAccountNamespaceRequest)
            .await
            .expect("the manager answers")
            .expect("the holder creates its account namespace");
        let second = harness
            .manager
            .send(EnsureAccountNamespaceRequest)
            .await
            .expect("the manager answers")
            .expect("a second call is a read");

        assert_eq!(first, Some(expected));
        assert_eq!(second, Some(expected));
        assert_eq!(devices.account_namespace().expect("read"), Some(expected));
        let meta = MetaRepository::new(&store)
            .load(&expected)
            .expect("read")
            .expect("the namespace exists");
        assert_eq!(meta.target, GroupTarget::default());
    }

    /// Two pairings can reach the holder before the namespace exists, and both
    /// then try to create it. Losing that race must not fail a valid pairing.
    #[actix::test]
    async fn two_concurrent_first_calls_both_name_the_namespace() {
        let store = holder_store();
        let devices = NodeDeviceRepository::new(&store);
        let expected = devices
            .account_root()
            .expect("read")
            .expect("root")
            .account_namespace();

        let harness = actor::over(store.clone()).await;
        let (first, second) = tokio::join!(
            harness.manager.send(EnsureAccountNamespaceRequest),
            harness.manager.send(EnsureAccountNamespaceRequest)
        );

        assert_eq!(
            first.expect("the manager answers").expect("the creation"),
            Some(expected)
        );
        assert_eq!(
            second
                .expect("the manager answers")
                .expect("the loser heals"),
            Some(expected)
        );
        assert!(MetaRepository::new(&store)
            .load(&expected)
            .expect("read")
            .is_some());
    }

    /// A device that founds a namespace has to be able to bind the holder into
    /// it, and only the registry can tell it the holder's certificate. So the
    /// holder records itself when it creates the namespace.
    #[actix::test]
    async fn creation_records_the_holders_own_device() {
        let store = holder_store();
        let devices = NodeDeviceRepository::new(&store);

        let harness = actor::over(store.clone()).await;
        let namespace = harness
            .manager
            .send(EnsureAccountNamespaceRequest)
            .await
            .expect("the manager answers")
            .expect("created")
            .expect("the holder creates its account namespace");

        let own = devices
            .get()
            .expect("read")
            .expect("creating the namespace minted this node's device");
        let (recorded, epoch) = AccountDeviceRegistry::new(&store, namespace)
            .device(own.device())
            .expect("read")
            .expect("the holder is in its own registry");
        assert_eq!(epoch, 0);
        assert!(
            recorded.applications.is_empty(),
            "the holder speaks for every application"
        );
        assert_eq!(recorded.proof.statement.device, own.device());
    }

    /// A namespace created before the registry existed, or one whose publish
    /// failed, leaves the holder unrecorded. The next ensure has to put it back,
    /// because a device paired later binds the holder from that row alone.
    #[actix::test]
    async fn a_holder_whose_row_is_missing_gets_one_on_the_next_ensure() {
        let store = holder_store();
        let devices = NodeDeviceRepository::new(&store);

        let harness = actor::over(store.clone()).await;
        let namespace = harness
            .manager
            .send(EnsureAccountNamespaceRequest)
            .await
            .expect("the manager answers")
            .expect("created")
            .expect("the holder creates its account namespace");
        let own = devices
            .get()
            .expect("read")
            .expect("creating the namespace minted this node's device");

        let row = GroupAccountDevice::new(namespace.to_bytes(), *own.device().as_bytes());
        store.handle().delete(&row).expect("drop the holder's row");
        assert!(AccountDeviceRegistry::new(&store, namespace)
            .device(own.device())
            .expect("read")
            .is_none());

        let again = harness
            .manager
            .send(EnsureAccountNamespaceRequest)
            .await
            .expect("the manager answers")
            .expect("a second call finds the namespace");
        assert_eq!(again, Some(namespace));

        let (recorded, epoch) = AccountDeviceRegistry::new(&store, namespace)
            .device(own.device())
            .expect("read")
            .expect("the holder's row is back");
        assert_eq!(epoch, 0, "nothing stored, so the statement starts over");
        assert!(
            recorded.applications.is_empty(),
            "the holder speaks for every application"
        );
    }

    /// A node paired into another account holds a root, but not that account's,
    /// and must never mint a namespace for the account it merely holds a root of.
    #[actix::test]
    async fn a_node_that_is_not_the_holder_ensures_nothing() {
        let store = holder_store();
        let devices = NodeDeviceRepository::new(&store);
        let _ = devices
            .ensure_enrolled_into(
                &[ContextGroupId::from(NS)],
                AccountGenesis::new(PrivateKey::from([0x53; 32]).public_key()),
            )
            .expect("pair into another account");

        let harness = actor::over(store.clone()).await;
        let answer = harness
            .manager
            .send(EnsureAccountNamespaceRequest)
            .await
            .expect("the manager answers")
            .expect("nothing to do is not an error");

        assert_eq!(answer, None);
        assert_eq!(devices.account_namespace().expect("read"), None);
    }

    /// A crash after the row but before the creation leaves a row naming nothing;
    /// the next call creates what the row names.
    #[actix::test]
    async fn a_dangling_row_is_healed() {
        let store = holder_store();
        let devices = NodeDeviceRepository::new(&store);
        let expected = devices
            .account_root()
            .expect("read")
            .expect("root")
            .account_namespace();
        devices
            .store_account_namespace(&expected)
            .expect("the row a crash left behind");

        let harness = actor::over(store.clone()).await;
        let answer = harness
            .manager
            .send(EnsureAccountNamespaceRequest)
            .await
            .expect("the manager answers")
            .expect("heals");

        assert_eq!(answer, Some(expected));
        assert!(MetaRepository::new(&store)
            .load(&expected)
            .expect("read")
            .is_some());
    }
}
