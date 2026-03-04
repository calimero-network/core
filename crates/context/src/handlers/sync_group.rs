use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::{SyncGroupRequest, SyncGroupResponse};
use tracing::info;

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

        ActorResponse::r#async(
            async move {
                let _meta = group_store::sync_group_state_from_contract(
                    &datastore,
                    &context_client,
                    &group_id,
                    &protocol,
                    &network_id,
                    &contract_id,
                )
                .await?;

                let member_count = group_store::count_group_members(&datastore, &group_id)? as u64;
                let context_count =
                    group_store::count_group_contexts(&datastore, &group_id)? as u64;

                info!(?group_id, "group state synced from contract");

                Ok(SyncGroupResponse {
                    group_id,
                    app_key: _meta.app_key,
                    target_application_id: _meta.target_application_id,
                    member_count,
                    context_count,
                })
            }
            .into_actor(self),
        )
    }
}
