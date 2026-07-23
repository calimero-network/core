use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{GroupContextEntry, ListGroupContextsRequest};
use calimero_governance_store::MetadataRepository;
use eyre::bail;

use crate::ContextManager;

impl Handler<ListGroupContextsRequest> for ContextManager {
    type Result = ActorResponse<Self, <ListGroupContextsRequest as Message>::Result>;

    fn handle(
        &mut self,
        ListGroupContextsRequest {
            group_id,
            offset,
            limit,
        }: ListGroupContextsRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_namespace_identity(&group_id) else {
                bail!("node has no group identity configured");
            };
            if !crate::scope_projection::ScopeProjections::member_now_checked(
                &self.datastore,
                &group_id,
                &node_identity,
            )? {
                // Typed so the admin API surfaces this precondition as a 403,
                // not a generic 500 (see `parse_api_error`).
                return Err(crate::error::ContextError::NotAGroupMember {
                    group_id: format!("{group_id:?}"),
                }
                .into());
            }
            MetadataRepository::new(&self.datastore)
                .enumerate_contexts_with_names(&group_id, offset, limit)
                .map(|entries| {
                    entries
                        .into_iter()
                        .map(|(context_id, name)| GroupContextEntry { context_id, name })
                        .collect()
                })
        })();

        ActorResponse::reply(result)
    }
}
