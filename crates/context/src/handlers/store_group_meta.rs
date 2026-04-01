use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::StoreGroupMetaRequest;
use calimero_primitives::context::GroupMemberRole;
use calimero_store::key::GroupMetaValue;
use tracing::{info, warn};

use crate::{group_store, ContextManager};

impl Handler<StoreGroupMetaRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreGroupMetaRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreGroupMetaRequest {
            group_id,
            meta_payload,
        }: StoreGroupMetaRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Idempotent: skip if group metadata already exists locally
        if let Ok(Some(_)) = group_store::load_group_meta(&self.datastore, &group_id) {
            return ActorResponse::reply(Ok(()));
        }

        let meta: GroupMetaValue = match borsh::from_slice(&meta_payload) {
            Ok(m) => m,
            Err(err) => {
                warn!(?err, "failed to deserialize group meta payload from gossip");
                return ActorResponse::reply(Err(err.into()));
            }
        };

        let admin_identity = meta.admin_identity.into();

        if let Err(err) = group_store::save_group_meta(&self.datastore, &group_id, &meta) {
            return ActorResponse::reply(Err(err));
        }

        // Bootstrap the admin as a group member so permission checks work
        if let Err(err) = group_store::add_group_member(
            &self.datastore,
            &group_id,
            &admin_identity,
            GroupMemberRole::Admin,
        ) {
            return ActorResponse::reply(Err(err));
        }

        info!(?group_id, %admin_identity, "stored group metadata from gossip");

        ActorResponse::reply(Ok(()))
    }
}
