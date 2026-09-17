use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::local_governance::NamespaceOp;
use calimero_context_client::messages::RelaySignedJoinRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{GovernanceSigner, GroupKeyring};
use calimero_primitives::identity::PrivateKey;

use crate::ContextManager;

impl Handler<RelaySignedJoinRequest> for ContextManager {
    type Result = ActorResponse<Self, <RelaySignedJoinRequest as Message>::Result>;

    fn handle(
        &mut self,
        RelaySignedJoinRequest { op }: RelaySignedJoinRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let namespace_id = op.namespace_id;
        let ns_group = ContextGroupId::from(namespace_id.to_bytes());

        // This node's namespace identity signs the envelope that carries the
        // join. It is not signing the join itself — that signature is the
        // joiner's, inside the seal, and every peer checks it after decrypting.
        let (_pk, node_sk) = match self.resolve_signer(&ns_group) {
            Ok(pair) => pair,
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                // The admitter holds the namespace key by definition — it is a
                // member — so a missing key here is a broken node rather than the
                // ordinary not-yet-delivered case, and it is refused rather than
                // downgraded to a cleartext publish. Falling back would put the
                // very metadata this path exists to hide back on the topic, and a
                // receiver could not tell that from a publisher that skipped
                // sealing on purpose.
                let Some((key_id, key)) =
                    GroupKeyring::new(&datastore, ns_group).load_current_key()?
                else {
                    eyre::bail!(
                        "refusing to relay a join in the clear: this node holds no key for \
                         namespace {}, so it cannot seal, and publishing the joiner's op \
                         unsealed would leak the membership this path exists to hide",
                        hex::encode(namespace_id.as_bytes())
                    );
                };

                let encrypted = GroupKeyring::encrypt_relayed_op(&key, &op)?;
                let sealed = NamespaceOp::RootRelaySealed {
                    key_id: key_id.into(),
                    encrypted,
                };

                let sk = PrivateKey::from(node_sk);
                let report = GovernanceSigner::new(&datastore, &node_client, &ack_router)
                    .publish_namespace_op(namespace_id, &sk, sealed)
                    .await?;
                report.observe("relay_signed_join", "RootRelaySealed");
                Ok(())
            }
            .into_actor(self),
        )
    }
}
