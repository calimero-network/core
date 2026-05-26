use crate::group_store::NamespaceRepository;
use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::GetNamespaceIdentityRequest;

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
            let ns_id = NamespaceRepository::new(&self.datastore).resolve(&group_id)?;
            match NamespaceRepository::new(&self.datastore).identity(&ns_id)? {
                Some((pk, _sk, _sender)) => Ok(Some((ns_id, pk))),
                None => Ok(None),
            }
        })();

        ActorResponse::reply(result)
    }
}
