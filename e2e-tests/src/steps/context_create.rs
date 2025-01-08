use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextCreateStep;

impl Test for ContextCreateStep {
    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let Some(ref application_id) = ctx.application_id else {
            bail!("Application ID is required for ContextCreateStep");
        };

        let (context_id, member_public_key) = ctx
            .meroctl
            .context_create(&ctx.inviter, &application_id)
            .await?;

        ctx.context_id = Some(context_id);
        ctx.inviter_public_key = Some(member_public_key);

        ctx.output_writer.write_string(format!(
            "Report: Created context on '{}' node",
            &ctx.inviter
        ));

        Ok(())
    }
}
