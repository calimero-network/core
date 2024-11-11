use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteJoinContextStep;

impl Test for InviteJoinContextStep {
    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let (invitee_public_key, invitee_private_key) =
            ctx.meroctl.identity_generate(&ctx.invitee_node).await?;

        let Some(ref context_id) = ctx.context_id else {
            bail!("Context ID is required for InviteJoinContextStep");
        };

        let Some(ref inviter_public_key) = ctx.inviter_public_key else {
            bail!("Inviter public key is required for InviteJoinContextStep");
        };

        let invitation_payload = ctx
            .meroctl
            .context_invite(
                &ctx.inviter_node,
                context_id,
                inviter_public_key,
                &invitee_public_key,
            )
            .await?;

        let (invitee_context_id, invite_member_public_key) = ctx
            .meroctl
            .context_join(&ctx.invitee_node, &invitee_private_key, &invitation_payload)
            .await?;

        if *context_id != invitee_context_id {
            bail!(
                "Context ID mismatch: {} != {}",
                context_id,
                invitee_context_id
            );
        }

        if invitee_public_key != invite_member_public_key {
            bail!(
                "Invitee public key mismatch: {} != {}",
                invitee_public_key,
                invite_member_public_key
            );
        }

        Ok(())
    }
}
