use calimero_governance_store::NamespaceRepository;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::SetTeeAdmissionPolicyRequest;
use calimero_context_client::local_governance::GroupOp;
use tracing::info;

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::ObserveDelivery;

impl Handler<SetTeeAdmissionPolicyRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetTeeAdmissionPolicyRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetTeeAdmissionPolicyRequest {
            group_id,
            allowed_mrtd,
            allowed_rtmr0,
            allowed_rtmr1,
            allowed_rtmr2,
            allowed_rtmr3,
            allowed_tcb_statuses,
            accept_mock,
        }: SetTeeAdmissionPolicyRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // TEE admission policies are namespace-scoped. Reject attempts to set one
        // on a subgroup — callers must target the namespace root. The policy a
        // subgroup "inherits" is whatever is set on the root.
        match NamespaceRepository::new(&self.datastore).parent(&group_id) {
            Ok(Some(parent)) => {
                let root = match NamespaceRepository::new(&self.datastore).resolve(&group_id) {
                    Ok(root) => root,
                    Err(err) => return ActorResponse::reply(Err(err)),
                };
                return ActorResponse::reply(Err(eyre::eyre!(
                    "TEE admission policy is namespace-scoped; set it on the namespace root \
                     '{root:?}' instead of subgroup '{group_id:?}' (parent: '{parent:?}')"
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
                let report = calimero_governance_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &sk,
                    GroupOp::TeeAdmissionPolicySet {
                        allowed_mrtd: allowed_mrtd.clone(),
                        allowed_rtmr0: allowed_rtmr0.clone(),
                        allowed_rtmr1: allowed_rtmr1.clone(),
                        allowed_rtmr2: allowed_rtmr2.clone(),
                        allowed_rtmr3: allowed_rtmr3.clone(),
                        allowed_tcb_statuses: allowed_tcb_statuses.clone(),
                        accept_mock,
                    },
                )
                .await?;
                report.observe("set_tee_admission_policy", "TeeAdmissionPolicySet");

                info!(?group_id, accept_mock, "TEE admission policy updated");

                Ok(())
            }
            .into_actor(self),
        )
    }
}
