use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::AdmitTeeNodeRequest;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use eyre::bail;
use tracing::{debug, info, warn};

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
            tcb_status,
            is_mock,
        }: AdmitTeeNodeRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

        let requester = match node_identity {
            Some((pk, _)) => pk,
            None => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "node has no configured group identity for TEE admission"
                )))
            }
        };

        let node_sk = node_identity.map(|(_, sk)| sk);

        if let Err(err) = (|| -> eyre::Result<()> {
            let policy = group_store::read_tee_admission_policy(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("no TeeAdmissionPolicy set for group"))?;

            if is_mock && !policy.accept_mock {
                bail!("mock attestation rejected by group policy");
            }

            // Empty allowlist = accept any value
            if !policy.allowed_mrtd.is_empty() && !policy.allowed_mrtd.iter().any(|a| a == &mrtd) {
                bail!("MRTD not in policy allowlist");
            }
            if !policy.allowed_tcb_statuses.is_empty()
                && !policy.allowed_tcb_statuses.iter().any(|a| a == &tcb_status)
            {
                bail!("TCB status not in policy allowlist");
            }

            if group_store::check_group_membership(&self.datastore, &group_id, &member)? {
                debug!(%member, "TEE node already a group member, skipping");
                return Ok(());
            }

            if group_store::is_quote_hash_used(&self.datastore, &group_id, &quote_hash)? {
                warn!(quote_hash = %hex::encode(quote_hash), "quote already used (replay)");
                bail!("TEE attestation quote already used");
            }

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
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
                        tcb_status,
                        role: GroupMemberRole::Member,
                    },
                )
                .await?;

                info!(%member, ?group_id, "TEE node admitted via attestation");
                Ok(())
            }
            .into_actor(self),
        )
    }
}
