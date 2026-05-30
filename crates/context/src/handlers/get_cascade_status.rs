use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{CascadeStatusEntry, GetCascadeStatusRequest};
use calimero_governance_store::{MembershipRepository, NamespaceRepository, UpgradesRepository};
use eyre::bail;

use crate::ContextManager;

/// Walk the namespace subtree rooted at `namespace_id` and return one
/// [`CascadeStatusEntry`] per group that has an upgrade record.
///
/// This is a pure function so it can be exercised in unit/integration tests
/// without standing up an actor.
pub fn collect_cascade_status(
    store: &calimero_store::Store,
    namespace_id: &calimero_context_config::types::ContextGroupId,
) -> eyre::Result<Vec<CascadeStatusEntry>> {
    // `collect_descendants` EXCLUDES the starting group, so we prepend it.
    let mut groups = vec![*namespace_id];
    groups.extend(NamespaceRepository::new(store).collect_descendants(namespace_id)?);

    let repo = UpgradesRepository::new(store);
    let mut out = Vec::with_capacity(groups.len());

    for gid in groups {
        if let Some(v) = repo.load(&gid)? {
            let cascade_hlc = v.cascade_hlc;
            out.push(CascadeStatusEntry {
                group_id: gid,
                upgrade: v.into(),
                cascade_hlc,
            });
        }
    }

    Ok(out)
}

impl Handler<GetCascadeStatusRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetCascadeStatusRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetCascadeStatusRequest { namespace_id }: GetCascadeStatusRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_namespace_identity(&namespace_id) else {
                bail!("node has no group identity configured");
            };
            if !MembershipRepository::new(&self.datastore)
                .is_member(&namespace_id, &node_identity)?
            {
                bail!("node is not a member of namespace '{namespace_id:?}'");
            }
            collect_cascade_status(&self.datastore, &namespace_id)
        })();
        ActorResponse::reply(result)
    }
}
