//! `RelinkDeviceRequest` handler - repair or widen a device's bindings without
//! re-pairing it, by re-running pairing's fan-out against the namespaces this
//! node takes part in now. A revoked device is refused outright rather than
//! skipped per namespace: the `DeviceId` is spent everywhere, not just where the
//! tombstone landed.
//!
//! It closes by publishing `AccountDeviceCertified` into the account namespace,
//! restating the device's certificate and scope at the next scope epoch.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::DeviceId;
use calimero_context_client::group::{RelinkDeviceRequest, RelinkDeviceResponse};
use calimero_governance_store::{
    AccountDeviceRegistry, AccountRoot, KnownDeviceCert, NamespaceRepository, NodeDeviceRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::info;

use crate::error::ContextError;
use crate::handlers::pair_device_complete::{
    require_not_revoked, require_this_node_holds, signing_identity,
};
use crate::ContextManager;

/// The certificate a relink will re-publish, and the scope it will use.
///
/// Every refusal lives here, in the order a caller can act on: wrong machine,
/// unknown device, spent id. The widened scope is returned rather than written:
/// the statement the handler publishes into the account namespace is what makes
/// it durable, and what every LATER namespace gain on any device is judged
/// against.
fn resolve_target(
    store: &Store,
    device: DeviceId,
    applications: Vec<ApplicationId>,
) -> EyreResult<(AccountRoot, KnownDeviceCert)> {
    let devices = NodeDeviceRepository::new(store);

    // The account root decides which account this node may extend a device into:
    // the genesis is the content address of that root, so a node that paired INTO
    // somebody else's account holds no root that could have signed the
    // certificate it would be re-publishing.
    let root = devices.require_account_root()?;
    require_this_node_holds(store, root.account())?;

    // The registry rather than the node-local cache: it is replicated, so a
    // sibling this node never certified itself is still one it can relink.
    let registry = AccountDeviceRegistry::new(store, root.account_namespace());
    let Some((mut cached, _scope_epoch)) = registry.device(device)? else {
        return Err(ContextError::PairingUnknownDevice {
            device: device.to_string(),
        }
        .into());
    };

    require_not_revoked(store, device)?;

    // An empty stored scope already covers every application, so adding to it could
    // only narrow the device rather than widen it.
    if !applications.is_empty() && !cached.applications.is_empty() {
        for application in applications {
            if !cached.applications.contains(&application) {
                cached.applications.push(application);
            }
        }
    }

    Ok((root, cached))
}

impl Handler<RelinkDeviceRequest> for ContextManager {
    type Result = ActorResponse<Self, <RelinkDeviceRequest as Message>::Result>;

    fn handle(
        &mut self,
        RelinkDeviceRequest {
            device,
            applications,
        }: RelinkDeviceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let store = self.datastore.clone();

        let (root, cached) = match resolve_target(&store, device, applications) {
            Ok(target) => target,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let account = root.account();

        // Every namespace this node takes part in, narrowed by the device's own
        // scope inside the loop. Participation is the base set for the same reason
        // pairing's fan-out uses it: publishing needs this node's identity and
        // scope key, so a namespace it merely knows the metadata of is one it
        // cannot author in.
        let namespaces = match NamespaceRepository::new(&store).participating_namespaces() {
            Ok(namespaces) => namespaces,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let signer_sk_bytes = match signing_identity(&store, &namespaces) {
            Ok(identity) => identity,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let signer_sk = PrivateKey::from(signer_sk_bytes);

        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let scope = cached.applications.clone();

        ActorResponse::r#async(
            async move {
                let outcomes = calimero_governance_store::bind_device_everywhere(
                    &store,
                    &node_client,
                    &ack_router,
                    &namespaces,
                    &signer_sk,
                    &cached,
                )
                .await;

                info!(%account, %device, ?outcomes, "relinked a device of this account");

                // The statement is the only durable record of the widening, so a
                // response carrying a scope nothing recorded would be a lie.
                if !crate::account_namespace::publish_device_certified(
                    &store,
                    &node_client,
                    &ack_router,
                    root.account_namespace(),
                    &signer_sk,
                    &root,
                    &cached.proof,
                    &scope,
                    "relink_device",
                )
                .await
                {
                    eyre::bail!(
                        "the widened scope for {device} was not recorded in the account namespace"
                    );
                }

                Ok(RelinkDeviceResponse::new(account, device, scope, outcomes))
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
    use calimero_governance_store::{
        AccountBindingRepository, GroupKeyring, MembershipRepository, MetaRepository,
        NamespaceRepository,
    };
    use calimero_store::db::InMemoryDB;

    use super::*;
    use crate::test_support::{actor, certify_device};

    const APP_ONE: [u8; 32] = [0x11; 32];
    const APP_TWO: [u8; 32] = [0x22; 32];
    const NS: [u8; 32] = [0xA1; 32];

    fn app(id: [u8; 32]) -> ApplicationId {
        ApplicationId::from(id)
    }

    /// A store where this node holds its own account and takes part in one
    /// namespace - the state any node is in after creating or joining one.
    fn a_node_holding_its_own_account() -> Store {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let namespaces = NamespaceRepository::new(&store);
        let _identity = namespaces
            .participate_in(&NS.into())
            .expect("take part in the namespace");
        let devices = NodeDeviceRepository::new(&store);
        let _root = devices
            .provision_account_root()
            .expect("a node that ran `merod init` holds a root");
        let _held = devices
            .ensure_enrolled(&NS.into())
            .expect("mint this node's own device");
        store
    }

    /// The right request at the wrong machine. A node that paired INTO somebody
    /// else's account holds no root that could have signed the certificate it
    /// would be re-publishing, and no retry here changes that.
    #[test]
    fn a_node_that_does_not_hold_the_account_is_refused() {
        let store = a_node_holding_its_own_account();
        let device = certify_device(&store, 0x31, &[]);

        // Adopt somebody else's account, which is what pairing this node into one
        // does to its single device slot.
        let devices = NodeDeviceRepository::new(&store);
        devices.delete().expect("release the slot");
        let _adopted = devices
            .ensure_enrolled_into(
                &[NS.into()],
                AccountGenesis::new(PrivateKey::from([0x41; 32]).public_key()),
            )
            .expect("adopt");

        let refused = resolve_target(&store, device, vec![]).expect_err("wrong machine");
        assert!(matches!(
            refused.downcast_ref::<ContextError>(),
            Some(ContextError::PairingNotTheAccountHolder { .. })
        ));
    }

    /// Authority before resource, and the order is the assertion. A caller at
    /// the wrong machine must be told so whether or not the device it named
    /// happens to be known here: answering `404` first would send an operator
    /// looking for a missing device when the machine is what is wrong.
    #[test]
    fn the_wrong_machine_is_refused_before_the_device_is_looked_up() {
        let store = a_node_holding_its_own_account();

        let devices = NodeDeviceRepository::new(&store);
        devices.delete().expect("release the slot");
        let _adopted = devices
            .ensure_enrolled_into(
                &[NS.into()],
                AccountGenesis::new(PrivateKey::from([0x42; 32]).public_key()),
            )
            .expect("adopt");

        let refused = resolve_target(&store, DeviceId::from([0x61; 32]), vec![])
            .expect_err("wrong machine, and a device nothing here knows");
        assert!(
            matches!(
                refused.downcast_ref::<ContextError>(),
                Some(ContextError::PairingNotTheAccountHolder { .. })
            ),
            "the unknown device must not answer first; got: {refused}"
        );
    }

    /// Only a device this node holds a certificate for can be extended: the
    /// certificate is what a link carries, and the replicated binding row drops
    /// the root signature, so there is nothing to rebuild it from.
    #[test]
    fn a_device_this_node_holds_no_certificate_for_is_refused() {
        let store = a_node_holding_its_own_account();

        let refused = resolve_target(&store, DeviceId::from([0x51; 32]), vec![])
            .expect_err("nothing is known about this device");
        assert!(matches!(
            refused.downcast_ref::<ContextError>(),
            Some(ContextError::PairingUnknownDevice { .. })
        ));
    }

    /// The registry is the only place a certificate lives now, and it is
    /// replicated - so a device this node never certified itself, learned from
    /// the account namespace, is one it can still relink.
    #[test]
    fn a_device_only_the_registry_knows_is_relinked() {
        let store = a_node_holding_its_own_account();
        let devices = NodeDeviceRepository::new(&store);
        let root = devices
            .account_root()
            .expect("read")
            .expect("this node holds its own account");
        let device = DeviceId::from([0x36; 32]);
        let proof = calimero_account::AccountProof {
            genesis: root.genesis(),
            chain: vec![],
            statement: calimero_account::DeviceCert::sign(
                root.signing_key(),
                root.account(),
                device,
                &PrivateKey::from([0x36; 32]).public_key(),
                &calimero_account::KemPublicKey::from([0xC9; 32]),
                0,
                0,
            )
            .expect("the account root signs its own device cert"),
        };
        let _recorded = AccountDeviceRegistry::new(&store, root.account_namespace())
            .record(&proof, &[app(APP_ONE)], 3)
            .expect("the certified op's apply wrote this row");

        let (_root, cert) = resolve_target(&store, device, vec![]).expect("the registry knows it");

        assert_eq!(cert.applications, vec![app(APP_ONE)]);
    }

    /// Refused outright rather than skipped per namespace. The tombstone is per
    /// namespace but the id is spent everywhere, so repairing around it would be
    /// repairing the wrong thing - and the refusal has to say that enrolling
    /// afresh mints a NEW id rather than suggest an un-revoke that cannot exist.
    #[test]
    fn a_revoked_device_is_refused_rather_than_repaired() {
        let store = a_node_holding_its_own_account();
        let device = certify_device(&store, 0x32, &[]);
        AccountBindingRepository::new(&store)
            .apply_revocation(&NS.into(), device)
            .expect("tombstone the device");

        let refused = resolve_target(&store, device, vec![]).expect_err("the id is spent");
        assert!(matches!(
            refused.downcast_ref::<ContextError>(),
            Some(ContextError::PairingDeviceRevoked { .. })
        ));
        assert!(
            refused.to_string().contains("mints a new device id"),
            "the refusal has to name the only way forward; got: {refused}"
        );
    }

    /// The widening is what the relink will publish. It is not written here:
    /// the statement's apply writes the registry row, which is what every later
    /// namespace gain on any device of the account is judged against.
    #[test]
    fn naming_applications_widens_the_scope_the_relink_will_publish() {
        let store = a_node_holding_its_own_account();
        let device = certify_device(&store, 0x33, &[app(APP_ONE)]);

        let (_root, widened) =
            resolve_target(&store, device, vec![app(APP_TWO)]).expect("extend the scope");

        assert_eq!(widened.applications, vec![app(APP_ONE), app(APP_TWO)]);
    }

    /// An empty scope already covers every application, so naming one must not
    /// replace "all" with "only that one" - a widening request that silently
    /// narrows is the worst shape this endpoint could have.
    #[test]
    fn naming_an_application_on_an_all_applications_device_does_not_narrow_it() {
        let store = a_node_holding_its_own_account();
        let device = certify_device(&store, 0x35, &[]);

        let (_root, cached) = resolve_target(&store, device, vec![app(APP_ONE)]).expect("repair");

        assert!(
            cached.applications.is_empty(),
            "an all-applications device stayed all-applications, got {:?}",
            cached.applications
        );
    }

    /// Naming an application the device already covers is a no-op rather than a
    /// duplicate - an operator repeating a widening should not grow the row.
    #[test]
    fn re_naming_an_application_the_device_already_covers_changes_nothing() {
        let store = a_node_holding_its_own_account();
        let device = certify_device(&store, 0x34, &[app(APP_ONE)]);

        let (_root, cached) = resolve_target(&store, device, vec![app(APP_ONE)]).expect("extend");

        assert_eq!(cached.applications, vec![app(APP_ONE)]);
    }

    /// The rest of what a member holds in a namespace it has joined: a
    /// membership its endorsement is admissible under, and the scope key the
    /// key delivery is wrapped from. `resolve_target` needs neither, which is
    /// why only the handler test seeds them.
    fn a_namespace_this_node_can_publish_in(store: &Store) {
        let ns = ContextGroupId::from(NS);
        let (_ns, node_pk, _sk) = NamespaceRepository::new(store)
            .participate_in(&ns)
            .expect("this node's identity here");
        let account = crate::test_support::enrol(store, &ns, &node_pk);
        MetaRepository::new(store)
            .save(
                &ns,
                &calimero_store::key::GroupMetaValue {
                    target: calimero_store::key::GroupTarget {
                        application_id: app(APP_ONE),
                        bytecode_id: [0xAA; 32],
                        ..Default::default()
                    },
                    created_at: 1_700_000_000,
                    admin_identity: account,
                    owner_identity: account,
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("save the namespace metadata");
        MembershipRepository::new(store)
            .add_member(
                &ns,
                &account,
                calimero_primitives::context::GroupMemberRole::Admin,
            )
            .expect("be a member here");
        let _key_id = GroupKeyring::new(store, ns)
            .store_key(&[0x42; 32])
            .expect("hold the scope key");
    }

    /// The handler shell, which `resolve_target` does not reach: the namespaces
    /// a repair runs against are the ones this node TAKES PART in, and the key
    /// it signs with is its identity there. Get either wrong and the mechanism
    /// below publishes into the wrong place, or nowhere.
    #[actix::test]
    async fn a_repair_binds_the_device_in_every_namespace_this_node_takes_part_in() {
        let store = a_node_holding_its_own_account();
        a_namespace_this_node_can_publish_in(&store);
        let harness = actor::over(store.clone()).await;
        // A holder has one, and the relink's closing statement has nowhere to
        // land without it.
        let namespace = harness
            .manager
            .send(EnsureAccountNamespaceRequest)
            .await
            .expect("the manager answers")
            .expect("the holder creates its account namespace")
            .expect("this node holds an account root");
        let device = certify_device(&store, 0x35, &[]);

        let repaired = harness
            .manager
            .send(RelinkDeviceRequest {
                device,
                applications: vec![],
            })
            .await
            .expect("the manager answers")
            .expect("the repair runs");

        let linked = calimero_context_client::group::BindOutcome::Linked {
            key_delivered: true,
        };
        // Order follows the key scan over a randomly minted account namespace id,
        // so the pair is asserted rather than the sequence.
        assert_eq!(repaired.outcomes.len(), 2, "got: {:?}", repaired.outcomes);
        assert!(repaired
            .outcomes
            .contains(&(ContextGroupId::from(NS), linked)));
        assert!(repaired.outcomes.contains(&(namespace, linked)));
        assert!(
            AccountBindingRepository::new(&store)
                .is_device_linked(&NS.into(), device)
                .expect("read the bindings"),
            "the link has to have APPLIED, not merely been reported"
        );
    }

    /// Widening a device's scope is a new statement at a higher epoch, so the
    /// registry every other device reads is what moves, not just the cache.
    #[actix::test]
    async fn a_relink_publishes_the_widened_scope_at_the_next_epoch() {
        let store = a_node_holding_its_own_account();
        let harness = actor::over(store.clone()).await;
        let namespace = harness
            .manager
            .send(EnsureAccountNamespaceRequest)
            .await
            .expect("the manager answers")
            .expect("the holder creates its account namespace")
            .expect("this node holds an account root");

        let device = certify_device(&store, 0x31, &[app(APP_ONE)]);

        let _first = harness
            .manager
            .send(RelinkDeviceRequest {
                device,
                applications: vec![app(APP_TWO)],
            })
            .await
            .expect("the manager answers")
            .expect("relinked");

        let (recorded, epoch) = AccountDeviceRegistry::new(&store, namespace)
            .device(device)
            .expect("read")
            .expect("the relink recorded the device");
        assert_eq!(recorded.applications, vec![app(APP_ONE), app(APP_TWO)]);
        assert_eq!(
            epoch, 1,
            "the certification itself is epoch 0; the relink is the statement after it"
        );

        let _second = harness
            .manager
            .send(RelinkDeviceRequest {
                device,
                applications: vec![],
            })
            .await
            .expect("the manager answers")
            .expect("repaired");
        let (_again, epoch) = AccountDeviceRegistry::new(&store, namespace)
            .device(device)
            .expect("read")
            .expect("row");
        assert_eq!(epoch, 2, "a second relink must supersede the first");
    }

    /// A widening the registry never took must not answer with the new scope.
    /// The statement is the only durable record of it, so reporting a widening
    /// every other device still judges the device against the old scope for is
    /// the one failure this endpoint must not have.
    #[actix::test]
    async fn a_widening_the_registry_did_not_record_is_an_error() {
        let store = a_node_holding_its_own_account();
        a_namespace_this_node_can_publish_in(&store);
        let device = certify_device(&store, 0x37, &[app(APP_ONE)]);

        // Nothing created the account namespace here, so this node is not an
        // admin of the one its root names and the certified op's apply refuses.
        let harness = actor::over(store.clone()).await;
        let refused = harness
            .manager
            .send(RelinkDeviceRequest {
                device,
                applications: vec![app(APP_TWO)],
            })
            .await
            .expect("the manager answers")
            .expect_err("the widened scope reached no registry");

        assert!(
            refused.to_string().contains(&device.to_string()),
            "the refusal has to name the device; got: {refused}"
        );
    }
}
