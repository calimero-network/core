use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{GetNamespaceIdentityRequest, NamespaceIdentity};
use calimero_governance_store::{account_for_group, NamespaceRepository};

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
                // The account is resolved rather than derived from `pk` here: an
                // enrolled node writes as its bound account, and only the store
                // knows the binding. Reached only once an identity exists, so the
                // `get_or_create_identity` inside cannot mint one as a side
                // effect of what is a read.
                Some((public_key, _sk)) => {
                    let account = account_for_group(&self.datastore, &group_id)?;
                    Ok(Some(NamespaceIdentity {
                        namespace_id: ns_id,
                        public_key,
                        account,
                    }))
                }
                None => Ok(None),
            }
        })();

        ActorResponse::reply(result)
    }
}
