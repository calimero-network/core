use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::DetachContextFromGroupRequest;
use calimero_node_primitives::sync::GroupMutationKind;
use eyre::bail;
use tracing::warn;

use crate::group_store;
use crate::ContextManager;

impl Handler<DetachContextFromGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <DetachContextFromGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        DetachContextFromGroupRequest {
            group_id,
            context_id,
            requester,
        }: DetachContextFromGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

        // Resolve requester: use provided value or fall back to node group identity
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

        // Resolve signing_key from node identity key
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

            let current_group = group_store::get_group_for_context(&self.datastore, &context_id)?;
            if current_group.as_ref() != Some(&group_id) {
                bail!("context '{context_id}' does not belong to group '{group_id:?}'");
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        // Auto-store signing key for future use
        if let Some(ref sk) = signing_key {
            let _ =
                group_store::store_group_signing_key(&self.datastore, &group_id, &requester, sk);
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        // Capture contexts before detach — the detached context is still subscribed
        let contexts = match group_store::enumerate_group_contexts(
            &self.datastore,
            &group_id,
            0,
            usize::MAX,
        ) {
            Ok(c) => c,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
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
                        .unregister_context_from_group(context_id)
                        .await?;
                }

                group_store::unregister_context_from_group(&datastore, &group_id, &context_id)?;

                // Clean up orphaned visibility and allowlist data
                if let Err(err) =
                    group_store::delete_context_visibility(&datastore, &group_id, &context_id)
                {
                    warn!(
                        ?group_id, %context_id, %err,
                        "failed to clean up context visibility on detach"
                    );
                }
                if let Err(err) =
                    group_store::clear_context_allowlist(&datastore, &group_id, &context_id)
                {
                    warn!(
                        ?group_id, %context_id, %err,
                        "failed to clean up context allowlist on detach"
                    );
                }

                let _ = node_client
                    .broadcast_group_mutation(
                        &contexts,
                        group_id.to_bytes(),
                        GroupMutationKind::ContextDetached,
                    )
                    .await;

                Ok(())
            }
            .into_actor(self),
        )
    }
}
