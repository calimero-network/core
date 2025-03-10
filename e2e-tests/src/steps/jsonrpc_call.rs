use std::collections::HashMap;

use eyre::{bail, eyre, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallStep {
    pub method_name: String,
    pub args_json: serde_json::Value,
    pub expected_result_json: Option<serde_json::Value>,
    pub target: CallTarget,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CallTarget {
    Inviter,
    AllMembers,
}

impl Test for CallStep {
    fn display_name(&self) -> String {
        format!("call ({}, {:?})", self.method_name, self.target)
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let mut context_id = if let Some(ref alias) = ctx.context_alias {
            alias
        } else {
            bail!("Context ID or Alias is required for JsonRpcExecuteStep");
        };

        let mut public_keys = HashMap::new();
        if let Some(ref inviter_public_key) = ctx.inviter_public_key {
            drop(public_keys.insert(ctx.inviter.clone(), inviter_public_key.clone()));
        } else {
            bail!("Inviter public key is required for JsonRpcExecuteStep");
        }

        match self.target {
            CallTarget::Inviter => {}
            CallTarget::AllMembers => {
                if let Some(ref id) = ctx.context_id {
                    context_id = id;
                } else {
                    bail!("Context ID is required for JsonRpcExecuteStep with AllMembers target");
                }

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
        }

        for (node, public_key) in &public_keys {
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

            ctx.output_writer
                .write_str(&format!("Report: Call on '{node}' node passed assertion"));
        }

        Ok(())
    }
}
