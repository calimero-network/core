//! One-shot on start: publish a pre-registry holder's cached device
//! certificates into the account namespace, then drop them.
//!
//! Nothing else will ever do it. The cache was node-local, so a device certified
//! before the registry existed is one no other device of the account can carry
//! anywhere until its certificate is on the account namespace's DAG.

use std::sync::Arc;

use calimero_context_client::client::ContextClient;
use calimero_context_client::group::EnsureAccountNamespaceRequest;
use calimero_context_client::local_governance::AckRouter;
use calimero_governance_store::{AccountDeviceRegistry, NamespaceRepository, NodeDeviceRepository};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{info, warn};

use crate::account_namespace::publish_device_certified;

/// Run the migration in the background, once.
pub(crate) fn spawn(
    store: Store,
    node_client: NodeClient,
    ack_router: Arc<AckRouter>,
    context_client: ContextClient,
) {
    let _task = tokio::spawn(async move {
        if let Err(err) = run(&store, &node_client, &ack_router, &context_client).await {
            warn!(%err, "could not migrate the cached device certificates into the account namespace");
        }
    });
}

async fn run(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    context_client: &ContextClient,
) -> EyreResult<()> {
    let devices = NodeDeviceRepository::new(store);
    let cached = devices.legacy_device_certs()?;
    if cached.is_empty() {
        return Ok(());
    }

    // Only the account root can sign a scope, so a node that no longer holds one
    // leaves the rows for a node that does rather than dropping them.
    let Some(root) = devices.holder_root()? else {
        warn!(
            certificates = cached.len(),
            "cached device certificates, but no account root to sign their scopes; \
             leaving them where they are"
        );
        return Ok(());
    };
    let Some(namespace) = context_client
        .ensure_account_namespace(EnsureAccountNamespaceRequest)
        .await?
    else {
        return Ok(());
    };
    let Some((_signer_pk, signer_sk)) = NamespaceRepository::new(store).identity(&namespace)?
    else {
        eyre::bail!("the account namespace exists but this node takes no part in it");
    };
    let signer_sk = PrivateKey::from(signer_sk);

    let registry = AccountDeviceRegistry::new(store, namespace);
    let mut landed = true;
    for cert in &cached {
        publish_device_certified(
            store,
            node_client,
            ack_router,
            namespace,
            &signer_sk,
            &root,
            &cert.proof,
            &cert.applications,
            "account_migration",
        )
        .await;
        // The publish warns rather than failing, so the row its own apply wrote
        // is the only thing that says the statement landed.
        landed &= registry.device(cert.device())?.is_some();
    }

    // Only once every op is on the DAG. A row left behind is republished by the
    // next start, at a fresh scope epoch carrying the same statement.
    if !landed {
        warn!(
            certificates = cached.len(),
            "not every cached device certificate reached the account namespace; \
             leaving the rows for the next start"
        );
        return Ok(());
    }
    devices.forget_legacy_device_certs()?;
    info!(
        certificates = cached.len(),
        ?namespace,
        "migrated the cached device certificates into the account namespace"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::{AccountGenesis, AccountProof, DeviceCert, DeviceId, KemPublicKey};
    use calimero_governance_store::{AccountDeviceRegistry, AccountRoot, NodeDeviceRepository};
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{NodeAccountDeviceCert, NodeAccountDeviceCertValue};
    use calimero_store::Store;

    use super::run;
    use crate::test_support::actor;

    const APP_ONE: [u8; 32] = [0x11; 32];

    fn app(id: [u8; 32]) -> ApplicationId {
        ApplicationId::from(id)
    }

    /// The row a holder held before the registry existed, written the way the
    /// deleted cache wrote it: straight at the key, which is all that is left.
    fn cached_row(
        store: &Store,
        root_sk: &PrivateKey,
        seed: u8,
        applications: &[ApplicationId],
    ) -> DeviceId {
        let genesis = AccountGenesis::new(root_sk.public_key());
        let account = genesis.account_id();
        let device = DeviceId::from([seed; 32]);
        let proof = AccountProof {
            genesis,
            chain: vec![],
            statement: DeviceCert::sign(
                root_sk,
                account,
                device,
                &PrivateKey::from([seed; 32]).public_key(),
                &KemPublicKey::from([seed ^ 0xFF; 32]),
                0,
                0,
            )
            .expect("the account root signs its own device cert"),
        };
        store
            .handle()
            .put(
                &NodeAccountDeviceCert::new(*device.as_bytes()),
                &NodeAccountDeviceCertValue {
                    proof,
                    applications: applications.to_vec(),
                },
            )
            .expect("write the row a pre-registry holder held");
        device
    }

    fn holder_store() -> (Store, AccountRoot) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let root = NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the holder's root");
        (store, root)
    }

    /// One statement per cached row, at the scope the pairing gave it, and the
    /// rows gone afterwards so a second start republishes nothing.
    #[actix::test]
    async fn the_migration_publishes_one_op_per_cached_row_and_forgets_them() {
        let (store, root) = holder_store();
        let scoped = cached_row(&store, root.signing_key(), 0x61, &[app(APP_ONE)]);
        let wide = cached_row(&store, root.signing_key(), 0x62, &[]);

        let harness = actor::over(store.clone()).await;
        run(
            &store,
            &harness.node_client,
            harness.context_client.ack_router(),
            &harness.context_client,
        )
        .await
        .expect("the migration runs");

        let devices = NodeDeviceRepository::new(&store);
        let namespace = devices
            .account_namespace()
            .expect("read")
            .expect("the migration ensured it");
        let registry = AccountDeviceRegistry::new(&store, namespace);
        assert_eq!(
            registry
                .device(scoped)
                .expect("read")
                .expect("the scoped device is in the registry")
                .0
                .applications,
            vec![app(APP_ONE)],
        );
        assert!(registry
            .device(wide)
            .expect("read")
            .expect("the unscoped device is in the registry")
            .0
            .applications
            .is_empty());
        assert!(
            devices.legacy_device_certs().expect("read").is_empty(),
            "the rows must be gone, or every start republishes them"
        );
    }

    /// Only the account root can sign a scope. A node that lost its root - the
    /// key exported and removed - must leave the rows for one that holds it
    /// rather than drop them.
    #[actix::test]
    async fn a_cold_root_leaves_the_cached_rows_alone() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let _stranded = cached_row(&store, &PrivateKey::from([0x71; 32]), 0x63, &[]);

        let harness = actor::over(store.clone()).await;
        run(
            &store,
            &harness.node_client,
            harness.context_client.ack_router(),
            &harness.context_client,
        )
        .await
        .expect("nothing to do is not an error");

        assert_eq!(
            NodeDeviceRepository::new(&store)
                .legacy_device_certs()
                .expect("read")
                .len(),
            1,
        );
        assert_eq!(
            NodeDeviceRepository::new(&store)
                .account_namespace()
                .expect("read"),
            None,
            "and nothing was created for an account this node cannot speak for"
        );
    }
}
