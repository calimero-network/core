use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::AccountId;
use calimero_context_client::group::AddGroupMembersRequest;
use calimero_context_client::local_governance::{GroupOp, KeyEnvelope, NamespaceOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::X25519PublicKey;
use calimero_primitives::identity::{MemberIdentity, PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{info, warn};

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{AccountBindingRepository, GroupKeyring, NamespaceRepository};

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
                for (identity, role) in &members {
                    // Resolution runs at the NAMESPACE: a direct add targets a
                    // subgroup and names somebody who joined the namespace
                    // earlier, so asking the subgroup would refuse every
                    // legitimate add.
                    let member_account = match identity {
                        MemberIdentity::Account(account) => *account,
                        MemberIdentity::Key(key) => {
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

                    // Admin-initiated key delivery: proactively push the
                    // group key to the just-added member, ECDH-wrapped, so
                    // it can decrypt its `MemberAdded` and the group's ops
                    // promptly. This is a ONE-SHOT publish per add (not the
                    // removed receiver-side re-publish that caused #2319).
                    // The joiner-side pull (`recover_missing_group_keys`) is
                    // the durable fallback if this delivery is missed.
                    if let Some((_key_id, group_key)) =
                        GroupKeyring::new(&datastore, group_id).load_current_key()?
                    {
                        let ns_id = NamespaceRepository::new(&datastore).resolve(&group_id)?;
                        match key_deliveries(
                            &datastore, &ns_id, identity, member_account, &sk, &group_id,
                            &group_key,
                        ) {
                            Ok(deliveries) => {
                                if deliveries.is_empty() {
                                    warn!(%member_account, "added member has no live device; it must pull the group key itself");
                                }
                                for (envelope, recipient) in deliveries {
                                    let delivery_op = NamespaceOp::Root(RootOp::KeyDelivery {
                                        group_id: group_id.to_bytes().into(),
                                        envelope,
                                    });
                                    // Recipient-specific: pass
                                    // `required_signers = Some([recipient])` so
                                    // the report's `acked_by` cleanly reflects
                                    // whether the recipient applied and acked.
                                    if let Err(e) = calimero_governance_store::sign_and_publish_namespace_op(
                                        &datastore,
                                        &node_client,
                                        &ack_router,
                                        ns_id.to_bytes().into(),
                                        &sk,
                                        delivery_op,
                                        Some(vec![recipient]),
                                    )
                                    .await
                                    {
                                        warn!(?e, %recipient, "failed to publish KeyDelivery for added member");
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(?e, %identity, "failed to wrap group key for added member");
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

/// The wrapped group key per recipient, paired with the key whose ack confirms
/// the delivery landed.
///
/// An account is addressed through its live devices, the same device-first rule
/// the scope-key fan-out follows, so a revoked device cannot be handed the key.
fn key_deliveries(
    datastore: &Store,
    ns_id: &ContextGroupId,
    identity: &MemberIdentity,
    member_account: AccountId,
    sk: &PrivateKey,
    group_id: &ContextGroupId,
    group_key: &[u8; 32],
) -> EyreResult<Vec<(KeyEnvelope, PublicKey)>> {
    match identity {
        MemberIdentity::Key(key) => Ok(vec![(
            GroupKeyring::wrap_for_member(sk, key, &group_id.to_bytes(), group_key)?,
            *key,
        )]),
        MemberIdentity::Account(_) => AccountBindingRepository::new(datastore)
            .devices_of(ns_id, member_account)?
            .into_iter()
            .map(|binding| {
                Ok((
                    GroupKeyring::wrap_for_device(
                        sk,
                        binding.device,
                        &X25519PublicKey::from(binding.kem_pk),
                        &group_id.to_bytes(),
                        group_key,
                    )?,
                    binding.sign_pk,
                ))
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_client::local_governance::EnvelopeRecipient;
    use calimero_store::db::InMemoryDB;
    use rand::rngs::OsRng;

    use super::*;
    use crate::test_support::{account_for, enrol};

    const GROUP_KEY: [u8; 32] = [0x5A; 32];

    #[test]
    fn an_account_is_addressed_through_its_live_devices() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x01; 32]);
        let group = ContextGroupId::from([0x02; 32]);
        let mut rng = OsRng;

        let admin_sk = PrivateKey::random(&mut rng);
        let member = PrivateKey::random(&mut rng).public_key();
        let account = enrol(&store, &ns, &member);

        let deliveries = key_deliveries(
            &store,
            &ns,
            &MemberIdentity::Account(account),
            account,
            &admin_sk,
            &group,
            &GROUP_KEY,
        )
        .expect("wrap for the enrolled device");

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
        let mut rng = OsRng;

        let admin_sk = PrivateKey::random(&mut rng);
        // Never enrolled, so the namespace holds no binding.
        let account = account_for(&PrivateKey::random(&mut rng).public_key());

        let deliveries = key_deliveries(
            &store,
            &ns,
            &MemberIdentity::Account(account),
            account,
            &admin_sk,
            &group,
            &GROUP_KEY,
        )
        .expect("an empty device set is not an error");

        assert!(deliveries.is_empty());
    }

    #[test]
    fn a_key_is_addressed_directly() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x01; 32]);
        let group = ContextGroupId::from([0x02; 32]);
        let mut rng = OsRng;

        let admin_sk = PrivateKey::random(&mut rng);
        let member = PrivateKey::random(&mut rng).public_key();
        let account = enrol(&store, &ns, &member);

        let deliveries = key_deliveries(
            &store,
            &ns,
            &MemberIdentity::Key(member),
            account,
            &admin_sk,
            &group,
            &GROUP_KEY,
        )
        .expect("wrap for the named key");

        assert_eq!(deliveries.len(), 1);
        let (envelope, recipient) = &deliveries[0];
        assert_eq!(*recipient, member);
        assert!(matches!(
            envelope.recipient,
            EnvelopeRecipient::Member { identity, .. } if identity == member
        ));
    }
}
