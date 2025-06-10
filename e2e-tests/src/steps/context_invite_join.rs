use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextInviteJoinStep;

impl Test for ContextInviteJoinStep {
    fn display_name(&self) -> String {
        "ctx invite-join".to_owned()
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let Some(ref context_id) = ctx.context_id else {
            bail!("Context ID is required for InviteJoinContextStep");
        };

        let Some(ref inviter_public_key) = ctx.inviter_public_key else {
            bail!("Inviter public key is required for InviteJoinContextStep");
        };

        for invitee in &ctx.invitees {
            let invitee_public_key = ctx.meroctl.identity_generate(invitee).await?;

            let invitation_payload = ctx
                .meroctl
                .context_invite(
                    &ctx.inviter,
                    context_id,
                    inviter_public_key,
                    &invitee_public_key,
                )
                .await?;

            let (invitee_context_id, invitee_member_public_key) = ctx
                .meroctl
                .context_join(invitee, &invitation_payload)
                .await?;

            if *context_id != invitee_context_id {
                bail!(
                    "Context ID mismatch: {} != {}",
                    context_id,
                    invitee_context_id
                );
            }

            if invitee_public_key != invitee_member_public_key {
                bail!(
                    "Invitee public key mismatch: {} != {}",
                    invitee_public_key,
                    invitee_member_public_key
                );
            }

            drop(
                ctx.invitees_public_keys
                    .insert(invitee.clone(), invitee_public_key),
            );

            ctx.output_writer
                .write_str(&format!("Report: Node '{invitee}' joined the context"));
        }

        Ok(())
    }
}
