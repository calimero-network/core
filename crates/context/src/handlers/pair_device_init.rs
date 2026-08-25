//! `PairDeviceInitRequest` handler — adopt an existing account on this node and
//! mint a device for it.
//!
//! The first half of pairing, and the half that runs on the *new* device. It
//! produces the three values the account holder needs in order to certify this
//! device — the `DeviceId`, the KEM public key, and the signing key — plus a
//! signature over them and the confirmation code for the two humans to compare.
//! It publishes no op.
//!
//! All three have to be minted here, which is what forces the exchange to be
//! two-way: the id is `H(account ‖ nonce)` so it needs the account, while the
//! certificate over it needs the root key, which lives on the other device.
//!
//! **One device across every namespace named, not one per namespace.** The
//! certificate covers the account rather than a scope, so minting per namespace
//! would hand the holder several ids to certify and several codes to read aloud
//! for one machine. The code falls out shared for the same reason: it is a hash
//! over material that is now minted once.
//!
//! **The caller has to name the namespaces.** This node is a member of nothing
//! and holds no scope key, so it can neither read the account's namespace set off
//! a DAG nor derive it; the holder is the only party that knows it. Taking a set
//! is what closes the asymmetry the fan-out left behind - the link is published
//! into every namespace the account speaks in, and a device listening on one of
//! them observes one of them.
//!
//! **This node is deliberately not a member here.** A paired device is one
//! device of an account that belongs to somebody else; membership stays with
//! the account, which is the whole point of separating the two ids. So this
//! handler uses the two membership-free primitives rather than the join flow:
//! `get_or_create_namespace_identity` provisions a signing identity for a
//! namespace this node may never have heard of (an unknown group has no parent
//! row, so it resolves as its own root), and `subscribe_namespace` opens the
//! gossip subscription. `join_namespace` would be wrong — it publishes
//! `MemberJoinedAt`, which is exactly what a paired device must not do.
//!
//! **No scope key is required, unlike enrolling a fresh account.** A pairing
//! device holds none; obtaining one is what the second half of the exchange is
//! for. Since nothing is published here there is no encrypted op to gate on.

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_account::PairingOffer;
use calimero_context_client::group::{PairDeviceInitRequest, PairDeviceInitResponse};
use calimero_governance_store::NodeDeviceRepository;
use calimero_primitives::identity::PrivateKey;
use tracing::info;

use crate::ContextManager;

impl Handler<PairDeviceInitRequest> for ContextManager {
    type Result = ActorResponse<Self, <PairDeviceInitRequest as Message>::Result>;

    fn handle(
        &mut self,
        PairDeviceInitRequest {
            namespaces,
            genesis,
        }: PairDeviceInitRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Provision this node's signing identity for each namespace. Not a
        // membership claim and not gated on one — it is the key this node will
        // sign its own ops with once the account holder has linked it.
        //
        // The key is node-level, so from the second namespace onward this only
        // records participation. That marker is not cosmetic: the sync layer and
        // the startup sweep walk it, so a namespace missing one is a namespace
        // this node never syncs.
        let mut identity = None;
        for namespace_id in &namespaces {
            match self.get_or_create_namespace_identity(namespace_id) {
                Ok((_, sign_pk, sign_sk)) => identity = Some((sign_pk, PrivateKey::from(sign_sk))),
                Err(err) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "failed to provision a namespace identity for {namespace_id:?}: {err}"
                    )))
                }
            }
        }
        // Nothing to enroll into, so nothing to subscribe to: the device would be
        // certified and then listen on no topic at all. Refusing beats reporting
        // a pairing that reaches nowhere.
        let Some((sign_pk, sign_sk)) = identity else {
            return ActorResponse::reply(Err(eyre::eyre!(
                "pairing needs at least one namespace to enroll into, and only the \
                 device that holds the account knows which ones it speaks in — so it \
                 has to name them"
            )));
        };

        let store = self.datastore.clone();

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
            match NodeDeviceRepository::new(&store).ensure_enrolled_into(&namespaces, genesis) {
                Ok(enrolled) => enrolled,
                Err(err) => return ActorResponse::reply(Err(err)),
            };

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
        let (offer, statement) = match PairingOffer::signed(
            &sign_sk,
            enrolled.account,
            enrolled.device(),
            enrolled.kem_public_key(),
        ) {
            Ok(signed) => signed,
            Err(err) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "failed to sign the pairing statement: {err}"
                )))
            }
        };

        let confirmation_code = offer.confirmation_code();

        let response = PairDeviceInitResponse::new(
            enrolled.account,
            enrolled.device(),
            enrolled.kem_public_key(),
            sign_pk,
            statement,
            confirmation_code,
        );

        let node_client = self.node_client.clone();
        ActorResponse::r#async(
            async move {
                // Subscribe now rather than after the link lands. The link is
                // authored by the *other* device, so this one has to already be
                // listening to observe it — and to receive the key delivery that
                // follows it.
                //
                // Subscribing to a namespace the holder's fan-out never reaches is
                // harmless, so this set does not have to agree with that one: an
                // extra topic delivers nothing, and a binding published somewhere
                // this device has not subscribed to is picked up whenever it does.
                for namespace_id in &namespaces {
                    node_client
                        .subscribe_namespace(namespace_id.to_bytes())
                        .await?;
                }

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
