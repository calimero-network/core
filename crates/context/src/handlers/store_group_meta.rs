use actix::{ActorResponse, Handler, Message};
use calimero_context_config::MemberCapabilities;
use calimero_context_client::group::StoreGroupMetaRequest;
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
        let existing_meta = group_store::load_group_meta(&self.datastore, &group_id);

        // Only skip if BOTH metadata and admin member exist (full success previously).
        // If metadata exists but admin is missing, fall through to repair.
        if let Ok(Some(ref meta)) = existing_meta {
            let admin_identity = meta.admin_identity.into();
            if group_store::check_group_membership(&self.datastore, &group_id, &admin_identity)
                .unwrap_or(false)
            {
                return ActorResponse::reply(Ok(()));
            }
        }

        // Reuse existing metadata if available, otherwise deserialize from payload.
        let meta: GroupMetaValue = match existing_meta {
            Ok(Some(m)) => m,
            _ => match borsh::from_slice(&meta_payload) {
                Ok(m) => m,
                Err(err) => {
                    warn!(?err, "failed to deserialize group meta payload from gossip");
                    return ActorResponse::reply(Err(err.into()));
                }
            },
        };

        let admin_identity = meta.admin_identity.into();

        // save_group_meta is idempotent — safe to call on retry
        if !matches!(
            group_store::load_group_meta(&self.datastore, &group_id),
            Ok(Some(_))
        ) {
            if let Err(err) = group_store::save_group_meta(&self.datastore, &group_id, &meta) {
                return ActorResponse::reply(Err(err));
            }
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

        // Set default capabilities so members added before the separate
        // DefaultCapabilitiesSet gossip arrives still get CAN_JOIN_OPEN_CONTEXTS.
        if group_store::get_default_capabilities(&self.datastore, &group_id)
            .ok()
            .flatten()
            .is_none()
        {
            let _ = group_store::set_default_capabilities(
                &self.datastore,
                &group_id,
                MemberCapabilities::CAN_JOIN_OPEN_CONTEXTS,
            );
        }

        info!(?group_id, %admin_identity, "stored group metadata from gossip");

        ActorResponse::reply(Ok(()))
    }
}
