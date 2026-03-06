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

                // Ensure target application binary is installed locally via P2P
                // blob sharing. Without this, lazy upgrades on peer nodes fail
                // because the new version's WASM binary isn't available locally.
                // The admin node announces the blob on the DHT during upgrade,
                // so get_blob_bytes should discover it via DHT and fetch via P2P.
                if let Some((blob_id, source, _size)) =
                    group_store::extract_application_blob_info(&group_info.target_application)
                {
                    if !node_client.has_blob(&blob_id)? {
                        let contexts = group_store::enumerate_group_contexts(
                            &datastore, &group_id, 0, 1,
                        )?;
                        if let Some(ctx_id) = contexts.first() {
                            match node_client
                                .get_blob_bytes(&blob_id, Some(ctx_id))
                                .await
                            {
                                Ok(Some(_bytes)) => {
                                    if let Ok(app_source) = source.parse() {
                                        if let Err(err) = node_client
                                            .install_application_from_bundle_blob(
                                                &blob_id,
                                                &app_source,
                                            )
                                            .await
                                        {
                                            warn!(
                                                %err,
                                                "failed to install target app bundle"
                                            );
                                        }
                                    }
                                }
                                Ok(None) => {
                                    warn!(
                                        "target app blob not available from any peer yet"
                                    );
                                }
                                Err(err) => {
                                    warn!(
                                        %err,
                                        "failed to fetch target app blob from peers"
                                    );
                                }
                            }
                        }
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
