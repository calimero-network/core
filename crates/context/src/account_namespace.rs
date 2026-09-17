//! Publishing into this node's account namespace: one device's row into its
//! registry, and one namespace into its set. One publisher per op, since the
//! scope epoch is minted from THIS node's folded row.
//!
//! The exception is a namespace's registry coordinates, which every device of
//! the account can see go stale; [`refresh_target`] gates on the staleness
//! itself instead of on an election.

use std::sync::Arc;
use std::time::Duration;

use calimero_account::{AccountProof, DeviceCert, DeviceScope};
use calimero_app_downloader::registry::stored_coords;
use calimero_context_client::local_governance::{AckRouter, GroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{
    AccountDeviceRegistry, AccountNamespaceSet, AccountRoot, KnownDeviceCert, MembershipRepository,
    MetaRepository, NamespaceRepository, NodeDeviceRepository,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::key::GroupAccountNamespaceTargetValue;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tokio::time::sleep;
use tracing::{debug, warn};

#[cfg(not(test))]
const TARGET_WAIT_INTERVAL: Duration = Duration::from_secs(1); // how often a target-less gain re-reads the namespace's meta
#[cfg(test)]
const TARGET_WAIT_INTERVAL: Duration = Duration::from_millis(20); // the same wait, shortened so tests do not sit out the real one
const TARGET_WAIT_ATTEMPTS: u32 = 30; // how many of those before the gain is announced with no target

/// Sign `applications` for `certificate`'s device at the next scope epoch. Minted
/// before publishing because every link made under it has to carry this statement.
pub fn next_device_scope(
    store: &Store,
    namespace: Option<ContextGroupId>,
    root: &AccountRoot,
    certificate: &AccountProof<DeviceCert>,
    applications: &[ApplicationId],
) -> EyreResult<AccountProof<DeviceScope>> {
    let device = certificate.statement.device;
    // No account namespace yet means no statement has ever been recorded, so the
    // first one starts the sequence exactly as an empty registry row would.
    let scope_epoch = match namespace {
        Some(namespace) => AccountDeviceRegistry::new(store, namespace)
            .device(device)?
            .map_or(0, |cert| cert.scope.statement.scope_epoch.saturating_add(1)),
        None => 0,
    };

    // Key epoch 0: the account root has not rotated (rotation is not implemented
    // yet), so the certifying key is the genesis key and there are no handoffs.
    let statement = DeviceScope::sign(
        root.signing_key(),
        certificate.statement.account,
        device,
        applications.to_vec(),
        scope_epoch,
        0,
    )
    .map_err(|err| eyre::eyre!("failed to sign the scope for {device}: {err}"))?;

    // Both anchors taken from the certificate, so the two proofs in one op can
    // never resolve the root key at different epochs.
    Ok(AccountProof {
        genesis: certificate.genesis,
        chain: certificate.chain.clone(),
        statement,
    })
}

/// Record `certificate` and `scope` in `namespace`'s registry. Never fails the
/// caller: it runs after the request's own work, so the next publish repairs it.
pub async fn publish_device_certified(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: ContextGroupId,
    signer_sk: &PrivateKey,
    known: &KnownDeviceCert,
    site: &'static str,
) -> bool {
    let (certificate, scope) = (&known.proof, &known.scope);
    let device = certificate.statement.device;
    match calimero_governance_store::sign_apply_and_publish(
        store,
        node_client,
        ack_router,
        &namespace,
        signer_sk,
        GroupOp::AccountDeviceCertified {
            certificate: Box::new(certificate.clone()),
            scope: Box::new(scope.clone()),
        },
    )
    .await
    {
        Ok(report) => report.observe(site, "AccountDeviceCertified"),
        Err(err) => {
            warn!(%device, %err, "the device was not recorded in the account namespace");
            return false;
        }
    }

    // The op's own local apply is the only durable write, and an apply that
    // refuses the statement warns rather than failing - so the row is what says it.
    let epoch = scope.statement.scope_epoch;
    match AccountDeviceRegistry::new(store, namespace).device(device) {
        Ok(Some(cert)) if cert.scope.statement.scope_epoch == epoch => true,
        Ok(_) => {
            warn!(%device, "the account namespace did not take the device's scope");
            false
        }
        Err(err) => {
            warn!(%device, %err, "could not confirm the device's registry row");
            false
        }
    }
}

/// What this node did to a namespace, from the account's point of view.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AccountNamespaceChange {
    Gained,
    Left,
}

/// Tell this account's other devices that this node gained or left `namespace`.
///
/// Best effort by design: no creation, join or leave may fail because a publish
/// did not, so every skip and failure is warned here rather than returned.
pub(crate) async fn announce(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &Arc<AckRouter>,
    namespace: ContextGroupId,
    change: AccountNamespaceChange,
    site: &'static str,
) {
    if let AccountNamespaceChange::Gained = change {
        // A namespace's meta can fold well after the bind that gained it, and no
        // event says when, so a gain with no target waits for one off the caller.
        if matches!(target_application(store, namespace), Ok(None)) {
            wait_for_target_then_announce(store, node_client, ack_router, namespace, site);
            return;
        }
    }
    publish_or_warn(store, node_client, ack_router, namespace, change, site).await;
}

/// Announce the gain of `namespace` once its target application is known, or
/// with none once [`TARGET_WAIT_ATTEMPTS`] have passed. Detached; never awaited.
fn wait_for_target_then_announce(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &Arc<AckRouter>,
    namespace: ContextGroupId,
    site: &'static str,
) {
    let store = store.clone();
    let node_client = node_client.clone();
    let ack_router = Arc::clone(ack_router);
    drop(tokio::spawn(async move {
        for _ in 0..TARGET_WAIT_ATTEMPTS {
            sleep(TARGET_WAIT_INTERVAL).await;
            if !matches!(target_application(&store, namespace), Ok(None)) {
                break;
            }
        }
        // A `Left` published inside the window would otherwise be undone here,
        // and nothing can drop a namespace the set has re-named.
        if !account_is_member(&store, namespace).unwrap_or_else(|err| {
            warn!(
                ?err,
                ?namespace,
                "could not confirm the account still holds a namespace"
            );
            false
        }) {
            debug!(
                ?namespace,
                "the account left it while the gain waited; dropping the gain"
            );
            return;
        }
        if matches!(target_application(&store, namespace), Ok(None)) {
            debug!(
                ?namespace,
                "no target application after waiting; announcing the gain with none"
            );
        }
        publish_or_warn(
            &store,
            &node_client,
            &ack_router,
            namespace,
            AccountNamespaceChange::Gained,
            site,
        )
        .await;
    }));
}

/// Is this node's account a member of `namespace`? Answered from the member row
/// a leave's own `MemberLeft` apply removes.
///
/// # Errors
/// Propagates the device or membership read failure.
pub(crate) fn account_is_member(store: &Store, namespace: ContextGroupId) -> EyreResult<bool> {
    let Some(held) = NodeDeviceRepository::new(store).get()? else {
        return Ok(false);
    };
    MembershipRepository::new(store).is_member(&namespace, &held.account)
}

async fn publish_or_warn(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: ContextGroupId,
    change: AccountNamespaceChange,
    site: &'static str,
) {
    if let Err(err) = publish(store, node_client, ack_router, namespace, change, site).await {
        warn!(
            ?err,
            ?namespace,
            ?change,
            "failed to tell this account's other devices about a namespace"
        );
    }
}

async fn publish(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: ContextGroupId,
    change: AccountNamespaceChange,
    site: &'static str,
) -> EyreResult<()> {
    let Some(account_namespace) = NodeDeviceRepository::new(store).account_namespace()? else {
        return Ok(());
    };
    if account_namespace == namespace {
        return Ok(());
    }
    // Participation, not the row: the holder names its account namespace before
    // creating it, and this is what says there is a DAG to write to.
    let Some((_signer_pk, signer_sk)) =
        NamespaceRepository::new(store).identity(&account_namespace)?
    else {
        return Ok(());
    };

    let op = match change {
        AccountNamespaceChange::Gained => GroupOp::AccountNamespaceGained {
            namespace,
            application: target_application(store, namespace)?,
        },
        AccountNamespaceChange::Left => GroupOp::AccountNamespaceLeft { namespace },
    };
    let op_kind = op.op_kind_label();
    let signer = PrivateKey::from(signer_sk);

    calimero_governance_store::sign_apply_and_publish(
        store,
        node_client,
        ack_router,
        &account_namespace,
        &signer,
        op,
    )
    .await?
    .observe(site, op_kind);

    // The op's own local apply is the only durable write, and an apply that
    // refuses the statement warns rather than failing - so the set is what says it.
    let named = AccountNamespaceSet::new(store, account_namespace)
        .contains(namespace)?
        .is_some();
    if named != matches!(change, AccountNamespaceChange::Gained) {
        warn!(
            ?namespace,
            ?change,
            "the account namespace did not take the change"
        );
    }

    // After the gain, never before it: the target op is a no-op for a namespace
    // the set does not name yet. Warned rather than propagated, so a gain that
    // landed is not reported as one that did not.
    if named {
        if let Err(err) = publish_target(
            store,
            node_client,
            ack_router,
            account_namespace,
            &signer,
            namespace,
            site,
        )
        .await
        {
            warn!(
                ?err,
                ?namespace,
                "gained a namespace but could not name where its application is published"
            );
        }
    }
    Ok(())
}

/// Name where `namespace`'s application is published, so a device whose scope
/// excludes it can still offer to install it.
///
/// Silent while either half is unfolded here - a `TargetApplicationSet` folded
/// later is what refreshes it, through [`refresh_target`].
async fn publish_target(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    account_namespace: ContextGroupId,
    signer: &PrivateKey,
    namespace: ContextGroupId,
    site: &'static str,
) -> EyreResult<()> {
    let Some(target) = target_coords(store, namespace)? else {
        debug!(
            ?namespace,
            "no registry coordinates folded here yet; announcing none"
        );
        return Ok(());
    };
    calimero_governance_store::sign_apply_and_publish(
        store,
        node_client,
        ack_router,
        &account_namespace,
        signer,
        GroupOp::AccountNamespaceTargetNamed {
            namespace,
            application: target.application,
            package: target.package,
            version: target.version,
        },
    )
    .await?
    .observe(site, "account_namespace_target_named");
    Ok(())
}

/// Re-announce `namespace`'s coordinates when a `TargetApplicationSet` left the
/// account's recorded pair stale. Nothing else refreshes it: a gain is announced
/// once and never restated.
///
/// Every device of the account that follows `namespace` folds that op, so the
/// single-publisher rule is the staleness gate itself rather than an election -
/// whichever device publishes first silences the others as they fold the result,
/// and a concurrent second publish writes the same pair.
pub(crate) async fn refresh_target(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: ContextGroupId,
) {
    let account_namespace = match stale_target(store, namespace) {
        Ok(Some(account_namespace)) => account_namespace,
        Ok(None) => return,
        Err(err) => {
            warn!(
                ?err,
                ?namespace,
                "could not tell whether this account's recorded coordinates are stale"
            );
            return;
        }
    };
    // Participation, not the row, exactly as `publish`: a node authorized to
    // write in the account namespace has an identity there, and one without
    // skips rather than publishing something the apply would refuse.
    let identity = match NamespaceRepository::new(store).identity(&account_namespace) {
        Ok(Some((_signer_pk, signer_sk))) => PrivateKey::from(signer_sk),
        Ok(None) => return,
        Err(err) => {
            warn!(
                ?err,
                "could not read this node's account-namespace identity"
            );
            return;
        }
    };
    if let Err(err) = publish_target(
        store,
        node_client,
        ack_router,
        account_namespace,
        &identity,
        namespace,
        "refresh_target",
    )
    .await
    {
        warn!(
            ?err,
            ?namespace,
            "failed to refresh a namespace's coordinates in this account's set"
        );
    }
}

/// The account namespace whose recorded coordinates for `namespace` no longer
/// match what is folded here, or `None` when there is nothing to re-announce.
fn stale_target(store: &Store, namespace: ContextGroupId) -> EyreResult<Option<ContextGroupId>> {
    let Some(account_namespace) = NodeDeviceRepository::new(store).account_namespace()? else {
        return Ok(None);
    };
    if account_namespace == namespace {
        return Ok(None);
    }
    let Some(target) = target_coords(store, namespace)? else {
        return Ok(None);
    };
    let set = AccountNamespaceSet::new(store, account_namespace);
    if set.contains(namespace)?.is_none() {
        return Ok(None);
    }
    let current = set.target(namespace)? == Some(target);
    Ok((!current).then_some(account_namespace))
}

/// The application `namespace` targets and the coordinates addressing it, as
/// folded here. `stored_coords` rejects both the unset pair and the placeholder
/// a raw-wasm row still carries, neither of which addresses a registry.
fn target_coords(
    store: &Store,
    namespace: ContextGroupId,
) -> EyreResult<Option<GroupAccountNamespaceTargetValue>> {
    let Some(meta) = MetaRepository::new(store).load(&namespace)? else {
        return Ok(None);
    };
    let application = meta.target.application_id;
    if *application.as_ref() == [0u8; 32] {
        return Ok(None);
    }
    Ok(
        stored_coords(&meta.target.package, &meta.target.version).map(|coords| {
            GroupAccountNamespaceTargetValue {
                application,
                package: coords.package.to_owned(),
                version: coords.version.to_owned(),
            }
        }),
    )
}

/// The application `namespace` targets, as folded here. The zero id a
/// cold-start seed writes reads as absent, never as an id to be scoped to.
fn target_application(
    store: &Store,
    namespace: ContextGroupId,
) -> EyreResult<Option<ApplicationId>> {
    Ok(MetaRepository::new(store)
        .load(&namespace)?
        .map(|meta| meta.target.application_id)
        .filter(|application| *application.as_ref() != [0u8; 32]))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::AccountId;
    use calimero_governance_store::{GroupKeyring, MembershipRepository};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupAccountNamespaceTargetValue, GroupMetaValue, GroupTarget};

    use super::{
        announce, refresh_target, stale_target, target_coords, AccountNamespaceChange,
        AccountNamespaceSet, ApplicationId, ContextGroupId, MetaRepository, NamespaceRepository,
        NodeDeviceRepository, Store,
    };
    use crate::test_support::{actor, enrol};

    const PROJECT: [u8; 32] = [0xD1; 32];

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn app(seed: u8) -> ApplicationId {
        ApplicationId::from([seed; 32])
    }

    /// This node as the HOLDER of its account: a root, its account namespace
    /// taken part in and keyed, and its own identity an admin there - the state
    /// `publish` needs before anything it signs is taken.
    fn a_holder(store: &Store) -> ContextGroupId {
        let devices = NodeDeviceRepository::new(store);
        let account_namespace = devices
            .provision_account_root()
            .expect("mint an account root")
            .account_namespace();
        let namespaces = NamespaceRepository::new(store);
        let _identity = namespaces
            .participate_in(&account_namespace)
            .expect("take part in its own account namespace");
        let _key_id = GroupKeyring::new(store, account_namespace)
            .store_key(&[0x42; 32])
            .expect("hold its key, without which nothing is published");
        a_namespace_published_as(store, account_namespace, None);
        let (sign_pk, _secret) = namespaces
            .resolve_identity(&account_namespace)
            .expect("read this node's identity here")
            .expect("taking part in a namespace mints one");
        let account = enrol(store, &account_namespace, &sign_pk);
        MembershipRepository::new(store)
            .add_member(&account_namespace, &account, GroupMemberRole::Admin)
            .expect("and an admin of it, which is what the gate asks");
        account_namespace
    }

    /// A namespace as its founder leaves it: a meta row naming a target, and
    /// `None` for the account namespace, whose target is unset by construction.
    fn a_namespace_published_as(
        store: &Store,
        namespace: ContextGroupId,
        target: Option<(ApplicationId, &str, &str)>,
    ) {
        let target = target.map_or_else(GroupTarget::default, |(application, package, version)| {
            GroupTarget {
                application_id: application,
                bytecode_id: [0u8; 32],
                package: package.into(),
                version: version.into(),
            }
        });
        MetaRepository::new(store)
            .save(
                &namespace,
                &GroupMetaValue {
                    target,
                    created_at: 1_700_000_000,
                    admin_identity: AccountId::from([0x01; 32]),
                    owner_identity: AccountId::from([0x01; 32]),
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("save the namespace metadata");
    }

    /// Both halves or nothing. A target with no addressable coordinates is what
    /// a raw-wasm install leaves, and it must not be announced as a location.
    #[test]
    fn coordinates_need_a_target_and_a_real_registry_pair() {
        let store = store();
        let project = ContextGroupId::from(PROJECT);

        assert_eq!(target_coords(&store, project).expect("read"), None);

        a_namespace_published_as(&store, project, Some((app(0x11), "", "")));
        assert_eq!(target_coords(&store, project).expect("read"), None);

        a_namespace_published_as(&store, project, Some((app(0x11), "unknown", "0.0.0")));
        assert_eq!(
            target_coords(&store, project).expect("read"),
            None,
            "the raw-wasm placeholder addresses no registry"
        );

        a_namespace_published_as(&store, project, Some((app(0x11), "com.acme.app", "1.0.0")));
        assert_eq!(
            target_coords(&store, project).expect("read"),
            Some(GroupAccountNamespaceTargetValue {
                application: app(0x11),
                package: "com.acme.app".to_owned(),
                version: "1.0.0".to_owned(),
            })
        );
    }

    /// The gain and the coordinates travel together, so a device that never
    /// follows the namespace still learns where to install its application.
    #[actix::test]
    async fn a_gain_names_where_the_namespace_is_published() {
        let store = store();
        let account_namespace = a_holder(&store);
        let project = ContextGroupId::from(PROJECT);
        a_namespace_published_as(&store, project, Some((app(0x11), "com.acme.app", "1.0.0")));

        let harness = actor::over(store.clone()).await;
        announce(
            &store,
            &harness.node_client,
            harness.context_client.ack_router(),
            project,
            AccountNamespaceChange::Gained,
            "test",
        )
        .await;

        let set = AccountNamespaceSet::new(&store, account_namespace);
        assert_eq!(set.contains(project).expect("read"), Some(Some(app(0x11))));
        let named = set.target(project).expect("read").expect("named");
        assert_eq!(named.application, app(0x11));
        assert_eq!(named.package, "com.acme.app");
        assert_eq!(named.version, "1.0.0");
    }

    /// A gain announced before the coordinates folded records none, and nothing
    /// restates a gain - so the later `TargetApplicationSet` has to name them.
    #[actix::test]
    async fn coordinates_that_arrive_after_the_gain_are_named_by_the_refresh() {
        let store = store();
        let account_namespace = a_holder(&store);
        let project = ContextGroupId::from(PROJECT);
        a_namespace_published_as(&store, project, Some((app(0x11), "", "")));

        let harness = actor::over(store.clone()).await;
        announce(
            &store,
            &harness.node_client,
            harness.context_client.ack_router(),
            project,
            AccountNamespaceChange::Gained,
            "test",
        )
        .await;

        let set = AccountNamespaceSet::new(&store, account_namespace);
        assert!(set.contains(project).expect("read").is_some(), "gained");
        assert_eq!(
            set.target(project).expect("read"),
            None,
            "a gain with no addressable coordinates must announce none"
        );

        a_namespace_published_as(&store, project, Some((app(0x22), "com.acme.app", "2.0.0")));
        assert_eq!(
            stale_target(&store, project).expect("read"),
            Some(account_namespace)
        );
        refresh_target(
            &store,
            &harness.node_client,
            harness.context_client.ack_router(),
            project,
        )
        .await;

        let named = set.target(project).expect("read").expect("named");
        assert_eq!(named.application, app(0x22));
        assert_eq!(named.version, "2.0.0");
        assert_eq!(
            stale_target(&store, project).expect("read"),
            None,
            "the pair now recorded is what is folded here, so nobody re-announces"
        );
    }

    /// The refresh is gated on the record, not on an election: a namespace this
    /// account never gained is not one it names coordinates for.
    #[test]
    fn a_namespace_the_account_never_gained_is_never_refreshed() {
        let store = store();
        let account_namespace = a_holder(&store);
        let project = ContextGroupId::from(PROJECT);
        a_namespace_published_as(&store, project, Some((app(0x11), "com.acme.app", "1.0.0")));

        assert_eq!(stale_target(&store, project).expect("read"), None);
        assert_eq!(
            stale_target(&store, account_namespace).expect("read"),
            None,
            "the account namespace is never in its own set"
        );
    }
}
