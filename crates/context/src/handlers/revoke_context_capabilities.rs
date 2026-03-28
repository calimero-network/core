use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::RevokeContextCapabilitiesRequest;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_primitives::identity::PrivateKey;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<RevokeContextCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <RevokeContextCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        RevokeContextCapabilitiesRequest {
            context_id,
            capabilities,
            signer_id,
        }: RevokeContextCapabilitiesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();
        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        let group_id = match group_store::get_group_for_context(&self.datastore, &context_id) {
            Ok(Some(gid)) => gid,
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "context '{}' is not registered in any group",
                    context_id
                )));
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        if let Err(err) = (|| -> eyre::Result<()> {
            if signing_key.is_none() {
                group_store::require_group_signing_key(
                    &self.datastore,
                    &group_id,
                    &signer_id,
                )?;
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        if let Some(ref sk) = signing_key {
            let _ =
                group_store::store_group_signing_key(&self.datastore, &group_id, &signer_id, sk);
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let effective_signing_key = signing_key.or_else(|| {
            group_store::get_group_signing_key(&self.datastore, &group_id, &signer_id)
                .ok()
                .flatten()
        });

        ActorResponse::r#async(
            async move {
                let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                    eyre::eyre!("local group governance requires a signing key for the requester")
                })?);

                for (member, capability) in &capabilities {
                    group_store::sign_apply_and_publish(
                        &datastore,
                        &node_client,
                        &group_id,
                        &sk,
                        GroupOp::ContextCapabilityRevoked {
                            context_id,
                            member: *member,
                            capability: *capability,
                        },
                    )
                    .await?;

                    info!(
                        ?group_id,
                        %context_id,
                        %member,
                        capability,
                        "context capability revoked"
                    );
                }

                Ok(())
            }
            .into_actor(self),
        )
    }
}
