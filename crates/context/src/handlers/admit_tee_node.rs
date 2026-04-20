use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::AdmitTeeNodeRequest;
use calimero_context_client::local_governance::GroupOp;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<AdmitTeeNodeRequest> for ContextManager {
    type Result = ActorResponse<Self, <AdmitTeeNodeRequest as Message>::Result>;

    fn handle(
        &mut self,
        AdmitTeeNodeRequest {
            group_id,
            member,
            quote_hash,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            is_mock,
        }: AdmitTeeNodeRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_namespace_identity(&group_id);

        let requester = match node_identity {
            Some((pk, _)) => pk,
            None => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "node has no configured group identity for TEE admission"
                )))
            }
        };

        let node_sk = node_identity.map(|(_, sk)| sk);

        let policy = match group_store::read_tee_admission_policy(&self.datastore, &group_id) {
            Ok(Some(p)) => p,
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "no TeeAdmissionPolicy set for group"
                )))
            }
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        if is_mock && !policy.accept_mock {
            return ActorResponse::reply(Err(eyre::eyre!(
                "mock attestation rejected by group policy"
            )));
        }

        if policy.allowed_mrtd.is_empty() {
            return ActorResponse::reply(Err(eyre::eyre!(
                "TEE admission policy has empty allowed_mrtd — at least one MRTD must be specified"
            )));
        }
        if !policy.allowed_mrtd.iter().any(|a| a == &mrtd) {
            return ActorResponse::reply(Err(eyre::eyre!("MRTD not in policy allowlist")));
        }
        if !policy.allowed_tcb_statuses.is_empty()
            && !policy.allowed_tcb_statuses.iter().any(|a| a == &tcb_status)
        {
            return ActorResponse::reply(Err(eyre::eyre!("TCB status not in policy allowlist")));
        }
        if !policy.allowed_rtmr0.is_empty() && !policy.allowed_rtmr0.iter().any(|a| a == &rtmr0) {
            return ActorResponse::reply(Err(eyre::eyre!("RTMR0 not in policy allowlist")));
        }
        if !policy.allowed_rtmr1.is_empty() && !policy.allowed_rtmr1.iter().any(|a| a == &rtmr1) {
            return ActorResponse::reply(Err(eyre::eyre!("RTMR1 not in policy allowlist")));
        }
        if !policy.allowed_rtmr2.is_empty() && !policy.allowed_rtmr2.iter().any(|a| a == &rtmr2) {
            return ActorResponse::reply(Err(eyre::eyre!("RTMR2 not in policy allowlist")));
        }
        if !policy.allowed_rtmr3.is_empty() && !policy.allowed_rtmr3.iter().any(|a| a == &rtmr3) {
            return ActorResponse::reply(Err(eyre::eyre!("RTMR3 not in policy allowlist")));
        }

        match group_store::check_group_membership(&self.datastore, &group_id, &member) {
            Ok(true) => return ActorResponse::reply(Ok(())),
            Ok(false) => {}
            Err(e) => return ActorResponse::reply(Err(e)),
        }

        match group_store::is_quote_hash_used(&self.datastore, &group_id, &quote_hash) {
            Ok(true) => {
                return ActorResponse::reply(Err(eyre::eyre!("TEE attestation quote already used")))
            }
            Ok(false) => {}
            Err(e) => return ActorResponse::reply(Err(e)),
        }

        if let Some(ref sk) = node_sk {
            if let Err(err) =
                group_store::store_group_signing_key(&self.datastore, &group_id, &requester, sk)
            {
                tracing::warn!(?group_id, %requester, error = %err, "Failed to persist group signing key");
            }
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let effective_signing_key = node_sk.or_else(|| {
            group_store::get_group_signing_key(&self.datastore, &group_id, &requester)
                .ok()
                .flatten()
        });

        ActorResponse::r#async(
            async move {
                let sk =
                    PrivateKey::from(effective_signing_key.ok_or_else(|| {
                        eyre::eyre!("no signing key available for TEE admission")
                    })?);
                group_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &group_id,
                    &sk,
                    GroupOp::MemberJoinedViaTeeAttestation {
                        member,
                        quote_hash,
                        mrtd,
                        rtmr0,
                        rtmr1,
                        rtmr2,
                        rtmr3,
                        tcb_status,
                        role: GroupMemberRole::ReadOnlyTee,
                    },
                )
                .await?;

                info!(%member, ?group_id, "TEE node admitted via attestation");
                // Auto-follow flags for the admitted TEE member are
                // published by the member itself in `fleet_join.rs` after
                // it observes admission — signed with its own namespace
                // identity, which satisfies `MemberSetAutoFollow`'s
                // admin-or-self authorization rule. The verifier (this
                // handler) has neither admin authority nor the member's
                // signing key, so it can't do it here.

                Ok(())
            }
            .into_actor(self),
        )
    }
}
