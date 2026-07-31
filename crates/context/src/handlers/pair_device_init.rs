//! `PairDeviceInitRequest` handler — adopt an existing account on this node and
//! mint a device for it.
//!
//! The first half of pairing, and the half that runs on the *new* device. It
//! produces the three values the account holder needs in order to certify this
//! device — the `DeviceId`, the KEM public key, and the signing key — and
//! nothing else. It publishes no op.
//!
//! All three have to be minted here, which is what forces the exchange to be
//! two-way: the id is `H(account ‖ nonce)` so it needs the account, while the
//! certificate over it needs the root key, which lives on the other device.
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
use calimero_context_client::group::{PairDeviceInitRequest, PairDeviceInitResponse};
use calimero_governance_store::NodeDeviceRepository;
use tracing::info;

use crate::ContextManager;

impl Handler<PairDeviceInitRequest> for ContextManager {
    type Result = ActorResponse<Self, <PairDeviceInitRequest as Message>::Result>;

    fn handle(
        &mut self,
        PairDeviceInitRequest {
            namespace_id,
            genesis,
        }: PairDeviceInitRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Provision this node's signing identity for the namespace. Not a
        // membership claim and not gated on one — it is the key this node will
        // sign its own ops with once the account holder has linked it.
        let sign_pk = match self.get_or_create_namespace_identity(&namespace_id) {
            Ok((_, sign_pk, _, _)) => sign_pk,
            Err(err) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "failed to provision a namespace identity for {namespace_id:?}: {err}"
                )))
            }
        };

        let store = self.datastore.clone();

        // Mint this device under the account being adopted. Idempotent, so a
        // retried pairing hands back the same values rather than minting a
        // second replica id.
        //
        // A stored row that names a *different* account is decided by the
        // repository, not here: an unlinked one is replaced (it holds no replica
        // state, and the row is minted before anyone certifies it, so refusing
        // would let one mistyped nonce claim this namespace's only device slot
        // for good), a linked one is refused. `account create` reaches the same
        // rule through the same place, which is why it lives there.
        let enrolled =
            match NodeDeviceRepository::new(&store).ensure_enrolled_into(&namespace_id, genesis) {
                Ok(enrolled) => enrolled,
                Err(err) => return ActorResponse::reply(Err(err)),
            };

        let response = PairDeviceInitResponse::new(
            enrolled.account,
            enrolled.device(),
            enrolled.kem_public_key(),
            sign_pk,
        );

        let node_client = self.node_client.clone();
        ActorResponse::r#async(
            async move {
                // Subscribe now rather than after the link lands. The link is
                // authored by the *other* device, so this one has to already be
                // listening to observe it — and to receive the key delivery that
                // follows it.
                node_client
                    .subscribe_namespace(namespace_id.to_bytes())
                    .await?;

                info!(
                    namespace_id = ?namespace_id,
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
