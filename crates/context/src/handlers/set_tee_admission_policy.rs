use calimero_governance_store::NamespaceRepository;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{SetTeeAdmissionPolicyRequest, SignedReleaseTrust};
use calimero_context_client::local_governance::GroupOp;
use tracing::info;

use crate::ContextManager;
use calimero_governance_store;

/// Refuse the policy as [`crate::error::ContextError::TeePolicyInvalid`], a `400`
/// naming what to fix, where a bare `bail!` would answer the caller's own input
/// as a `500`.
macro_rules! refuse {
    ($($arg:tt)+) => {
        eyre::bail!(crate::error::ContextError::TeePolicyInvalid {
            reason: format!($($arg)+),
        })
    };
}
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
            signed_release,
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
                return ActorResponse::reply(Err(crate::error::ContextError::TeePolicyInvalid {
                    reason: format!(
                        "TEE admission policy is namespace-scoped; set it on the namespace root \
                         '{root:?}' instead of subgroup '{group_id:?}' (parent: '{parent:?}')"
                    ),
                }
                .into()));
            }
            Ok(None) => {}
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        let op = match signed_release {
            Some(trust) => {
                let lists = [
                    &allowed_mrtd,
                    &allowed_rtmr0,
                    &allowed_rtmr1,
                    &allowed_rtmr2,
                    &allowed_rtmr3,
                ];
                match signed_release_op(trust, lists, allowed_tcb_statuses, accept_mock) {
                    Ok(op) => op,
                    Err(err) => return ActorResponse::reply(Err(err)),
                }
            }
            None => {
                match list_policy_op(
                    allowed_mrtd,
                    allowed_rtmr0,
                    allowed_rtmr1,
                    allowed_rtmr2,
                    allowed_rtmr3,
                    allowed_tcb_statuses,
                    accept_mock,
                ) {
                    Ok(op) => op,
                    Err(err) => return ActorResponse::reply(Err(err)),
                }
            }
        };
        // The metric label each form has always been observed under.
        let op_kind = match op {
            GroupOp::TeeReleaseAdmissionPolicySet { .. } => "TeeReleaseAdmissionPolicySet",
            _ => "TeeAdmissionPolicySet",
        };

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
                    op,
                )
                .await?;
                report.observe("set_tee_admission_policy", op_kind);

                info!(
                    ?group_id,
                    accept_mock, op_kind, "TEE admission policy updated"
                );

                Ok(())
            }
            .into_actor(self),
        )
    }
}

/// The signed-release form. The profiles are required and the measurement
/// lists refused: a policy carrying both would store lists nothing checks.
fn signed_release_op(
    trust: SignedReleaseTrust,
    measurement_lists: [&Vec<String>; 5],
    allowed_tcb_statuses: Vec<String>,
    accept_mock: bool,
) -> eyre::Result<GroupOp> {
    if measurement_lists.iter().any(|list| !list.is_empty()) {
        refuse!(
            "a signed-release policy takes its measurements from the release the TEE runs; \
             leave the measurement lists empty"
        );
    }
    if trust.allowed_profiles.iter().all(|p| p.trim().is_empty()) {
        refuse!("a signed-release policy must name at least one image profile");
    }
    let min_release_version = trust
        .min_release_version
        .map(|v| {
            calimero_tee_release::normalize_release_version(
                &v,
                calimero_tee_release::NODE_RELEASE_TAG_PREFIX,
            )
        })
        .transpose()?;
    Ok(GroupOp::TeeReleaseAdmissionPolicySet {
        allowed_profiles: trust.allowed_profiles,
        min_release_version,
        allowed_tcb_statuses,
        accept_mock,
    })
}

/// The measurement-list form.
fn list_policy_op(
    allowed_mrtd: Vec<String>,
    allowed_rtmr0: Vec<String>,
    allowed_rtmr1: Vec<String>,
    allowed_rtmr2: Vec<String>,
    allowed_rtmr3: Vec<String>,
    allowed_tcb_statuses: Vec<String>,
    accept_mock: bool,
) -> eyre::Result<GroupOp> {
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
        refuse!("allowed_mrtd must name at least one measurement");
    }
    if allowed_rtmr3.is_empty() {
        refuse!(
            "allowed_rtmr3 must name at least one measurement. MRTD alone does not identify \
             the image -- it is the same for every profile of a release and does not change \
             between most releases -- so a policy without RTMR3 would admit any profile, \
             including debug images that are not locked down. Take the value for each \
             profile you accept from that release's published-mrtds.json; it changes every \
             release, so add the new one when upgrading."
        );
    }
    // RTMR1 and RTMR2 are required for RTMR3's sake. `calimero-init` extends
    // RTMR3 from public inputs, so it only names the image when the kernel
    // (RTMR1) and the command line + initrd (RTMR2) that ran before it are
    // pinned too; otherwise a custom kernel or initrd can reproduce a
    // locked profile's RTMR3. RTMR0 varies with machine shape and stays
    // optional.
    if allowed_rtmr1.is_empty() || allowed_rtmr2.is_empty() {
        refuse!(
            "allowed_rtmr1 and allowed_rtmr2 must each name at least one measurement. RTMR3 \
             is extended from public inputs, so it only identifies the image when the kernel \
             (RTMR1) and the kernel command line + initrd (RTMR2) are pinned as well -- \
             without them a custom kernel or initrd can reproduce a locked profile's RTMR3. \
             Take the values for each profile you accept from that release's \
             published-mrtds.json."
        );
    }

    Ok(GroupOp::TeeAdmissionPolicySet {
        allowed_mrtd,
        allowed_rtmr0,
        allowed_rtmr1,
        allowed_rtmr2,
        allowed_rtmr3,
        allowed_tcb_statuses,
        accept_mock,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trust(profiles: &[&str], min: Option<&str>) -> SignedReleaseTrust {
        SignedReleaseTrust {
            allowed_profiles: profiles.iter().map(|p| (*p).to_owned()).collect(),
            min_release_version: min.map(str::to_owned),
        }
    }

    #[test]
    fn a_signed_release_policy_becomes_its_own_op_with_a_normalised_floor() {
        let empty = Vec::new();
        let op = signed_release_op(
            trust(&["locked-read-only"], Some("mero-tee-v2.3.72")),
            [&empty; 5],
            vec!["UpToDate".to_owned()],
            false,
        )
        .unwrap();
        let GroupOp::TeeReleaseAdmissionPolicySet {
            allowed_profiles,
            min_release_version,
            ..
        } = op
        else {
            panic!("expected the signed-release op, got {op:?}")
        };
        assert_eq!(allowed_profiles, vec!["locked-read-only".to_owned()]);
        assert_eq!(min_release_version.as_deref(), Some("2.3.72"));
    }

    #[test]
    fn a_signed_release_policy_refuses_lists_and_needs_a_profile() {
        let empty = Vec::new();
        let listed = vec!["aa".to_owned()];
        assert_invalid(
            signed_release_op(
                trust(&["locked-read-only"], None),
                [&empty, &empty, &empty, &empty, &listed],
                vec![],
                false,
            ),
            "leave the measurement lists empty",
        );
        assert_invalid(
            signed_release_op(trust(&[" "], None), [&empty; 5], vec![], false),
            "at least one image profile",
        );
        assert!(signed_release_op(
            trust(&["locked-read-only"], Some("x")),
            [&empty; 5],
            vec![],
            false
        )
        .is_err());
    }

    /// A policy the admission gate would refuse every node under is refused
    /// now, as the typed `400`: never stored, and never the untyped `500`.
    #[test]
    fn a_list_policy_without_rtmr3_is_refused_as_invalid() {
        let one = || vec!["aa".to_owned()];
        assert_invalid(
            list_policy_op(one(), vec![], one(), one(), vec![], vec![], false),
            "allowed_rtmr3 must name at least one measurement",
        );
    }

    /// The refusal is `TeePolicyInvalid`, and its message names the field.
    fn assert_invalid(result: eyre::Result<GroupOp>, names: &str) {
        let err = result.expect_err("the policy must be refused");
        assert!(
            matches!(
                err.downcast_ref::<crate::error::ContextError>(),
                Some(crate::error::ContextError::TeePolicyInvalid { .. })
            ),
            "expected TeePolicyInvalid, got: {err:#}"
        );
        assert!(err.to_string().contains(names), "got: {err}");
    }
}
