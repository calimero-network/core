//! Create the holder's account namespace on first use, and name it.

use calimero_context_client::client::ContextClient;
use calimero_context_client::group::CreateGroupRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{MetaRepository, NodeDeviceRepository};
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::info;

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
    Ok(Some(namespace_id))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::AccountGenesis;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{MetaRepository, NodeDeviceRepository};
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupTarget;
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

    /// A node paired into another account mints no namespace for the root it holds.
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
