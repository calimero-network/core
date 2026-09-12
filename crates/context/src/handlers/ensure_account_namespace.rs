//! Create this account's namespace the first time the holder needs to write to
//! it, and name it.
//!
//! The creation is skipped when a meta row already exists, so a crash between
//! the row and the creation heals on the next call, and so does losing the race
//! to a concurrent first pairing.

use calimero_context_client::client::ContextClient;
use calimero_context_client::group::CreateGroupRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{MetaRepository, NodeDeviceRepository};
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{debug, info};

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
    if MetaRepository::new(store).load(&namespace_id)?.is_some() {
        return Ok(Some(namespace_id));
    }

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
        // Two first pairings race here; the one that loses finds the
        // namespace created and has nothing left to do.
        Err(err) => {
            if MetaRepository::new(store).load(&namespace_id)?.is_none() {
                return Err(err);
            }
            debug!(
                ?namespace_id,
                "another call created this account's namespace"
            );
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
        let first = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the holder creates its account namespace");
        let second = ensure_account_namespace(&store, &harness.context_client)
            .await
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
            ensure_account_namespace(&store, &harness.context_client),
            ensure_account_namespace(&store, &harness.context_client)
        );

        assert_eq!(first.expect("the creation"), Some(expected));
        assert_eq!(second.expect("the loser heals"), Some(expected));
        assert!(MetaRepository::new(&store)
            .load(&expected)
            .expect("read")
            .is_some());
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
        let answer = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("heals");

        assert_eq!(answer, Some(expected));
        assert!(MetaRepository::new(&store)
            .load(&expected)
            .expect("read")
            .is_some());
    }
}
