use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::SetTeeAuthoringPolicyRequest;
use calimero_context_client::local_governance::GroupOp;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::NamespaceRepository;
use tracing::info;

use crate::ContextManager;

impl Handler<SetTeeAuthoringPolicyRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetTeeAuthoringPolicyRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetTeeAuthoringPolicyRequest {
            group_id,
            allowed_mrtd,
        }: SetTeeAuthoringPolicyRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Namespace-scoped, like the admission policy: the apply handler refuses a
        // subgroup copy, so refuse it here too, where the caller can still read why.
        match NamespaceRepository::new(&self.datastore).parent(&group_id) {
            Ok(Some(_)) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "TEE authoring policy is namespace-scoped; set it on the namespace root \
                     instead of subgroup '{group_id:?}'"
                )));
            }
            Ok(None) => {}
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        let preflight = match self.governance_preflight(&group_id, true) {
            Ok(preflight) => preflight,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let sk = preflight.signer_sk();
        let datastore = preflight.datastore;
        let node_client = preflight.node_client;
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                // An empty list is accepted on purpose: it is how an admin turns
                // TEE authorship back off.
                let enabled = !allowed_mrtd.is_empty();
                let report = calimero_governance_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &sk,
                    GroupOp::TeeAuthoringPolicySet { allowed_mrtd },
                )
                .await?;
                report.observe("set_tee_authoring_policy", "TeeAuthoringPolicySet");

                info!(?group_id, enabled, "TEE authoring policy updated");

                Ok(())
            }
            .into_actor(self),
        )
    }
}
