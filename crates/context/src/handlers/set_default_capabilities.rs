use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::SetDefaultCapabilitiesRequest;
use calimero_node_primitives::sync::GroupMutationKind;
use eyre::bail;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<SetDefaultCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetDefaultCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetDefaultCapabilitiesRequest {
            group_id,
            default_capabilities,
            requester,
        }: SetDefaultCapabilitiesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured group identity"
                    )))
                }
            },
        };

        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            if signing_key.is_none() {
                group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;
            }

            group_store::set_default_capabilities(
                &self.datastore,
                &group_id,
                default_capabilities,
            )?;

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        if let Some(ref sk) = signing_key {
            let _ =
                group_store::store_group_signing_key(&self.datastore, &group_id, &requester, sk);
        }

        let node_client = self.node_client.clone();
        let effective_signing_key = signing_key.or_else(|| {
            group_store::get_group_signing_key(&self.datastore, &group_id, &requester)
                .ok()
                .flatten()
        });
        let group_client_result = effective_signing_key.map(|sk| self.group_client(group_id, sk));

        ActorResponse::r#async(
            async move {
                if let Some(client_result) = group_client_result {
                    let mut group_client = client_result?;
                    group_client
                        .set_default_capabilities(default_capabilities)
                        .await?;
                }

                info!(
                    ?group_id,
                    default_capabilities, "default member capabilities updated"
                );

                let _ = node_client
                    .broadcast_group_mutation(
                        group_id.to_bytes(),
                        GroupMutationKind::DefaultCapabilitiesSet {
                            capabilities: default_capabilities,
                        },
                    )
                    .await;

                Ok(())
            }
            .into_actor(self),
        )
    }
}
