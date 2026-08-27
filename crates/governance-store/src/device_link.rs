//! Extending a device this account already certified into one more namespace.
//!
//! Cheap because a [`DeviceCert`](calimero_account::DeviceCert) names no namespace
//! and no expiry, so this needs only the stored certificate, a fresh endorsement
//! and one key wrap. One publish path for all three callers: pairing's fan-out,
//! the auto-bind on gaining a namespace, and the relink that repairs drift.

use calimero_account::{AccountMemberEndorsement, AccountProof, DeviceCert, DeviceId};
use calimero_context_client::group::BindOutcome;
use calimero_context_client::local_governance::{AckRouter, GroupOp, NamespaceOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::X25519PublicKey;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{info, warn};

use crate::{
    AccountBindingRepository, GroupKeyring, KnownDeviceCert, MetaRepository, NodeDeviceRepository,
};

/// Publish, or the reason not to.
enum BindPlan {
    /// Go ahead, under this scope key.
    Publish([u8; 32]),
    Skip(BindOutcome),
}

/// Everything that decides whether the pair of publishes is worth making,
/// cheapest question first.
fn plan(store: &Store, namespace: &ContextGroupId, cert: &KnownDeviceCert) -> EyreResult<BindPlan> {
    let device = cert.device();
    let devices = NodeDeviceRepository::new(store);

    if devices.get()?.is_some_and(|held| held.device() == device) {
        return Ok(BindPlan::Skip(BindOutcome::OwnDevice));
    }
    // The namespace's target application, read the way the pairing fan-out reads
    // it. A namespace whose metadata has not synced names none, and is reachable
    // only by a scope that names none either.
    let application = MetaRepository::new(store)
        .load(namespace)?
        .map(|meta| meta.target.application_id);
    if !cert.covers(application) {
        return Ok(BindPlan::Skip(BindOutcome::OutOfScope));
    }

    let bindings = AccountBindingRepository::new(store);
    if bindings.is_revoked(namespace, device)? {
        return Ok(BindPlan::Skip(BindOutcome::Revoked));
    }
    if bindings.is_device_linked(namespace, device)? {
        return Ok(BindPlan::Skip(BindOutcome::AlreadyBound));
    }

    let Some((_key_id, ns_key)) = GroupKeyring::new(store, *namespace).load_current_key()? else {
        return Ok(BindPlan::Skip(BindOutcome::NoScopeKey));
    };
    Ok(BindPlan::Publish(ns_key))
}

/// Publish the two ops that make a device usable in `namespace`.
///
/// The link confers authority; the delivery hands over the key without which the
/// link lands and the device still cannot read anything. Both are load-bearing,
/// which is why they live in one function rather than at three call sites.
///
/// The delivery has to be a cleartext `RootOp`: a device-addressed envelope
/// carried inside an *encrypted* group op would be unreadable by its only
/// recipient.
///
/// The endorsement is minted here from the key that also signs the ops: taking one
/// signing key makes crossing the root and the namespace identity unrepresentable,
/// and a crossed pair looks healthy locally while every peer refuses it.
///
/// `Ok(false)` means the link landed and the delivery did not - not a failed
/// bind. An `Err` means nothing was published: the wrap runs first precisely so a
/// device this node cannot address never gets a link published for it.
async fn publish_link_and_key(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: &ContextGroupId,
    signer_sk: &PrivateKey,
    proof: &AccountProof<DeviceCert>,
    ns_key: &[u8; 32],
) -> EyreResult<bool> {
    let cert = &proof.statement;
    let device = cert.device;

    // Wrapped under the KEM key the certificate names rather than one read off a
    // folded binding, so the delivery does not depend on this node having already
    // folded the link it is about to publish.
    let envelope = GroupKeyring::wrap_for_device(
        signer_sk,
        device,
        &X25519PublicKey::from(*cert.kem_pk.as_bytes()),
        &namespace.to_bytes(),
        ns_key,
    )?;

    // Only a member can endorse and only the root can certify; the apply gate
    // needs both, and a fresh endorsement is all a namespace gained later is
    // missing.
    let endorsement = AccountMemberEndorsement::sign(signer_sk, cert.account)
        .map_err(|err| eyre::eyre!("failed to endorse device {device}: {err}"))?;
    let link = GroupOp::AccountDeviceLinked {
        genesis: proof.genesis,
        chain: proof.chain.clone(),
        cert: *cert,
        endorsement,
    };

    let report =
        crate::sign_apply_and_publish(store, node_client, ack_router, namespace, signer_sk, link)
            .await?;
    info!(
        namespace_id = ?namespace,
        %device,
        published = report.is_some(),
        "linked a device of this account"
    );

    // `required_signers` is None because the device is not a member and so is not
    // among the acking set - its receipt shows up as the device being able to
    // read, not as an ack.
    let delivery = NamespaceOp::Root(RootOp::KeyDelivery {
        group_id: namespace.to_bytes().into(),
        envelope,
    });
    if let Err(err) = crate::sign_and_publish_namespace_op(
        store,
        node_client,
        ack_router,
        namespace.to_bytes().into(),
        signer_sk,
        delivery,
        None,
    )
    .await
    {
        warn!(
            ?err,
            namespace_id = ?namespace,
            %device,
            "device linked but the scope-key delivery failed to publish; \
             the device's sync pull is the durable retry"
        );
        return Ok(false);
    }
    Ok(true)
}

/// Bind `cert`'s device into `namespace` unless something makes it pointless or
/// forbidden.
///
/// Never fails: every outcome is a value. A namespace gain is the primary
/// operation and extending a device into it is best-effort on top, so a caller
/// must not be able to propagate a failure here by accident.
async fn ensure_bound(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: &ContextGroupId,
    signer_sk: &PrivateKey,
    cert: &KnownDeviceCert,
) -> BindOutcome {
    let device = cert.device();
    let ns_key = match plan(store, namespace, cert) {
        Ok(BindPlan::Publish(ns_key)) => ns_key,
        Ok(BindPlan::Skip(outcome)) => return outcome,
        Err(err) => {
            warn!(namespace_id = ?namespace, %device, %err,
                  "could not decide whether to bind this device here; leaving it unbound");
            return BindOutcome::Failed;
        }
    };

    match publish_link_and_key(
        store,
        node_client,
        ack_router,
        namespace,
        signer_sk,
        &cert.proof,
        &ns_key,
    )
    .await
    {
        Ok(key_delivered) => BindOutcome::Linked { key_delivered },
        Err(err) => {
            warn!(namespace_id = ?namespace, %device, %err,
                  "could not extend a known device into this namespace");
            BindOutcome::Failed
        }
    }
}

/// Extend every device this account has already certified into `namespace`.
///
/// Runs when this node gains a namespace and holds its scope key. Best-effort by
/// construction: it returns what happened rather than a `Result`, so a join or a
/// creation cannot fail because one device could not be carried across.
pub async fn bind_known_devices(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: &ContextGroupId,
    signer_sk: &PrivateKey,
) -> Vec<(DeviceId, BindOutcome)> {
    let certs = match NodeDeviceRepository::new(store).device_certs() {
        Ok(certs) => certs,
        Err(err) => {
            warn!(namespace_id = ?namespace, %err,
                  "could not read this account's device certificates; binding none here");
            return Vec::new();
        }
    };

    let mut outcomes = Vec::with_capacity(certs.len());
    for cert in &certs {
        let outcome =
            ensure_bound(store, node_client, ack_router, namespace, signer_sk, cert).await;
        outcomes.push((cert.device(), outcome));
    }
    if !outcomes.is_empty() {
        info!(namespace_id = ?namespace, ?outcomes, "carried this account's devices into a namespace");
    }
    outcomes
}

/// Extend one device into every namespace it should reach.
///
/// The transpose of [`bind_known_devices`] and the repair half of the same
/// mechanism: that one asks "which devices belong in this new namespace", this
/// one asks "which namespaces is this device missing from". Best-effort on the
/// same terms - an outcome per namespace, never a `Result`.
pub async fn bind_device_everywhere(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespaces: &[ContextGroupId],
    signer_sk: &PrivateKey,
    cert: &KnownDeviceCert,
) -> Vec<(ContextGroupId, BindOutcome)> {
    let mut outcomes = Vec::with_capacity(namespaces.len());
    for namespace in namespaces {
        let outcome =
            ensure_bound(store, node_client, ack_router, namespace, signer_sk, cert).await;
        outcomes.push((*namespace, outcome));
    }
    outcomes
}

#[cfg(test)]
mod tests {
    use calimero_account::{AccountGenesis, AccountProof, DeviceCert, KemPublicKey};
    use calimero_primitives::identity::PublicKey;
    use calimero_store::key::GroupMetaValue;

    use super::*;
    use crate::test_fixtures::{namespace_publish_fixture, test_group_id, test_store};
    use crate::{AccountBindingRepository, MembershipRepository};

    const APP_ONE: [u8; 32] = [0x11; 32];
    const APP_TWO: [u8; 32] = [0x22; 32];

    fn app(id: [u8; 32]) -> calimero_primitives::application::ApplicationId {
        calimero_primitives::application::ApplicationId::from(id)
    }

    /// The account root [`crate::test_fixtures::enrol_member`] derives for a
    /// signing key, so a second device can be certified into the same account the
    /// fixture already made a member.
    fn account_root_of(sign_pk: &PublicKey) -> PrivateKey {
        PrivateKey::from(*(*sign_pk))
    }

    /// A device of `root_sk`'s account, with `seed` deciding its id.
    ///
    /// The id is `seed` repeated rather than minted, so the store's key-ordered
    /// scan visits these devices in a known order - which one test needs in order
    /// to prove that a failure early in the scan does not stop the ones after it.
    fn certify(root_sk: &PrivateKey, seed: u8, kem: [u8; 32]) -> AccountProof<DeviceCert> {
        let genesis = AccountGenesis::new(root_sk.public_key());
        let account = genesis.account_id();
        let statement = DeviceCert::sign(
            root_sk,
            account,
            calimero_account::DeviceId::from([seed; 32]),
            &PrivateKey::from([seed; 32]).public_key(),
            &KemPublicKey::from(kem),
            0,
            0,
        )
        .expect("sign the certificate");
        AccountProof {
            genesis,
            chain: vec![],
            statement,
        }
    }

    fn known(root_sk: &PrivateKey, seed: u8, applications: Vec<[u8; 32]>) -> KnownDeviceCert {
        KnownDeviceCert {
            proof: certify(root_sk, seed, [seed ^ 0xFF; 32]),
            applications: applications.into_iter().map(app).collect(),
        }
    }

    /// A namespace serving `application`, with a scope key this node holds - the
    /// state a node is in the moment it has gained one.
    fn namespace_serving(store: &Store, namespace: &ContextGroupId, application: [u8; 32]) {
        MetaRepository::new(store)
            .save(
                namespace,
                &GroupMetaValue {
                    target: calimero_store::key::GroupTarget {
                        application_id: app(application),
                        bytecode_id: [0xAA; 32],
                        ..Default::default()
                    },
                    created_at: 1_700_000_000,
                    admin_identity: calimero_account::AccountId::from([0x01; 32]),
                    owner_identity: calimero_account::AccountId::from([0x01; 32]),
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("save the namespace metadata");
        let _key_id = GroupKeyring::new(store, *namespace)
            .store_key(&[0x42; 32])
            .expect("store a scope key");
    }

    fn planned(store: &Store, namespace: &ContextGroupId, cert: &KnownDeviceCert) -> BindOutcome {
        match plan(store, namespace, cert).expect("plan") {
            BindPlan::Publish(_) => BindOutcome::Linked {
                key_delivered: true,
            },
            BindPlan::Skip(outcome) => outcome,
        }
    }

    #[test]
    fn a_device_whose_scope_covers_this_namespace_is_carried_into_it() {
        let store = test_store();
        let ns = test_group_id();
        namespace_serving(&store, &ns, APP_ONE);
        let root = PrivateKey::from([0x51; 32]);

        for scope in [vec![], vec![APP_ONE], vec![APP_TWO, APP_ONE]] {
            assert_eq!(
                planned(&store, &ns, &known(&root, 0x61, scope.clone())),
                BindOutcome::Linked {
                    key_delivered: true
                },
                "scope {scope:?} covers this namespace's application"
            );
        }
    }

    #[test]
    fn a_device_scoped_to_another_application_is_left_alone() {
        let store = test_store();
        let ns = test_group_id();
        namespace_serving(&store, &ns, APP_ONE);
        let root = PrivateKey::from([0x52; 32]);

        assert_eq!(
            planned(&store, &ns, &known(&root, 0x62, vec![APP_TWO])),
            BindOutcome::OutOfScope,
        );
    }

    /// A namespace whose metadata has not synced names no application, so only a
    /// scope that names none reaches it - the same answer the pairing fan-out
    /// gives such a namespace.
    #[test]
    fn a_namespace_that_names_no_application_is_reached_only_by_the_widest_scope() {
        let store = test_store();
        let ns = test_group_id();
        let _key_id = GroupKeyring::new(&store, ns)
            .store_key(&[0x42; 32])
            .expect("store a scope key");
        let root = PrivateKey::from([0x53; 32]);

        assert_eq!(
            planned(&store, &ns, &known(&root, 0x63, vec![])),
            BindOutcome::Linked {
                key_delivered: true
            },
        );
        assert_eq!(
            planned(&store, &ns, &known(&root, 0x64, vec![APP_ONE])),
            BindOutcome::OutOfScope,
        );
    }

    /// Revocation is terminal: the id is spent for good, so no later namespace
    /// gain may quietly hand it back.
    #[test]
    fn a_revoked_device_is_never_carried_into_a_namespace() {
        let store = test_store();
        let ns = test_group_id();
        namespace_serving(&store, &ns, APP_ONE);
        let root = PrivateKey::from([0x54; 32]);
        let cert = known(&root, 0x65, vec![APP_ONE]);

        AccountBindingRepository::new(&store)
            .apply_revocation(&ns, cert.device())
            .expect("tombstone the device");

        assert_eq!(planned(&store, &ns, &cert), BindOutcome::Revoked);
    }

    /// Idempotent, and cheaply so: a live binding means there is nothing to
    /// repair, and republishing would cost an encrypted op and a key delivery for
    /// no change.
    #[test]
    fn a_device_already_bound_here_is_not_republished() {
        let store = test_store();
        let ns = test_group_id();
        namespace_serving(&store, &ns, APP_ONE);
        let root = PrivateKey::from([0x55; 32]);
        let cert = known(&root, 0x66, vec![APP_ONE]);

        let _binding = AccountBindingRepository::new(&store)
            .apply_link(
                &ns,
                &cert.proof.genesis,
                &cert.proof.chain,
                &cert.proof.statement,
            )
            .expect("store the binding")
            .expect("the credential is admissible");

        assert_eq!(planned(&store, &ns, &cert), BindOutcome::AlreadyBound);
    }

    /// Without the current key this node can neither publish an encrypted group
    /// op nor deliver one, so there is nothing to do here yet.
    #[test]
    fn a_namespace_this_node_holds_no_key_in_is_skipped() {
        let store = test_store();
        let ns = test_group_id();
        MetaRepository::new(&store)
            .save(
                &ns,
                &GroupMetaValue {
                    target: calimero_store::key::GroupTarget {
                        application_id: app(APP_ONE),

                        bytecode_id: [0xAA; 32],

                        ..Default::default()
                    },
                    created_at: 1_700_000_000,
                    admin_identity: calimero_account::AccountId::from([0x01; 32]),
                    owner_identity: calimero_account::AccountId::from([0x01; 32]),
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("save the namespace metadata");
        let root = PrivateKey::from([0x56; 32]);

        assert_eq!(
            planned(&store, &ns, &known(&root, 0x67, vec![])),
            BindOutcome::NoScopeKey,
        );
    }

    /// Ordinary enrolment already binds this node's own device, and it does it
    /// with the replica state the id carries - re-publishing it here would be a
    /// second, weaker path to the same row.
    #[test]
    fn this_nodes_own_device_is_left_to_enrolment() {
        let store = test_store();
        let ns = test_group_id();
        namespace_serving(&store, &ns, APP_ONE);

        let held = NodeDeviceRepository::new(&store)
            .ensure_enrolled(&ns)
            .expect("mint this node's device");
        let root = NodeDeviceRepository::new(&store)
            .account_root()
            .expect("read")
            .expect("present");
        let cert = KnownDeviceCert {
            proof: AccountProof {
                genesis: held.genesis,
                chain: vec![],
                statement: DeviceCert::sign(
                    root.signing_key(),
                    held.account,
                    held.device(),
                    &PrivateKey::from([0x68; 32]).public_key(),
                    &held.kem_public_key(),
                    0,
                    0,
                )
                .expect("sign"),
            },
            applications: Vec::new(),
        };

        assert_eq!(planned(&store, &ns, &cert), BindOutcome::OwnDevice);
    }

    /// The whole point, through the real publish path: a namespace gained after
    /// the pairing binds the device the pairing certified, with no ceremony and
    /// with the device offline.
    #[actix::test]
    async fn gaining_a_namespace_binds_a_device_the_pairing_already_certified() {
        let (store, node_client, ack_router, ns_id, sk, _tmp, _msgs) =
            namespace_publish_fixture().await;
        let ns = ContextGroupId::from(ns_id.to_bytes());
        let root = account_root_of(&sk.public_key());
        let devices = NodeDeviceRepository::new(&store);

        let in_scope = certify(&root, 0x71, [0x71; 32]);
        let out_of_scope = certify(&root, 0x72, [0x72; 32]);
        devices
            .remember_device_cert(&in_scope, &[])
            .expect("remember the paired device");
        devices
            .remember_device_cert(&out_of_scope, &[app(APP_TWO)])
            .expect("remember a device scoped elsewhere");
        let _key_id = GroupKeyring::new(&store, ns)
            .store_key(&[0x42; 32])
            .expect("hold the scope key");

        let outcomes = bind_known_devices(&store, &node_client, &ack_router, &ns, &sk).await;

        assert_eq!(
            outcomes
                .iter()
                .find(|(device, _)| *device == in_scope.statement.device)
                .map(|(_, outcome)| *outcome),
            Some(BindOutcome::Linked {
                key_delivered: true
            }),
        );
        assert_eq!(
            outcomes
                .iter()
                .find(|(device, _)| *device == out_of_scope.statement.device)
                .map(|(_, outcome)| *outcome),
            Some(BindOutcome::OutOfScope),
        );

        let live: Vec<_> = AccountBindingRepository::new(&store)
            .live_bindings(&ns)
            .expect("read the bindings")
            .into_iter()
            .map(|binding| binding.device)
            .collect();
        assert!(
            live.contains(&in_scope.statement.device),
            "the link has to have APPLIED, not merely been published"
        );
        assert!(!live.contains(&out_of_scope.statement.device));
    }

    /// The repair, from the other side: one device, every namespace this node
    /// takes part in. The namespace it is missing from gets the link; the one this
    /// node cannot publish into is reported rather than silently dropped, because
    /// a partially reached device is a state an operator has to be able to see.
    #[actix::test]
    async fn repairing_one_device_reaches_the_namespaces_it_is_missing_from() {
        let (store, node_client, ack_router, ns_id, sk, _tmp, _msgs) =
            namespace_publish_fixture().await;
        let repaired = ContextGroupId::from(ns_id.to_bytes());
        // A second namespace this node takes part in but holds no key for, which is
        // the ordinary "not caught up yet" state rather than a fault.
        let keyless = ContextGroupId::from([0xEE; 32]);
        let root = account_root_of(&sk.public_key());
        let cert = KnownDeviceCert {
            proof: certify(&root, 0x77, [0x77; 32]),
            applications: Vec::new(),
        };
        let _key_id = GroupKeyring::new(&store, repaired)
            .store_key(&[0x42; 32])
            .expect("hold the scope key where the repair can land");

        let outcomes = bind_device_everywhere(
            &store,
            &node_client,
            &ack_router,
            &[repaired, keyless],
            &sk,
            &cert,
        )
        .await;

        assert_eq!(
            outcomes,
            vec![
                (
                    repaired,
                    BindOutcome::Linked {
                        key_delivered: true
                    }
                ),
                (keyless, BindOutcome::NoScopeKey),
            ],
        );
        assert!(
            AccountBindingRepository::new(&store)
                .live_bindings(&repaired)
                .expect("read the bindings")
                .into_iter()
                .any(|binding| binding.device == cert.device()),
            "the missing binding has to have been repaired, not merely reported"
        );
    }

    /// A device this node cannot address takes nothing down with it. The gain is
    /// the primary operation; carrying devices across is best-effort on top, and
    /// the caller is handed outcomes rather than a `Result` so it cannot fail on
    /// one by accident.
    #[actix::test]
    async fn a_device_that_cannot_be_bound_does_not_stop_the_rest() {
        let (store, node_client, ack_router, ns_id, sk, _tmp, _msgs) =
            namespace_publish_fixture().await;
        let ns = ContextGroupId::from(ns_id.to_bytes());
        let root = account_root_of(&sk.public_key());
        let devices = NodeDeviceRepository::new(&store);

        // An all-zero agreement key is a degenerate X25519 point: the scope key
        // cannot be wrapped for it, so nothing about this device is publishable.
        let unaddressable = certify(&root, 0x73, [0u8; 32]);
        let healthy = certify(&root, 0x74, [0x74; 32]);
        devices
            .remember_device_cert(&unaddressable, &[])
            .expect("remember");
        devices
            .remember_device_cert(&healthy, &[])
            .expect("remember");
        let _key_id = GroupKeyring::new(&store, ns)
            .store_key(&[0x42; 32])
            .expect("hold the scope key");

        // The scan is key-ordered, so this pins that the failure really is the
        // FIRST device the loop meets - otherwise "the rest continue" would be
        // proved by an ordering rather than by the loop.
        assert_eq!(
            devices
                .device_certs()
                .expect("scan")
                .first()
                .map(KnownDeviceCert::device),
            Some(unaddressable.statement.device),
        );

        let outcomes = bind_known_devices(&store, &node_client, &ack_router, &ns, &sk).await;

        assert_eq!(
            outcomes
                .iter()
                .find(|(device, _)| *device == unaddressable.statement.device)
                .map(|(_, outcome)| *outcome),
            Some(BindOutcome::Failed),
        );
        assert_eq!(
            outcomes
                .iter()
                .find(|(device, _)| *device == healthy.statement.device)
                .map(|(_, outcome)| *outcome),
            Some(BindOutcome::Linked {
                key_delivered: true
            }),
        );
        let live: Vec<_> = AccountBindingRepository::new(&store)
            .live_bindings(&ns)
            .expect("read the bindings")
            .into_iter()
            .map(|binding| binding.device)
            .collect();
        assert!(
            live.contains(&healthy.statement.device),
            "the device that could be bound must still have been"
        );
        assert!(
            !live.contains(&unaddressable.statement.device),
            "and the one that could not must not have been linked either: the wrap \
             runs first so no link is published for a device that would never receive \
             the key"
        );
    }

    /// Terminal through the real path too, not merely in the decision: a
    /// tombstoned device must not be handed a fresh binding by a namespace gain.
    #[actix::test]
    async fn a_revoked_device_is_not_re_bound_by_a_namespace_gain() {
        let (store, node_client, ack_router, ns_id, sk, _tmp, _msgs) =
            namespace_publish_fixture().await;
        let ns = ContextGroupId::from(ns_id.to_bytes());
        let root = account_root_of(&sk.public_key());

        let revoked = certify(&root, 0x75, [0x75; 32]);
        NodeDeviceRepository::new(&store)
            .remember_device_cert(&revoked, &[])
            .expect("remember");
        AccountBindingRepository::new(&store)
            .apply_revocation(&ns, revoked.statement.device)
            .expect("tombstone the device");
        let _key_id = GroupKeyring::new(&store, ns)
            .store_key(&[0x42; 32])
            .expect("hold the scope key");

        let outcomes = bind_known_devices(&store, &node_client, &ack_router, &ns, &sk).await;

        assert_eq!(
            outcomes,
            vec![(revoked.statement.device, BindOutcome::Revoked)],
        );
        assert!(AccountBindingRepository::new(&store)
            .live_bindings(&ns)
            .expect("read the bindings")
            .into_iter()
            .all(|binding| binding.device != revoked.statement.device));
    }

    /// The endorsement is minted at bind time by whoever is a member NOW, which
    /// is what makes this possible at all - the certificate carries no namespace,
    /// and the creator of a namespace is a member at the cut its own genesis
    /// established.
    #[actix::test]
    async fn the_binder_endorses_as_the_member_it_is_at_this_cut() {
        let (store, node_client, ack_router, ns_id, sk, _tmp, _msgs) =
            namespace_publish_fixture().await;
        let ns = ContextGroupId::from(ns_id.to_bytes());
        let root = account_root_of(&sk.public_key());
        let cert = certify(&root, 0x76, [0x76; 32]);
        NodeDeviceRepository::new(&store)
            .remember_device_cert(&cert, &[])
            .expect("remember");
        let _key_id = GroupKeyring::new(&store, ns)
            .store_key(&[0x42; 32])
            .expect("hold the scope key");

        // Strip the binder's membership: the endorsement it signs is then from a
        // non-member and the apply refuses the link, which is the gate this whole
        // path has to satisfy rather than route around.
        let endorser = crate::member_account_in_namespace(&store, &ns, &sk.public_key())
            .expect("resolve")
            .expect("the fixture enrols the admin");
        MembershipRepository::new(&store)
            .remove_member(&ns, &endorser)
            .expect("remove the binder from the namespace");

        let _outcomes = bind_known_devices(&store, &node_client, &ack_router, &ns, &sk).await;

        assert!(
            AccountBindingRepository::new(&store)
                .live_bindings(&ns)
                .expect("read the bindings")
                .into_iter()
                .all(|binding| binding.device != cert.statement.device),
            "a link endorsed by a non-member must not record a binding"
        );
    }
}
