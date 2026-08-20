use std::collections::BTreeMap;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::AccountId;
use calimero_context_client::group::AddGroupMembersRequest;
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp};
use calimero_crypto::X25519PublicKey;
use calimero_primitives::identity::{MemberPrincipal, PublicKey};
use tracing::{info, warn};

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{
    AccountBindingRepository, DeviceBinding, GroupKeyring, KeyRecipient, NamespaceRepository,
};

/// One planned delivery of the group key to a just-added member.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Delivery {
    /// Where the key goes, and therefore which key it is sealed under.
    recipient: KeyRecipient,
    /// The identity whose ack proves this delivery arrived — passed as the
    /// publish's `required_signers` so the report speaks about the recipient
    /// rather than about the group.
    acked_by: PublicKey,
}

/// Who the group key goes to for one member of an add.
///
/// This is the whole behavioural difference between the two ways of naming a
/// member, so it is a function of its inputs and nothing else:
///
/// * A **key** names one device's worth of a person — the caller knows that key
///   and possibly nothing else about them — so it is addressed as-is, under the
///   Ed25519-identity agreement.
/// * An **account** names the person, and a person is reached through their
///   live devices. Every one of them gets an envelope sealed to the X25519 key
///   its certificate published.
///
/// An account with no live device yields **nothing**, and deliberately does not
/// fall back to addressing an identity. A member whose devices are all revoked
/// or superseded must receive nothing, or the key is handed straight back to the
/// revoked device — the same rule `current_key_recipients` enforces for the
/// rotation fan-out. The member row still lands; the key follows by pull once a
/// live device exists.
fn plan_deliveries(
    who: &MemberPrincipal,
    devices_by_account: &BTreeMap<AccountId, Vec<DeviceBinding>>,
) -> Vec<Delivery> {
    match *who {
        MemberPrincipal::Key(key) => vec![Delivery {
            recipient: KeyRecipient::Member(key),
            acked_by: key,
        }],
        MemberPrincipal::Account(account) => devices_by_account
            .get(&account)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|binding| Delivery {
                // The KEM key comes from the folded binding, never from the
                // wire: the row that supplies it is the row that says the
                // device is still authorized, so a revoked device's key is
                // simply not available to wrap with.
                recipient: KeyRecipient::Device {
                    device: binding.device,
                    kem_pk: X25519PublicKey::from(binding.kem_pk),
                },
                acked_by: binding.sign_pk,
            })
            .collect(),
    }
}

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
                // Both the device lookup below and the resolution a key needs
                // are NAMESPACE-scoped. Bindings are written where the
                // credential arrived, which is the namespace a member joined,
                // and a subgroup holds none of its own — so since every direct
                // add in practice targets a subgroup, asking the subgroup would
                // find nothing for anybody.
                let ns_id = NamespaceRepository::new(&datastore).resolve(&group_id)?;

                // Read once for the whole call, and only when something needs
                // it. Resolving one account at a time filters a fresh scan
                // each time, so an add of *m* members would read the binding
                // column *m* times to answer one question.
                let devices_by_account = if members.iter().any(|(who, _)| who.account().is_some()) {
                    AccountBindingRepository::new(&datastore).live_devices_by_account(&ns_id)?
                } else {
                    BTreeMap::new()
                };

                for (who, role) in &members {
                    // The op names an ACCOUNT, so a caller naming a key has to
                    // be resolved to one first.
                    let member_account = match *who {
                        // Already the principal the row is keyed by: nothing to
                        // resolve, and so nothing that can refuse. An account
                        // this node has not learned yet still names the same
                        // person everywhere, which is what a key cannot do.
                        MemberPrincipal::Account(account) => account,
                        // What an operator can be handed for somebody this node
                        // never listed. Resolution requires the key to be bound
                        // here already, so this form cannot in fact serve a
                        // subject with no account — see `MemberPrincipal` for
                        // why an account is the better thing to name.
                        MemberPrincipal::Key(ref key) => {
                            crate::member_account::require(&datastore, &group_id, key)?
                        }
                    };
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

                    // Admin-initiated key delivery: proactively push the group
                    // key to the just-added member, ECDH-wrapped, so it can
                    // decrypt its `MemberAdded` and the group's ops promptly.
                    // This is a ONE-SHOT publish per addressee (not the removed
                    // receiver-side re-publish that caused #2319). The
                    // joiner-side pull (`recover_missing_group_keys`) is the
                    // durable fallback if a delivery is missed.
                    if let Some((_key_id, group_key)) =
                        GroupKeyring::new(&datastore, group_id).load_current_key()?
                    {
                        let deliveries = plan_deliveries(who, &devices_by_account);
                        if deliveries.is_empty() {
                            warn!(
                                %member_account, ?group_id,
                                "added member holds no live device here, so the group \
                                 key was not delivered; it will be pulled once one is \
                                 enrolled"
                            );
                        }
                        for Delivery {
                            recipient,
                            acked_by,
                        } in deliveries
                        {
                            let envelope = match GroupKeyring::wrap_for_recipient(
                                &sk,
                                &recipient,
                                &group_id.to_bytes(),
                                &group_key,
                            ) {
                                Ok(envelope) => envelope,
                                Err(e) => {
                                    warn!(
                                        ?e, %acked_by,
                                        "failed to wrap group key for added member"
                                    );
                                    continue;
                                }
                            };
                            let delivery_op = NamespaceOp::Root(RootOp::KeyDelivery {
                                group_id: group_id.to_bytes().into(),
                                envelope,
                            });
                            // Recipient-specific: `required_signers` is the one
                            // identity that can ack this envelope, so the
                            // report's `acked_by` reflects whether the
                            // recipient applied it.
                            if let Err(e) =
                                calimero_governance_store::sign_and_publish_namespace_op(
                                    &datastore,
                                    &node_client,
                                    &ack_router,
                                    ns_id.to_bytes().into(),
                                    &sk,
                                    delivery_op,
                                    Some(vec![acked_by]),
                                )
                                .await
                            {
                                warn!(
                                    ?e, %acked_by,
                                    "failed to publish KeyDelivery for added member"
                                );
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

#[cfg(test)]
mod tests {
    use calimero_account::{DeviceId, KemPublicKey};
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    fn key(seed: u8) -> PublicKey {
        PrivateKey::from([seed; 32]).public_key()
    }

    /// A binding as the fold writes one: one device of one account, with the
    /// signing key its certificate published and the KEM key deliveries are
    /// sealed to.
    fn binding(account: AccountId, device: u8, sign: u8, kem: u8) -> DeviceBinding {
        DeviceBinding {
            device: DeviceId::from([device; 32]),
            account,
            sign_pk: key(sign),
            kem_pk: *KemPublicKey::from([kem; 32]).as_bytes(),
            device_epoch: 0,
        }
    }

    #[test]
    fn a_key_named_member_is_addressed_as_the_key_the_caller_gave() {
        // The caller knows a key and possibly nothing else about this person, so
        // there is nothing to expand it to.
        let who = MemberPrincipal::Key(key(7));

        let deliveries = plan_deliveries(&who, &BTreeMap::new());

        assert_eq!(
            deliveries,
            vec![Delivery {
                recipient: KeyRecipient::Member(key(7)),
                acked_by: key(7),
            }],
            "a key names exactly one addressee: itself"
        );
    }

    #[test]
    fn an_account_named_member_reaches_every_device_that_person_holds() {
        // The point of naming an account. A key would have reached whichever
        // single device the caller happened to know about, leaving this person's
        // other devices to discover the key by pull.
        let account = AccountId::from([0xA1; 32]);
        let devices = BTreeMap::from([(
            account,
            vec![
                binding(account, 0x01, 0x11, 0x21),
                binding(account, 0x02, 0x12, 0x22),
            ],
        )]);

        let deliveries = plan_deliveries(&MemberPrincipal::Account(account), &devices);

        assert_eq!(
            deliveries,
            vec![
                Delivery {
                    recipient: KeyRecipient::Device {
                        device: DeviceId::from([0x01; 32]),
                        kem_pk: X25519PublicKey::from([0x21; 32]),
                    },
                    acked_by: key(0x11),
                },
                Delivery {
                    recipient: KeyRecipient::Device {
                        device: DeviceId::from([0x02; 32]),
                        kem_pk: X25519PublicKey::from([0x22; 32]),
                    },
                    acked_by: key(0x12),
                },
            ],
            "each device gets its own envelope, sealed to its own certified key"
        );
    }

    #[test]
    fn only_the_named_persons_devices_are_addressed() {
        // The map is one scan of the whole namespace, so the filtering has to
        // happen here rather than being assumed of the caller.
        let account = AccountId::from([0xA1; 32]);
        let other = AccountId::from([0xB2; 32]);
        let devices = BTreeMap::from([
            (account, vec![binding(account, 0x01, 0x11, 0x21)]),
            (other, vec![binding(other, 0x03, 0x13, 0x23)]),
        ]);

        let deliveries = plan_deliveries(&MemberPrincipal::Account(account), &devices);

        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].acked_by, key(0x11));
    }

    #[test]
    fn an_account_with_no_live_device_is_delivered_nothing() {
        // NOT a fallback to identity addressing. Every device of this person is
        // revoked or superseded, and identity-addressing the delivery would hand
        // the group key straight back to the revoked device — the exclusion the
        // device rows exist to enforce. The member row still lands, and the key
        // follows by pull once a live device exists.
        let account = AccountId::from([0xA1; 32]);

        let empty_entry = BTreeMap::from([(account, Vec::new())]);
        let absent = BTreeMap::new();

        for devices in [empty_entry, absent] {
            let deliveries = plan_deliveries(&MemberPrincipal::Account(account), &devices);

            assert!(
                deliveries.is_empty(),
                "no live device must mean no delivery, never an identity-addressed one"
            );
        }
    }
}
