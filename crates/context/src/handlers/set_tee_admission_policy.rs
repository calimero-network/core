use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::SetTeeAdmissionPolicyRequest;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_node_primitives::sync::GroupMutationKind;
use calimero_primitives::identity::PrivateKey;
use eyre::bail;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

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
            max_replicas,
            requester,
        }: SetTeeAdmissionPolicyRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured group identity"
                    )))
                }
            },
        };

        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            if signing_key.is_none() {
                group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;
            }

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        if let Some(ref sk) = signing_key {
            if let Err(err) =
                group_store::store_group_signing_key(&self.datastore, &group_id, &requester, sk)
            {
                tracing::warn!(?group_id, %requester, error = %err, "Failed to persist group signing key");
            }
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let effective_signing_key = signing_key.or_else(|| {
            group_store::get_group_signing_key(&self.datastore, &group_id, &requester)
                .ok()
                .flatten()
        });

        ActorResponse::r#async(
            async move {
                let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                    eyre::eyre!("local group governance requires a signing key for the requester")
                })?);
                group_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
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
                        max_replicas,
                    },
                )
                .await?;

                let _ = node_client
                    .broadcast_group_mutation(
                        group_id.to_bytes(),
                        GroupMutationKind::TeeAdmissionPolicySet,
                    )
                    .await;

                info!(
                    ?group_id,
                    max_replicas,
                    accept_mock,
                    "TEE admission policy updated"
                );

                Ok(())
            }
            .into_actor(self),
        )
    }
}
