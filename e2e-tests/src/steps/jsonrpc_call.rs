use eyre::{bail, eyre, Result as EyreResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcCallStep {
    pub method_name: String,
    pub args_json: serde_json::Value,
    pub expected_result_json: Option<serde_json::Value>,
    pub target: JsonRpcCallTarget,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JsonRpcCallTarget {
    Inviter,
    AllMembers,
}

impl Test for JsonRpcCallStep {
    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let Some(ref context_id) = ctx.context_id else {
            bail!("Context ID is required for JsonRpcExecuteStep");
        };

        let mut public_keys = HashMap::new();
        if let Some(ref inviter_public_key) = ctx.inviter_public_key {
            drop(public_keys.insert(ctx.inviter.clone(), inviter_public_key.clone()));
        } else {
            bail!("Inviter public key is required for JsonRpcExecuteStep");
        }

        if let JsonRpcCallTarget::AllMembers = self.target {
            for invitee in &ctx.invitees {
                if let Some(invitee_public_key) = ctx.invitees_public_keys.get(invitee) {
                    drop(public_keys.insert(invitee.clone(), invitee_public_key.clone()));
                } else {
                    bail!(
                        "Public key for invitee '{}' is required for JsonRpcExecuteStep",
                        invitee
                    );
                }
            }
        }

        for (node, public_key) in public_keys.iter() {
            let response = ctx
                .meroctl
                .json_rpc_execute(
                    node,
                    context_id,
                    &self.method_name,
                    &self.args_json,
                    public_key,
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

            println!("Report: Call on '{}' node passed assertion", node)
        }

        Ok(())
    }
}
