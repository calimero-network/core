//! Create the holder's account namespace on first use, and name it. Every call also
//! records the holder's own device in the registry when that row is missing.

use calimero_context_client::client::ContextClient;
use calimero_context_client::group::CreateGroupRequest;
use calimero_context_client::local_governance::AckRouter;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    AccountDeviceRegistry, AccountRoot, MetaRepository, NamespaceRepository, NodeDeviceRepository,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{info, warn};

use crate::account_namespace::publish_device_certified;

/// The holder's own registry row, published whenever it is missing: a device
/// that founds a namespace later binds the holder from it.
async fn record_holder_device(
    datastore: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace_id: ContextGroupId,
    signer_pk: &PublicKey,
    signer_sk: &PrivateKey,
    root: &AccountRoot,
) {
    // Read first, so the common case costs a lookup rather than a signature.
    let stored = NodeDeviceRepository::new(datastore).get().and_then(|own| {
        own.map_or(Ok(None), |own| {
            AccountDeviceRegistry::new(datastore, namespace_id).device(own.device())
        })
    });
    match stored {
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

/// Answers `None` on a node that is not the holder of its account.
pub(crate) async fn ensure_account_namespace(
    store: &Store,
    context_client: &ContextClient,
) -> EyreResult<Option<ContextGroupId>> {
    let devices = NodeDeviceRepository::new(store);
    let Some(root) = devices.holder_root()? else {
        return Ok(None);
    };
    let namespace_id = root.account_namespace();

    devices.store_account_namespace(&namespace_id)?;
    let exists = MetaRepository::new(store).load(&namespace_id)?.is_some();

    // Resolved here because the node's signing key is a singleton, so the
    // identity this mints is the one `create_group` will find.
    let (_namespace, signer_pk, signer_sk_bytes) =
        NamespaceRepository::new(store).participate_in(&namespace_id)?;
    let signer_sk = PrivateKey::from(signer_sk_bytes);

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
            // Losing the race to a concurrent first pairing is not an error.
            Err(_) if MetaRepository::new(store).load(&namespace_id)?.is_some() => {}
            Err(err) => return Err(err),
        }
    }

    record_holder_device(
        store,
        context_client.node_client(),
        context_client.ack_router(),
        namespace_id,
        &signer_pk,
        &signer_sk,
        &root,
    )
    .await;
    Ok(Some(namespace_id))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::AccountGenesis;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{AccountDeviceRegistry, MetaRepository, NodeDeviceRepository};
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupAccountDevice, GroupTarget};
    use calimero_store::Store;

    use super::ensure_account_namespace;
    use crate::test_support::actor;

    const NS: [u8; 32] = [0xA1; 32];

    fn holder_store() -> Store {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the holder's root");
        store
    }

    /// Two first pairings race to create it: both name it, and it targets nothing.
    #[actix::test]
    async fn concurrent_first_calls_create_one_app_less_namespace() {
        let store = holder_store();
        let expected = NodeDeviceRepository::new(&store)
            .account_root()
            .expect("read")
            .expect("root")
            .account_namespace();

        let harness = actor::over(store.clone()).await;
        let (first, second) = tokio::join!(
            ensure_account_namespace(&store, &harness.context_client),
            ensure_account_namespace(&store, &harness.context_client)
        );

        assert_eq!(first.expect("the creation"), Some(expected));
        assert_eq!(second.expect("the loser heals"), Some(expected));
        let meta = MetaRepository::new(&store)
            .load(&expected)
            .expect("read")
            .expect("the namespace exists");
        assert_eq!(meta.target, GroupTarget::default());
    }

    /// The holder records itself, since only that row carries its certificate.
    #[actix::test]
    async fn creation_records_the_holders_own_device() {
        let store = holder_store();
        let devices = NodeDeviceRepository::new(&store);

        let harness = actor::over(store.clone()).await;
        let namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
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

    /// A missing row, from a failed publish or a namespace older than the
    /// registry, is put back by the next ensure.
    #[actix::test]
    async fn a_holder_whose_row_is_missing_gets_one_on_the_next_ensure() {
        let store = holder_store();
        let devices = NodeDeviceRepository::new(&store);

        let harness = actor::over(store.clone()).await;
        let namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
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

        let again = ensure_account_namespace(&store, &harness.context_client)
            .await
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
        let answer = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("nothing to do is not an error");

        assert_eq!(answer, None);
        assert_eq!(devices.account_namespace().expect("read"), None);
    }
}
