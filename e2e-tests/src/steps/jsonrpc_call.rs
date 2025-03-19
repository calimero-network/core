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
    Invitees,
}

impl Test for CallStep {
    fn display_name(&self) -> String {
        format!("call ({}, {:?})", self.method_name, self.target)
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let context_id;

        let mut public_keys = HashMap::new();
        
        match self.target {
            CallTarget::Inviter => {
                if let Some(ref inviter_public_key) = ctx.inviter_public_key {
                    drop(public_keys.insert(ctx.inviter.clone(), inviter_public_key.clone()));
                } else {
                    bail!("Inviter public key is required for JsonRpcExecuteStep");
                }
            },
            CallTarget::AllMembers => {
                if let Some(ref inviter_public_key) = ctx.inviter_public_key {
                    drop(public_keys.insert(ctx.inviter.clone(), inviter_public_key.clone()));
                } else {
                    bail!("Inviter public key is required for JsonRpcExecuteStep");
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
            },
            CallTarget::Invitees => {
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
        println!("number of public keys: {}", public_keys.len());

        let mut args_json = self.args_json.clone();

        if self.method_name == "approve_proposal" {
            if let Some(ref proposal_id) = ctx.proposal_id {
                args_json["proposal_id"] = serde_json::Value::String(proposal_id.clone());
            } else {
                bail!("Proposal ID is required for JsonRpcExecuteStep");
            }
        }

        println!("args_json: {:?}", args_json);

        for (node, public_key) in &public_keys {
            let response = ctx
                .meroctl
                .json_rpc_execute(
                    node,
                    context_id,
                    &self.method_name,
                    &args_json,
                    public_key,
                )
                .await?;
            println!("response: {:?}", response);
            if self.method_name == "create_new_proposal" {
                let output = response
                    .get("result")
                    .ok_or_else(|| eyre!("No result in response"))?
                    .get("output")
                    .ok_or_else(|| eyre!("No output in result"))?
                    .as_str()
                    .ok_or_else(|| eyre!("Output is not a string"))?
                    .to_string();
                println!("output: {:?}", output);
                ctx.proposal_id = Some(output);
            }
            
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
