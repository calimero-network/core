use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

/// Step to retrieve and process proposals from a context, storing the first proposal ID
/// in the test context for use in subsequent steps.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalsStep {
    /// JSON arguments to pass to the get proposals request
    pub args_json: serde_json::Value,
}

impl Test for GetProposalsStep {
    fn display_name(&self) -> String {
        "get proposals".to_owned()
    }

    /// Executes the get proposals step by:
    /// 1. Retrieving proposals using meroctl for the given context
    /// 2. Extracting proposal IDs from the response
    /// 3. Storing the first proposal ID in the test context
    ///
    /// # Errors
    /// * If context ID is not set in the test context
    /// * If no proposals are found in the response
    /// * If the meroctl request fails
    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let Some(ref context_id) = ctx.context_id else {
            bail!("Context ID is required for GetProposalsStep");
        };

        let proposals = ctx
            .meroctl
            .get_proposals(&ctx.inviter, context_id, &self.args_json)
            .await?;

        let mut proposal_id = None;
        if let Some(proposals) = proposals["data"].as_array() {
            for proposal in proposals {
                if let Some(id) = proposal["id"].as_str() {
                    proposal_id = Some(id);
                    break;
                }
            }
        }
        let Some(proposal_id) = proposal_id else {
            bail!("No proposal IDs found in response: {:?}", proposals);
        };
        ctx.proposal_id = Some(proposal_id.to_string());

        ctx.output_writer
            .write_str(&format!("Report: Get proposals on '{}' node", &ctx.inviter));

        Ok(())
    }
}
