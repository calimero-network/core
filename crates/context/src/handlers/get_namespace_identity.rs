use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::GetNamespaceIdentityRequest;

use crate::group_store;
use crate::ContextManager;

impl Handler<GetNamespaceIdentityRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetNamespaceIdentityRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetNamespaceIdentityRequest { group_id }: GetNamespaceIdentityRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let ns_id = group_store::resolve_namespace(&self.datastore, &group_id)?;
            match group_store::get_namespace_identity(&self.datastore, &ns_id)? {
                Some((pk, _sk, _sender)) => Ok(Some((ns_id, pk))),
                None => Ok(None),
            }
        })();

        ActorResponse::reply(result)
    }
}
