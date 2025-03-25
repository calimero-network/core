use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalsStep {
    pub args_json: serde_json::Value,
}

impl Test for GetProposalsStep {
    fn display_name(&self) -> String {
        "get proposals".to_owned()
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let Some(ref context_id) = ctx.context_id else {
            bail!("Context ID is required for GetProposalsStep");
        };

        let proposals = ctx
            .meroctl
            .get_proposals(&ctx.inviter, context_id, &self.args_json)
            .await?;

        // Extract all proposal IDs
        let mut ids = Vec::new();

        if let Some(proposals) = proposals.get("data").and_then(|data| data.as_array()) {
            for proposal in proposals {
                if let Some(id) = proposal.get("id").and_then(|id| id.as_str()) {
                    ids.push(id.to_string());
                }
            }
        }

        if ids.is_empty() {
            bail!("No proposal IDs found in response: {:?}", proposals);
        }

        ctx.proposal_id = Some(ids.first().unwrap().clone());

        ctx.output_writer
            .write_str(&format!("Report: Get proposals on '{}' node", &ctx.inviter));

        Ok(())
    }
}
