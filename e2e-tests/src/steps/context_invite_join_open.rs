use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextInviteJoinOpenStep;

impl Test for ContextInviteJoinOpenStep {
    fn display_name(&self) -> String {
        "ctx invite-join open-invitation".to_owned()
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let Some(ref context_id) = ctx.context_id else {
            bail!("Context ID is required for ContextInviteJoinOpenStep");
        };

        let Some(ref inviter_public_key) = ctx.inviter_public_key else {
            bail!("Inviter public key is required for ContextInviteJoinOpenStep");
        };

        // TODO: allow specify validity in the step
        let valid_for_blocks = 1000_u64;

        for invitee in &ctx.invitees {
            let invitee_public_key = ctx.meroctl.identity_generate(invitee).await?;
            println!(
                "Generated on {invitee} a new identity: {}",
                invitee_public_key
            );

            let signed_open_invitation = ctx
                .meroctl
                .context_invite_by_open_invitation(
                    &ctx.inviter,
                    context_id,
                    inviter_public_key,
                    valid_for_blocks,
                )
                .await?;

            println!("E2E: successfully ran context_invite_by_open_invitation");
            let (invitee_context_id, invitee_member_public_key) = ctx
                .meroctl
                .context_join_by_open_invitation(
                    invitee,
                    &signed_open_invitation,
                    &invitee_public_key,
                )
                .await?;
            println!("E2E: successfully ran context_join_by_open_invitation");

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
