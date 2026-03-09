use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::SetContextVisibilityRequest;
use calimero_node_primitives::sync::GroupMutationKind;
use eyre::bail;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<SetContextVisibilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetContextVisibilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetContextVisibilityRequest {
            group_id,
            context_id,
            mode,
            requester,
        }: SetContextVisibilityRequest,
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

            // Context visibility can be set by admin or the context creator
            let is_admin = group_store::is_group_admin(&self.datastore, &group_id, &requester)?;
            if !is_admin {
                // Check if requester is the context creator
                if let Some((_, creator_bytes)) =
                    group_store::get_context_visibility(&self.datastore, &group_id, &context_id)?
                {
                    if creator_bytes != *requester {
                        bail!("only admin or context creator can set visibility");
                    }
                } else {
                    bail!("context visibility not found for context in group");
                }
            }

            if signing_key.is_none() {
                group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;
            }

            let mode_u8 = match mode {
                calimero_context_config::VisibilityMode::Open => 0u8,
                calimero_context_config::VisibilityMode::Restricted => 1u8,
            };

            // Preserve creator from existing visibility, or use requester as creator
            let creator =
                group_store::get_context_visibility(&self.datastore, &group_id, &context_id)?
                    .map(|(_, c)| c)
                    .unwrap_or(*requester);

            group_store::set_context_visibility(
                &self.datastore,
                &group_id,
                &context_id,
                mode_u8,
                creator,
            )?;

            // Auto-add creator to allowlist when switching to Restricted
            if mode == calimero_context_config::VisibilityMode::Restricted {
                let creator_pk = calimero_primitives::identity::PublicKey::from(creator);
                if !group_store::check_context_allowlist(
                    &self.datastore,
                    &group_id,
                    &context_id,
                    &creator_pk,
                )? {
                    group_store::add_to_context_allowlist(
                        &self.datastore,
                        &group_id,
                        &context_id,
                        &creator_pk,
                    )?;
                }
            }

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        if let Some(ref sk) = signing_key {
            let _ =
                group_store::store_group_signing_key(&self.datastore, &group_id, &requester, sk);
        }

        let datastore = self.datastore.clone();
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
                        .set_context_visibility(context_id, mode)
                        .await?;
                }

                info!(?group_id, %context_id, ?mode, "context visibility updated");

                let contexts =
                    group_store::enumerate_group_contexts(&datastore, &group_id, 0, usize::MAX)?;
                let _ = node_client
                    .broadcast_group_mutation(
                        &contexts,
                        group_id.to_bytes(),
                        GroupMutationKind::VisibilityUpdated,
                    )
                    .await;

                Ok(())
            }
            .into_actor(self),
        )
    }
}
