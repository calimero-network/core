//! `PairDeviceInitRequest` handler - adopt an existing account on this node and
//! mint a device for it.
//!
//! The first half of pairing, run on the *new* device. Mints the `DeviceId`, KEM
//! key and signing key the holder needs in order to certify it, and publishes no
//! op. One device across every namespace named, because the certificate covers
//! the account rather than a scope. The account namespace, when named, is one
//! more of them, and is recorded so the node can name it back.
//!
//! This node is deliberately not a member: membership stays with the account, so
//! each namespace named is followed through `follow_namespace::follow` - the same
//! helper the account-follow listener uses - rather than through `join_namespace`,
//! which would publish `MemberJoinedAt`.

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::PairingOffer;
use calimero_context_client::group::{PairDeviceInitRequest, PairDeviceInitResponse};
use calimero_governance_store::NodeDeviceRepository;
use calimero_primitives::identity::PrivateKey;
use tracing::info;

use crate::handlers::follow_namespace::follow;
use crate::ContextManager;

impl Handler<PairDeviceInitRequest> for ContextManager {
    type Result = ActorResponse<Self, <PairDeviceInitRequest as Message>::Result>;

    fn handle(
        &mut self,
        PairDeviceInitRequest {
            namespaces,
            genesis,
            account_namespace,
        }: PairDeviceInitRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let store = self.datastore.clone();
        let node_client = self.node_client.clone();

        ActorResponse::r#async(
            async move {
                let mut namespaces = namespaces;
                if let Some(account_namespace) = account_namespace {
                    NodeDeviceRepository::new(&store)
                        .store_account_namespace(&account_namespace)?;
                    if !namespaces.contains(&account_namespace) {
                        namespaces.push(account_namespace);
                    }
                }

                // Follow each namespace named: it records participation,
                // subscribes and pulls - all three needed before the link lands.
                let mut identity = None;
                for namespace_id in &namespaces {
                    let (sign_pk, sign_sk) = follow(&store, &node_client, namespace_id).await?;
                    _ = identity.get_or_insert((sign_pk, PrivateKey::from(sign_sk)));
                }
                let Some((sign_pk, sign_sk)) = identity else {
                    eyre::bail!("pair-init reached the handler with no namespace to enroll into")
                };

                // Mint this device under the account being adopted. Idempotent, so a
                // retried pairing hands back the same values rather than minting a
                // second replica id.
                //
                // A stored row that names a *different* account is decided by the
                // repository, not here: an unlinked one is replaced (it holds no replica
                // state, and the row is minted before anyone certifies it, so refusing
                // would let one mistyped nonce claim this node's only device slot for
                // good), a linked one is refused. The join path reaches the same rule
                // through the same place, which is why it lives there.
                let enrolled =
                    NodeDeviceRepository::new(&store).ensure_enrolled_into(&namespaces, genesis)?;

                // Sign what we minted. The three values below are otherwise bare
                // assertions by the time they reach the account holder — anyone able to
                // alter the payload in transit could put their own keys under this
                // device id, and the certificate would name them. Signing with the
                // device's own key proves the party offering the material generated it.
                //
                // The signature cannot rule out an attacker replacing both keys and
                // re-signing with its own; that is what the confirmation code below is
                // for, and why it is derived here rather than left to callers.
                // One offer, and both values derive from it. The statement and the code have
                // to describe the SAME four values or the two checks at the other end are
                // checking different things; building the offer once is what guarantees it.
                let (offer, statement) = PairingOffer::signed(
                    &sign_sk,
                    enrolled.account,
                    enrolled.device(),
                    enrolled.kem_public_key(),
                )
                .map_err(|err| eyre::eyre!("failed to sign the pairing statement: {err}"))?;

                let confirmation_code = offer.confirmation_code();

                let response = PairDeviceInitResponse::new(
                    enrolled.account,
                    enrolled.device(),
                    enrolled.kem_public_key(),
                    sign_pk,
                    statement,
                    confirmation_code,
                );

                info!(
                    namespaces = namespaces.len(),
                    account = %response.account,
                    device = %response.device,
                    "minted a device for an existing account; awaiting its certificate"
                );

                Ok(response)
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
    use calimero_governance_store::NamespaceRepository;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::*;
    use crate::test_support::actor;

    const ONE: [u8; 32] = [0xA1; 32];
    const TWO: [u8; 32] = [0xA2; 32];

    fn adopted_account() -> AccountGenesis {
        AccountGenesis::new(PrivateKey::from([0x51; 32]).public_key())
    }

    fn topic(namespace: [u8; 32]) -> String {
        format!("ns/{}", hex::encode(namespace))
    }

    /// Both loops run to the end of the set, and neither is cosmetic: a
    /// namespace with no participation row is one the sync layer never walks,
    /// and one with no subscription is one this device never hears the link on.
    #[actix::test]
    async fn every_named_namespace_is_provisioned_and_subscribed() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut harness = actor::over(store.clone()).await;

        let response = harness
            .manager
            .send(PairDeviceInitRequest {
                namespaces: vec![ONE.into(), TWO.into()],
                genesis: adopted_account(),
                account_namespace: None,
            })
            .await
            .expect("the manager answers")
            .expect("pair-init mints a device");

        let mut participating = NamespaceRepository::new(&store)
            .participating_namespaces()
            .expect("read the participation rows");
        participating.sort();
        assert_eq!(
            participating,
            vec![ContextGroupId::from(ONE), ContextGroupId::from(TWO)]
        );

        let mut subscribed = harness.subscribed();
        subscribed.sort();
        assert_eq!(subscribed, vec![topic(ONE), topic(TWO)]);

        // One device for the whole set, and it is the one this node now holds.
        let held = NodeDeviceRepository::new(&store)
            .get()
            .expect("read the device row")
            .expect("a device was minted");
        assert_eq!(held.account, adopted_account().account_id());
        assert_eq!(held.device(), response.device);
    }

    /// The invariant the request validator maintains, asserted where it is
    /// relied on: reaching the handler with no namespace refuses BEFORE minting,
    /// so a device is never certified to listen on no topic at all.
    #[actix::test]
    async fn an_empty_namespace_set_is_refused_before_a_device_is_minted() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let harness = actor::over(store.clone()).await;

        let refused = harness
            .manager
            .send(PairDeviceInitRequest {
                namespaces: vec![],
                genesis: adopted_account(),
                account_namespace: None,
            })
            .await
            .expect("the manager answers")
            .expect_err("nothing to enroll into");

        assert!(
            refused.to_string().contains("no namespace to enroll into"),
            "got: {refused}"
        );
        assert!(
            NodeDeviceRepository::new(&store)
                .get()
                .expect("read the device row")
                .is_none(),
            "a refused pairing must not have spent this node's device slot"
        );
    }

    /// One id is enough. The device records it and follows it exactly as it
    /// follows a namespace named in the list.
    #[actix::test]
    async fn the_account_namespace_is_recorded_and_followed() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut harness = actor::over(store.clone()).await;
        let account_namespace = ContextGroupId::from([0x4E; 32]);

        let response = harness
            .manager
            .send(PairDeviceInitRequest {
                namespaces: vec![],
                genesis: adopted_account(),
                account_namespace: Some(account_namespace),
            })
            .await
            .expect("the manager answers")
            .expect("pair-init mints a device");

        assert_eq!(
            NodeDeviceRepository::new(&store)
                .account_namespace()
                .expect("read"),
            Some(account_namespace)
        );
        assert_eq!(
            NamespaceRepository::new(&store)
                .participating_namespaces()
                .expect("read"),
            vec![account_namespace]
        );
        assert_eq!(harness.subscribed(), vec![topic([0x4E; 32])]);
        assert_eq!(response.account, adopted_account().account_id());
    }
}
