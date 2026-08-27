//! `RelinkDeviceRequest` handler - repair or widen a device's bindings without
//! re-pairing it.
//!
//! Pairing is a snapshot: the holder binds the device wherever it takes part at
//! that moment. This re-runs the fan-out against the namespaces it takes part in
//! **now**, which is the whole of the repair - the certificate is root-signed
//! once and names no namespace, so a fresh endorsement and a key wrap are all a
//! namespace gained afterwards is missing. No handshake, no confirmation code,
//! and the device need not be online.
//!
//! Naming applications widens the stored scope first, and the widening is
//! persisted before anything is published: a scope that only held for this one
//! call would leave every namespace gained later back where it started.
//!
//! **A revoked device is refused outright, not skipped per namespace.** The
//! tombstone is per namespace, but the `DeviceId` is spent everywhere - enrolling
//! the machine again mints a fresh one - so a repair that quietly worked around a
//! revocation would be repairing the wrong thing.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::{AccountId, DeviceId};
use calimero_context_client::group::{RelinkDeviceRequest, RelinkDeviceResponse};
use calimero_governance_store::{KnownDeviceCert, NamespaceRepository, NodeDeviceRepository};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::info;

use crate::error::ContextError;
use crate::handlers::pair_device_complete::{require_this_node_holds, signing_identity};
use crate::ContextManager;

/// The certificate a relink will re-publish, and the scope it will use.
///
/// Every refusal lives here, in the order a caller can act on: wrong machine,
/// unknown device, spent id. The scope extension is persisted before returning,
/// because the scope is what every LATER namespace gain is judged against - one
/// that held only for this call would leave the next namespace back where it
/// started.
fn resolve_target(
    store: &Store,
    device: DeviceId,
    applications: Vec<ApplicationId>,
) -> EyreResult<(AccountId, KnownDeviceCert)> {
    let devices = NodeDeviceRepository::new(store);

    // The account root decides which account this node may extend a device into:
    // the genesis is the content address of that root, so a node that paired INTO
    // somebody else's account holds no root that could have signed the
    // certificate it would be re-publishing.
    let account = devices.require_account_root()?.account();
    require_this_node_holds(store, account)?;

    let Some(mut cached) = devices.device_cert(device)? else {
        return Err(ContextError::PairingUnknownDevice {
            device: device.to_string(),
        }
        .into());
    };

    let revoked = devices.revoked_in(device)?;
    if !revoked.is_empty() {
        return Err(ContextError::PairingDeviceRevoked {
            device: device.to_string(),
            namespaces: format!("{revoked:?}"),
        }
        .into());
    }

    if !applications.is_empty() {
        for application in applications {
            if !cached.applications.contains(&application) {
                cached.applications.push(application);
            }
        }
        devices.remember_device_cert(&cached.proof, &cached.applications)?;
    }

    Ok((account, cached))
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

        let (account, cached) = match resolve_target(&store, device, applications) {
            Ok(target) => target,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

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

    /// The widening has to outlive the call: the stored scope is what every later
    /// namespace gain is judged against, so one that held only for this request
    /// would leave the next namespace exactly where it started.
    #[test]
    fn naming_applications_extends_the_stored_scope_persistently() {
        let store = a_node_holding_its_own_account();
        let device = certify_device(&store, 0x33, &[app(APP_ONE)]);

        let (_account, widened) =
            resolve_target(&store, device, vec![app(APP_TWO)]).expect("extend the scope");
        assert_eq!(widened.applications, vec![app(APP_ONE), app(APP_TWO)]);

        assert_eq!(
            NodeDeviceRepository::new(&store)
                .device_cert(device)
                .expect("read")
                .expect("present")
                .applications,
            vec![app(APP_ONE), app(APP_TWO)],
            "the widened scope has to be on disk, not merely in this response"
        );

        // And a repair that names nothing must not narrow it back.
        let (_account, repaired) = resolve_target(&store, device, vec![]).expect("repair");
        assert_eq!(repaired.applications, vec![app(APP_ONE), app(APP_TWO)]);
    }

    /// Naming an application the device already covers is a no-op rather than a
    /// duplicate - an operator repeating a widening should not grow the row.
    #[test]
    fn re_naming_an_application_the_device_already_covers_changes_nothing() {
        let store = a_node_holding_its_own_account();
        let device = certify_device(&store, 0x34, &[app(APP_ONE)]);

        let (_account, cached) =
            resolve_target(&store, device, vec![app(APP_ONE)]).expect("extend");

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
                    bytecode_id: [0xAA; 32],
                    target_application_id: app(APP_ONE),
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
        let device = certify_device(&store, 0x35, &[]);

        let harness = actor::over(store.clone()).await;
        let repaired = harness
            .manager
            .send(RelinkDeviceRequest {
                device,
                applications: vec![],
            })
            .await
            .expect("the manager answers")
            .expect("the repair runs");

        assert_eq!(
            repaired.outcomes,
            vec![(
                ContextGroupId::from(NS),
                calimero_context_client::group::BindOutcome::Linked {
                    key_delivered: true
                }
            )]
        );
        assert!(
            AccountBindingRepository::new(&store)
                .is_device_linked(&NS.into(), device)
                .expect("read the bindings"),
            "the link has to have APPLIED, not merely been reported"
        );
    }
}
