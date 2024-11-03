use eyre::Result as EyreResult;
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteJoinContextStep;

impl Test for InviteJoinContextStep {
    async fn run_assert(&self, ctx: &TestContext<'_>) -> EyreResult<()> {
        let (invitee_public_key, invitee_private_key) =
            ctx.meroctl.identity_generate(&ctx.invitee_node).await?;

        let context_id = ctx
            .get_context_id()
            .expect("Context ID is required for InviteJoinContextStep");
        let inviteer_public_key = ctx
            .get_inviter_public_key()
            .expect("Inviter public key is required for InviteJoinContextStep");

        let invitation_payload = ctx
            .meroctl
            .context_invite(
                &ctx.inviter_node,
                &context_id,
                &inviteer_public_key,
                &invitee_public_key,
            )
            .await?;

        let (invitee_context_id, invite_member_public_key) = ctx
            .meroctl
            .context_join(&ctx.invitee_node, &invitee_private_key, &invitation_payload)
            .await?;

        assert_eq!(context_id, invitee_context_id);
        assert_eq!(invitee_public_key, invite_member_public_key);

        Ok(())
    }
}
