use eyre::Result as EyreResult;
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextStep {
    pub application: ApplicationSource,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplicationSource {
    LocalFile(String),
    // CalimeroRegistry(String),
}

impl Test for CreateContextStep {
    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let app_id = match &self.application {
            ApplicationSource::LocalFile(path) => {
                ctx.meroctl.application_install(&ctx.inviter, path).await?
            }
        };

        let (context_id, member_public_key) =
            ctx.meroctl.context_create(&ctx.inviter, &app_id).await?;

        ctx.context_id = Some(context_id);
        ctx.inviter_public_key = Some(member_public_key);

        ctx.output_writer.write_string(format!(
            "Report: Created context on '{}' node",
            &ctx.inviter
        ));

        Ok(())
    }
}
