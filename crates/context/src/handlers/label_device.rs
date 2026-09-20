//! `LabelDeviceRequest` handler - give a device of this account a name, so every
//! device of the account renders the same one.
//!
//! What this node holds decides what it may name, so the refusals live in
//! `authority_for` rather than spread across the publish below.

use std::sync::Arc;
use std::time::{Duration, Instant};

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::{AccountId, AccountProof, DeviceId, DeviceLabel, SignedDeviceLabel};
use calimero_context_client::group::{LabelDeviceRequest, LabelDeviceResponse};
use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{AccountDeviceRegistry, NamespaceRepository, NodeDeviceRepository};
use calimero_governance_types::bounds::{device_label_is_valid, MAX_DEVICE_LABEL_BYTES};
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{debug, info};

use crate::error::ContextError;
use crate::handlers::relink_device::resolve_device;
use crate::ContextManager;

const DEVICE_RENAME_COOLDOWN: Duration = Duration::from_secs(10); // one device's renames wait this long on each other

/// What this node may publish for `device`: the account it speaks for, the
/// account namespace to publish into, and the root's statement when it holds one.
fn authority_for(
    store: &Store,
    device: DeviceId,
    label: &str,
    label_epoch: u32,
) -> EyreResult<(AccountId, ContextGroupId, Option<Box<SignedDeviceLabel>>)> {
    let devices = NodeDeviceRepository::new(store);
    if devices.holder_root()?.is_some() {
        // Shared with the relink and the rescope, so all three refuse an unknown
        // or spent device identically, and it yields the anchor reused below.
        let (root, cached) = resolve_device(store, device)?;
        let statement = DeviceLabel::sign(
            root.signing_key(),
            root.account(),
            device,
            label.to_owned(),
            label_epoch,
            0,
        )
        .map_err(|err| eyre::eyre!("failed to sign the name for {device}: {err}"))?;
        // Both anchors taken from the certificate, so the statement resolves the
        // root key at the epoch that certificate already resolves it at.
        return Ok((
            root.account(),
            root.account_namespace(),
            Some(Box::new(AccountProof {
                genesis: cached.proof.genesis,
                chain: cached.proof.chain.clone(),
                statement,
            })),
        ));
    }

    let Some(held) = devices.unrevoked_device()? else {
        eyre::bail!("this node holds no usable device, so it can name none");
    };
    if held.device() != device {
        return Err(ContextError::DeviceLabelNotOwn {
            device: device.to_string(),
            own: held.device().to_string(),
        }
        .into());
    }
    let Some(namespace) = devices.account_namespace()? else {
        eyre::bail!("this node follows no account namespace, so it has nowhere to publish a name");
    };
    Ok((held.account, namespace, None))
}

/// One past the epoch in force for `device`: minting at the stored epoch would
/// tie rather than supersede, and tell the caller a rename landed that did not.
fn next_label_epoch(store: &Store, device: DeviceId) -> EyreResult<u32> {
    let Some(namespace) = NodeDeviceRepository::new(store).account_namespace()? else {
        return Ok(0);
    };
    let Some(row) = AccountDeviceRegistry::new(store, namespace).label(device)? else {
        return Ok(0);
    };
    row.label_epoch
        .checked_add(1)
        .ok_or_else(|| eyre::eyre!("device {device} is at the last name epoch there is"))
}

impl Handler<LabelDeviceRequest> for ContextManager {
    type Result = ActorResponse<Self, <LabelDeviceRequest as Message>::Result>;

    fn handle(
        &mut self,
        LabelDeviceRequest { device, label }: LabelDeviceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        if !device_label_is_valid(&label) {
            return ActorResponse::reply(Err(ContextError::DeviceLabelInvalid {
                limit: MAX_DEVICE_LABEL_BYTES,
            }
            .into()));
        }
        // Node-local and checked before anything is signed: the apply never sees
        // it, so two nodes disagreeing about the wait cannot diverge a row.
        if let Some(published) = self.device_label_published.get(&device) {
            if published.elapsed() < DEVICE_RENAME_COOLDOWN {
                return ActorResponse::reply(Err(ContextError::DeviceRenamedTooRecently {
                    device: device.to_string(),
                    cooldown_secs: DEVICE_RENAME_COOLDOWN.as_secs(),
                }
                .into()));
            }
        }

        let store = self.datastore.clone();
        let label_epoch = match next_label_epoch(&store, device) {
            Ok(epoch) => epoch,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let (account, namespace, root_proof) =
            match authority_for(&store, device, &label, label_epoch) {
                Ok(authority) => authority,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
        let signer_sk = match NamespaceRepository::new(&store).identity(&namespace) {
            Ok(Some((_pk, sk))) => PrivateKey::from(sk),
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "this node has no identity in its own account namespace"
                )))
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let _replaced = self.device_label_published.insert(device, Instant::now());
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                calimero_governance_store::sign_apply_and_publish(
                    &store,
                    &node_client,
                    &ack_router,
                    &namespace,
                    &signer_sk,
                    GroupOp::AccountDeviceLabelled {
                        account,
                        device,
                        label: label.clone(),
                        label_epoch,
                        root_proof,
                    },
                )
                .await?
                .observe("label_device", "AccountDeviceLabelled");

                // An apply that refuses the name warns rather than failing, so the
                // row it writes is the only thing that says the rename landed.
                let stored = AccountDeviceRegistry::new(&store, namespace).label(device)?;
                if !stored.is_some_and(|row| row.label == label && row.label_epoch == label_epoch) {
                    eyre::bail!("the account namespace did not take the name for {device}");
                }

                debug!(%account, %device, label_epoch, "named a device");
                Ok(LabelDeviceResponse::new(
                    account,
                    device,
                    label,
                    label_epoch,
                ))
            }
            .into_actor(self),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::AccountGenesis;
    use calimero_governance_store::MembershipRepository;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupTarget};

    use super::*;
    use crate::handlers::ensure_account_namespace::ensure_account_namespace;
    use crate::test_support::{actor, certify_device};

    const NS: [u8; 32] = [0xA1; 32];

    /// A node holding its own account and taking part in one namespace, with the
    /// membership and key a publish into the account namespace needs.
    fn a_holder() -> Store {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let devices = NodeDeviceRepository::new(&store);
        let _root = devices
            .provision_account_root()
            .expect("a node that ran `merod init` holds a root");
        let namespace = ContextGroupId::from(NS);
        let (_ns, node_pk, _sk) = NamespaceRepository::new(&store)
            .participate_in(&namespace)
            .expect("this node's identity here");
        let _held = devices
            .ensure_enrolled(&namespace)
            .expect("mint this node's own device");
        let account = crate::test_support::enrol_holder(&store, &namespace, &node_pk);
        calimero_governance_store::MetaRepository::new(&store)
            .save(
                &namespace,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: calimero_primitives::application::ApplicationId::from(
                            [0x11; 32],
                        ),
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
        MembershipRepository::new(&store)
            .add_member(
                &namespace,
                &account,
                calimero_primitives::context::GroupMemberRole::Admin,
            )
            .expect("be an admin here");
        store
    }

    /// The name a settings UI would render for `device`.
    fn stored_label(store: &Store, device: DeviceId) -> Option<String> {
        let namespace = NodeDeviceRepository::new(store)
            .account_namespace()
            .expect("read")
            .expect("a holder names an account namespace");
        AccountDeviceRegistry::new(store, namespace)
            .label(device)
            .expect("read")
            .map(|row| row.label)
    }

    #[actix::test]
    async fn a_name_that_is_not_a_name_is_refused() {
        let store = a_holder();
        let harness = actor::over(store.clone()).await;
        let device = certify_device(&store, 0x31, &[]);

        for refused in ["", "  ", " untrimmed", "two\nlines", &"x".repeat(65)] {
            let err = harness
                .manager
                .send(LabelDeviceRequest {
                    device,
                    label: refused.to_owned(),
                })
                .await
                .expect("the manager answers")
                .expect_err("not a usable name");
            assert!(
                matches!(
                    err.downcast_ref::<ContextError>(),
                    Some(ContextError::DeviceLabelInvalid { .. })
                ),
                "{refused:?} got: {err}"
            );
        }
        assert_eq!(stored_label(&store, device), None);
    }

    #[actix::test]
    async fn the_holder_names_a_device_and_the_registry_carries_it() {
        let store = a_holder();
        let harness = actor::over(store.clone()).await;
        let _namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the holder creates its account namespace")
            .expect("this node holds an account root");
        let device = certify_device(&store, 0x31, &[]);

        let named = harness
            .manager
            .send(LabelDeviceRequest {
                device,
                label: "Work laptop".to_owned(),
            })
            .await
            .expect("the manager answers")
            .expect("the holder may name any device of its account");

        assert_eq!(named.label, "Work laptop");
        assert_eq!(named.label_epoch, 0);
        assert_eq!(stored_label(&store, device).as_deref(), Some("Work laptop"));
    }

    #[actix::test]
    async fn a_second_rename_inside_the_cooldown_is_refused() {
        let store = a_holder();
        let harness = actor::over(store.clone()).await;
        let _namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the holder creates its account namespace")
            .expect("this node holds an account root");
        let device = certify_device(&store, 0x31, &[]);

        let _first = harness
            .manager
            .send(LabelDeviceRequest {
                device,
                label: "First".to_owned(),
            })
            .await
            .expect("the manager answers")
            .expect("the first name lands");

        let err = harness
            .manager
            .send(LabelDeviceRequest {
                device,
                label: "Second".to_owned(),
            })
            .await
            .expect("the manager answers")
            .expect_err("inside the cooldown");

        assert!(
            matches!(
                err.downcast_ref::<ContextError>(),
                Some(ContextError::DeviceRenamedTooRecently { .. })
            ),
            "got: {err}"
        );
        assert_eq!(stored_label(&store, device).as_deref(), Some("First"));
    }

    #[actix::test]
    async fn a_paired_node_may_name_only_its_own_device() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let devices = NodeDeviceRepository::new(&store);
        let _own_root = devices
            .provision_account_root()
            .expect("a node that ran `merod init` holds a root");
        let held = devices
            .adopt_account(AccountGenesis::new(
                PrivateKey::from([0x53; 32]).public_key(),
            ))
            .expect("pair into another account");
        let harness = actor::over(store.clone()).await;

        let err = harness
            .manager
            .send(LabelDeviceRequest {
                device: DeviceId::from([0x31; 32]),
                label: "Not mine".to_owned(),
            })
            .await
            .expect("the manager answers")
            .expect_err("a paired device names only itself");

        assert!(
            matches!(
                err.downcast_ref::<ContextError>(),
                Some(ContextError::DeviceLabelNotOwn { .. })
            ),
            "got: {err}"
        );
        assert_ne!(held.device(), DeviceId::from([0x31; 32]));
    }
}
