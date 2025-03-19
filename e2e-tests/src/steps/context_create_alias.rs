use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextCreateAliasStep {
    pub alias_name: String,
}

impl Test for ContextCreateAliasStep {
    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let Some(ref context_id) = ctx.context_id else {
            bail!("To create an alias we need a context id")
        };

        let _ = ctx
            .meroctl
            .context_create_alias(&ctx.inviter, context_id, &self.alias_name)
            .await?;

        ctx.output_writer.write_str(&format!(
            "Created alias {} for context {}",
            self.alias_name, context_id
        ));

        let alias_context_id = ctx
            .meroctl
            .context_get_alias(&ctx.inviter, &self.alias_name)
            .await?;

        if alias_context_id != *context_id {
            bail!("Creating alias for context failed");
        }

        ctx.context_alias = Some(self.alias_name.clone());

        Ok(())
    }

    fn display_name(&self) -> String {
        "alias create".to_owned()
    }
}
