use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::BroadcastGroupLocalStateRequest;
use calimero_node_primitives::sync::GroupMutationKind;

use crate::{group_store, ContextManager};

impl Handler<BroadcastGroupLocalStateRequest> for ContextManager {
    type Result = ActorResponse<Self, <BroadcastGroupLocalStateRequest as Message>::Result>;

    fn handle(
        &mut self,
        BroadcastGroupLocalStateRequest { group_id }: BroadcastGroupLocalStateRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let member_caps =
            match group_store::enumerate_member_capabilities(&self.datastore, &group_id) {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
        let default_caps = match group_store::get_default_capabilities(&self.datastore, &group_id) {
            Ok(v) => v,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let context_vis =
            match group_store::enumerate_context_visibilities(&self.datastore, &group_id) {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
        let default_vis = match group_store::get_default_visibility(&self.datastore, &group_id) {
            Ok(v) => v,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let allowlists =
            match group_store::enumerate_contexts_with_allowlists(&self.datastore, &group_id) {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
        let member_aliases =
            match group_store::enumerate_member_aliases(&self.datastore, &group_id) {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            };

        let node_client = self.node_client.clone();
        let group_id_bytes = group_id.to_bytes();

        ActorResponse::r#async(
            async move {
                for (member, capabilities) in member_caps {
                    let _ = node_client
                        .broadcast_group_mutation(
                            group_id_bytes,
                            GroupMutationKind::MemberCapabilitySet {
                                member: *member,
                                capabilities,
                            },
                        )
                        .await;
                }

                if let Some(capabilities) = default_caps {
                    let _ = node_client
                        .broadcast_group_mutation(
                            group_id_bytes,
                            GroupMutationKind::DefaultCapabilitiesSet { capabilities },
                        )
                        .await;
                }

                for (context_id, mode, creator) in context_vis {
                    let _ = node_client
                        .broadcast_group_mutation(
                            group_id_bytes,
                            GroupMutationKind::ContextVisibilitySet {
                                context_id: *context_id,
                                mode,
                                creator,
                            },
                        )
                        .await;
                }

                if let Some(mode) = default_vis {
                    let _ = node_client
                        .broadcast_group_mutation(
                            group_id_bytes,
                            GroupMutationKind::DefaultVisibilitySet { mode },
                        )
                        .await;
                }

                for (context_id, members) in allowlists {
                    let members_raw: Vec<[u8; 32]> = members.iter().map(|pk| **pk).collect();
                    let _ = node_client
                        .broadcast_group_mutation(
                            group_id_bytes,
                            GroupMutationKind::ContextAllowlistSet {
                                context_id: *context_id,
                                members: members_raw,
                            },
                        )
                        .await;
                }

                for (member, alias) in member_aliases {
                    let _ = node_client
                        .broadcast_group_mutation(
                            group_id_bytes,
                            GroupMutationKind::MemberAliasSet {
                                member: *member,
                                alias,
                            },
                        )
                        .await;
                }

                Ok(())
            }
            .into_actor(self),
        )
    }
}
