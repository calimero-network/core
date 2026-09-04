use std::collections::BTreeMap;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::AccountId;
use calimero_context_client::group::AddGroupMembersRequest;
use calimero_context_client::local_governance::{GroupOp, KeyEnvelope, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::X25519PublicKey;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use tracing::{info, warn};

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{
    AccountBindingRepository, DeviceBinding, GroupKeyring, NamespaceRepository,
};

impl Handler<AddGroupMembersRequest> for ContextManager {
    type Result = ActorResponse<Self, <AddGroupMembersRequest as Message>::Result>;

    fn handle(
        &mut self,
        AddGroupMembersRequest { group_id, members }: AddGroupMembersRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let preflight = match self.governance_preflight(&group_id, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let datastore = preflight.datastore.clone();
        let node_client = preflight.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let sk = preflight.signer_sk();
        let signer = preflight.signer;
        let members = members.clone();

        ActorResponse::r#async(
            async move {
                let ns_id = NamespaceRepository::new(&datastore).resolve(&group_id)?;
                // One scan for the whole batch: `devices_of` per member rescans
                // the binding column once per member, and the batch is unbounded.
                let devices =
                    AccountBindingRepository::new(&datastore).live_devices_by_account(&ns_id)?;

                for (identity, role) in &members {
                    // The wire cannot say whether these 32 bytes are an account
                    // or a signing key, so the bindings decide. Resolution runs at
                    // the NAMESPACE, because a direct add targets a subgroup and
                    // names somebody who joined the namespace earlier. An
                    // unresolvable identity is taken as an account as given -
                    // deliberately, since the point of naming one is to name
                    // somebody this node may not have converged on yet.
                    let (member_account, member_key) =
                        crate::member_account::resolve(&datastore, &group_id, identity)?;
                    let report = calimero_governance_store::sign_apply_and_publish(
                        &datastore,
                        &node_client,
                        &ack_router,
                        &group_id,
                        &sk,
                        GroupOp::MemberAdded {
                            member: member_account,
                            role: role.clone(),
                        },
                    )
                    .await?;
                    report.observe("add_group_members", "MemberAdded");

                    // Admin-initiated key delivery: record the group key for
                    // the just-added member, ECDH-wrapped. This is a ONE-SHOT
                    // publish per add (not the removed receiver-side re-publish
                    // that caused #2319).
                    //
                    // Sealed under the NAMESPACE key, which splits the two adds
                    // this path serves. Adding an existing namespace member to a
                    // SUBGROUP still delivers end to end: that member holds the
                    // namespace key, so it opens the seal and unwraps the
                    // subgroup key inside. Adding a member to the namespace ROOT
                    // does not: the recipient holds nothing yet and cannot open
                    // the seal, so for that add the transfer is the joiner-side
                    // pull (`recover_missing_group_keys`) and this op is only the
                    // members-only causal record of the delivery.
                    if let Some((_key_id, group_key)) =
                        GroupKeyring::new(&datastore, group_id).load_current_key()?
                    {
                        let deliveries = key_deliveries(
                            member_key,
                            member_account,
                            &devices,
                            &sk,
                            &group_id,
                            &group_key,
                        );
                        if deliveries.is_empty() {
                            warn!(%member_account, "no group key was delivered to the added member; it must pull the key itself");
                        }
                        for (envelope, recipient) in deliveries {
                            let delivery_op =
                                match calimero_governance_store::seal_root_op_for_publish(
                                    &datastore,
                                    ns_id.to_bytes().into(),
                                    RootOp::KeyDelivery {
                                        group_id: group_id.to_bytes().into(),
                                        envelope,
                                    },
                                ) {
                                    Ok(op) => op,
                                    Err(e) => {
                                        warn!(?e, %recipient, "could not seal KeyDelivery for added member; it must pull the key itself");
                                        continue;
                                    }
                                };
                            // `required_signers` is None. It used to name the
                            // recipient, but a recipient that holds no namespace
                            // key cannot apply a sealed op, so on a root add the
                            // ack could never arrive and every successful add
                            // logged a failure. The pull is the confirmation
                            // path for that case.
                            if let Err(e) = calimero_governance_store::sign_and_publish_namespace_op(
                                &datastore,
                                &node_client,
                                &ack_router,
                                ns_id.to_bytes().into(),
                                &sk,
                                delivery_op,
                                None,
                            )
                            .await
                            {
                                warn!(?e, %recipient, "failed to publish KeyDelivery for added member");
                            }
                        }
                    }
                }
                info!(
                    ?group_id,
                    count = members.len(),
                    %signer,
                    "members added to group (local governance signed ops)"
                );
                Ok(())
            }
            .into_actor(self),
        )
    }
}

/// The wrapped group key per recipient, paired with the key that identifies the
/// recipient in the log line for that delivery.
///
/// An account is addressed through its live devices, the same device-first rule
/// the scope-key fan-out follows, so a revoked device cannot be handed the key.
///
/// A recipient whose wrap fails is dropped with a warning rather than failing
/// the batch: one stale `kem_pk` must not cost a member every other device it
/// has, since the joiner-side pull is a far slower way to get the key.
fn key_deliveries(
    member_key: Option<PublicKey>,
    member_account: AccountId,
    devices: &BTreeMap<AccountId, Vec<DeviceBinding>>,
    sk: &PrivateKey,
    group_id: &ContextGroupId,
    group_key: &[u8; 32],
) -> Vec<(KeyEnvelope, PublicKey)> {
    match member_key {
        Some(key) => {
            match GroupKeyring::wrap_for_member(sk, &key, &group_id.to_bytes(), group_key) {
                Ok(envelope) => vec![(envelope, key)],
                Err(e) => {
                    warn!(?e, %key, "failed to wrap the group key for the added member");
                    vec![]
                }
            }
        }
        None => devices
            .get(&member_account)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|binding| {
                match GroupKeyring::wrap_for_device(
                    sk,
                    binding.device,
                    &X25519PublicKey::from(binding.kem_pk),
                    &group_id.to_bytes(),
                    group_key,
                ) {
                    Ok(envelope) => Some((envelope, binding.sign_pk)),
                    Err(e) => {
                        warn!(?e, device = %hex::encode(binding.device.as_bytes()),
                              "failed to wrap the group key for a device of the added member");
                        None
                    }
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_client::local_governance::EnvelopeRecipient;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use rand::rand_core::UnwrapErr;
    use rand::rngs::SysRng;

    use super::*;
    use crate::test_support::{account_for, enrol};

    const GROUP_KEY: [u8; 32] = [0x5A; 32];

    /// The device map the handler builds once per batch.
    fn devices_in(store: &Store, ns: &ContextGroupId) -> BTreeMap<AccountId, Vec<DeviceBinding>> {
        AccountBindingRepository::new(store)
            .live_devices_by_account(ns)
            .expect("scan the binding column")
    }

    #[test]
    fn an_account_is_addressed_through_its_live_devices() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x01; 32]);
        let group = ContextGroupId::from([0x02; 32]);
        let mut rng = UnwrapErr(SysRng);

        let admin_sk = PrivateKey::random(&mut rng);
        let member = PrivateKey::random(&mut rng).public_key();
        let account = enrol(&store, &ns, &member);

        let deliveries = key_deliveries(
            None,
            account,
            &devices_in(&store, &ns),
            &admin_sk,
            &group,
            &GROUP_KEY,
        );

        assert_eq!(deliveries.len(), 1);
        let (envelope, recipient) = &deliveries[0];
        assert_eq!(
            *recipient, member,
            "the ack is attributed to the device key"
        );
        assert!(matches!(
            envelope.recipient,
            EnvelopeRecipient::Device { .. }
        ));
    }

    #[test]
    fn an_account_with_no_device_gets_nothing() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x01; 32]);
        let group = ContextGroupId::from([0x02; 32]);
        let mut rng = UnwrapErr(SysRng);

        let admin_sk = PrivateKey::random(&mut rng);
        // Never enrolled, so the namespace holds no binding.
        let account = account_for(&PrivateKey::random(&mut rng).public_key());

        let deliveries = key_deliveries(
            None,
            account,
            &devices_in(&store, &ns),
            &admin_sk,
            &group,
            &GROUP_KEY,
        );

        assert!(deliveries.is_empty());
    }

    #[test]
    fn a_recipient_that_cannot_be_wrapped_for_is_dropped_not_propagated() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x01; 32]);
        let group = ContextGroupId::from([0x02; 32]);
        let mut rng = UnwrapErr(SysRng);

        let admin_sk = PrivateKey::random(&mut rng);
        // A small-order point: the identity agreement refuses it, because a
        // shared secret derived from one does not depend on our scalar.
        let unusable = PublicKey::from([0u8; 32]);

        let deliveries = key_deliveries(
            Some(unusable),
            account_for(&unusable),
            &devices_in(&store, &ns),
            &admin_sk,
            &group,
            &GROUP_KEY,
        );

        assert!(deliveries.is_empty());
    }

    #[test]
    fn a_key_is_addressed_directly() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x01; 32]);
        let group = ContextGroupId::from([0x02; 32]);
        let mut rng = UnwrapErr(SysRng);

        let admin_sk = PrivateKey::random(&mut rng);
        let member = PrivateKey::random(&mut rng).public_key();
        let account = enrol(&store, &ns, &member);

        let deliveries = key_deliveries(
            Some(member),
            account,
            &devices_in(&store, &ns),
            &admin_sk,
            &group,
            &GROUP_KEY,
        );

        assert_eq!(deliveries.len(), 1);
        let (envelope, recipient) = &deliveries[0];
        assert_eq!(*recipient, member);
        assert!(matches!(
            envelope.recipient,
            EnvelopeRecipient::Member { identity, .. } if identity == member
        ));
    }
}
