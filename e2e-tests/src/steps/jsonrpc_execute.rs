use eyre::{bail, eyre, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcExecuteStep {
    pub method_name: String,
    pub args_json: serde_json::Value,
    pub expected_result_json: Option<serde_json::Value>,
}

impl Test for JsonRpcExecuteStep {
    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let Some(ref context_id) = ctx.context_id else {
            bail!("Context ID is required for InviteJoinContextStep");
        };

        let response = ctx
            .meroctl
            .json_rpc_execute(
                &ctx.inviter_node,
                context_id,
                &self.method_name,
                &self.args_json,
            )
            .await?;

        if let Some(expected_result_json) = &self.expected_result_json {
            let output = response
                .get("result")
                .ok_or_else(|| eyre!("result not found in JSON RPC response"))?
                .get("output")
                .ok_or_else(|| eyre!("output not found in JSON RPC response result"))?;

            if expected_result_json != output {
                bail!(
                    "JSON RPC result output mismatch:\nexpected: {}\nactual  : {}",
                    expected_result_json,
                    output
                );
            }
        }

        Ok(())
    }
}
