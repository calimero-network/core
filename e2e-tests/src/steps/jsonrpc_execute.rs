use eyre::{bail, Result as EyreResult};
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
    async fn run_assert(&self, ctx: &TestContext<'_>) -> EyreResult<()> {
        let context_id = ctx
            .get_context_id()
            .expect("Context ID is required for InviteJoinContextStep");

        let response = ctx
            .meroctl
            .json_rpc_execute(
                &ctx.inviter_node,
                &context_id,
                &self.method_name,
                &self.args_json,
            )
            .await?;

        if let Some(expected_result_json) = &self.expected_result_json {
            if *expected_result_json != response["result"]["output"] {
                bail!(
                    "JSON RPC Result mismatch: {} != {}",
                    *expected_result_json,
                    response["result"]["output"]
                );
            }
        }

        Ok(())
    }
}
