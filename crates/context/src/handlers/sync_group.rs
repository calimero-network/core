use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{SyncGroupRequest, SyncGroupResponse};
use tracing::{info, warn};

use crate::{group_store, ContextManager};

impl Handler<SyncGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <SyncGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        SyncGroupRequest { group_id, .. }: SyncGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();

        ActorResponse::r#async(
            async move {
                let meta =
                    group_store::load_group_meta(&datastore, &group_id)?.ok_or_else(|| {
                        eyre::eyre!("group not found locally; wait for P2P replication")
                    })?;

                let contexts =
                    group_store::enumerate_group_contexts(&datastore, &group_id, 0, usize::MAX)?;
                for context_id in &contexts {
                    if let Err(err) = context_client.sync_context_config(*context_id, None).await {
                        warn!(
                            ?group_id,
                            %context_id,
                            ?err,
                            "failed to sync context while syncing group"
                        );
                    }
                }

                let member_count = group_store::count_group_members(&datastore, &group_id)? as u64;
                let context_count =
                    group_store::count_group_contexts(&datastore, &group_id)? as u64;

                info!(?group_id, "group state refreshed from local store");

                Ok(SyncGroupResponse {
                    group_id,
                    app_key: meta.app_key,
                    target_application_id: meta.target_application_id,
                    member_count,
                    context_count,
                })
            }
            .into_actor(self),
        )
    }
}
