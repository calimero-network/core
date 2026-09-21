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

        // Refuse an unusable policy here rather than at the first admission.
        //
        // The gate in `admit_tee_node` requires both lists, so a policy missing
        // either is one that refuses every node -- and it would do so with an
        // error the operator reads days later, on a replica that will not join,
        // rather than now, on the request that created the problem.
        //
        // RTMR3 is required for the reason it exists: MRTD measures the virtual
        // firmware, so it is identical across every profile of a release and
        // constant across most releases. Only RTMR3 names a (profile, release)
        // pair, which also means it changes every release and the policy must
        // gain the new value on upgrade.
        if allowed_mrtd.is_empty() {
            return ActorResponse::reply(Err(eyre::eyre!(
                "allowed_mrtd must name at least one measurement"
            )));
        }
        if allowed_rtmr3.is_empty() {
            return ActorResponse::reply(Err(eyre::eyre!(
                "allowed_rtmr3 must name at least one measurement. MRTD alone does not identify \
                 the image -- it is the same for every profile of a release and does not change \
                 between most releases -- so a policy without RTMR3 would admit any profile, \
                 including debug images that are not locked down. Take the value for each \
                 profile you accept from that release's published-mrtds.json; it changes every \
                 release, so add the new one when upgrading."
            )));
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
