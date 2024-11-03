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
    async fn run_assert(&self, ctx: &TestContext<'_>) -> EyreResult<()> {
        let app_id = match &self.application {
            ApplicationSource::LocalFile(path) => {
                ctx.meroctl
                    .application_install(&ctx.inviter_node, path)
                    .await?
            }
        };

        let (context_id, member_public_key) = ctx
            .meroctl
            .context_create(&ctx.inviter_node, &app_id)
            .await?;

        ctx.set_context_id(context_id);
        ctx.set_inviter_public_key(member_public_key);

        Ok(())
    }
}
