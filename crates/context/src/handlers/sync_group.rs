use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::{SyncGroupRequest, SyncGroupResponse};
use tracing::{info, warn};

use crate::{group_store, ContextManager};

impl Handler<SyncGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <SyncGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        SyncGroupRequest {
            group_id,
            requester: _,
            protocol,
            network_id,
            contract_id,
        }: SyncGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Resolve contract coordinates: prefer request params, fall back to config
        let (protocol, network_id, contract_id) = match (protocol, network_id, contract_id) {
            (Some(p), Some(n), Some(c)) => (p, n, c),
            _ => {
                let params = match self.external_config.params.get("near") {
                    Some(p) => p,
                    None => {
                        return ActorResponse::reply(Err(eyre::eyre!(
                            "no 'near' protocol config and no coordinates provided"
                        )));
                    }
                };
                (
                    "near".to_owned(),
                    params.network.clone(),
                    params.contract_id.clone(),
                )
            }
        };

        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();

        ActorResponse::r#async(
            async move {
                let (meta, group_info) = group_store::sync_group_state_from_contract(
                    &datastore,
                    &context_client,
                    &group_id,
                    &protocol,
                    &network_id,
                    &contract_id,
                )
                .await?;

                // Check if target app blob is available (informational only).
                // Blob fetching is handled by join_group_context when the node
                // actually joins a context, which establishes P2P mesh connectivity.
                if let Some((blob_id, _source, _size)) =
                    group_store::extract_application_blob_info(&group_info.target_application)
                {
                    if !node_client.has_blob(&blob_id)? {
                        info!(
                            ?group_id,
                            %blob_id,
                            "target app blob not available locally; \
                             it will be fetched when joining a context"
                        );
                    }
                }

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

                info!(?group_id, "group state synced from contract");

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
