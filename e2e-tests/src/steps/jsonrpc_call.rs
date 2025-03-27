use std::collections::HashMap;

use eyre::{bail, OptionExt, Result as EyreResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::JoinSet;
use tokio::time;

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallStep {
    pub method_name: String,
    pub args_json: Value,
    pub expected_result_json: Option<Value>,
    pub target: CallTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
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
        let context_id = ctx.context_id.as_ref().unwrap();

        let mut public_keys = HashMap::new();

        match self.target {
            CallTarget::Inviter => {
                if let Some(ref inviter_public_key) = ctx.inviter_public_key {
                    drop(public_keys.insert(ctx.inviter.clone(), inviter_public_key.clone()));
                } else {
                    bail!("Inviter public key is required for JsonRpcExecuteStep");
                }
            }
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
            }
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
        let mut args_json = self.args_json.clone();

        process_json_variables(&mut args_json, ctx)?;

        let mut tasks = JoinSet::new();

        let task = |node: String, public_key: String, count: u8| {
            let task = ctx.meroctl.json_rpc_execute(
                &node,
                context_id,
                &self.method_name,
                &args_json,
                &public_key,
            );

            async move { (task.await, node, public_key, count) }
        };

        for (node, public_key) in public_keys {
            let _ignored = tasks.spawn(task(node, public_key, 0));
        }

        while let Some((response, node, public_key, count)) = tasks.join_next().await.transpose()? {
            let can_retry = count < self.retries.unwrap_or(0);

            if let Ok(response) = &response {
                let output = response
                    .get("result")
                    .ok_or_eyre("result not found in JSON RPC response")?
                    .get("output")
                    .ok_or_eyre("output not found in JSON RPC response result")?;

                let modified_expected_result =
                    if let Some(expected_json) = &self.expected_result_json {
                        let mut expected_clone = expected_json.clone();
                        process_json_variables(&mut expected_clone, ctx)?;
                        Some(expected_clone)
                    } else {
                        None
                    };

                let Some(expected_result) = &modified_expected_result else {
                    continue;
                };

                if expected_result == output {
                    continue;
                }

                if !can_retry {
                    bail!(
                        "JSON RPC result output mismatch:\nexpected: {}\nactual  : {}",
                        expected_result,
                        output
                    );
                }
            }

            if can_retry {
                ctx.output_writer
                    .write_str(&format!("Retrying JSON RPC call for node {}", node));

                let delay = self
                    .interval_ms
                    .map(|s| time::sleep(time::Duration::from_millis(s)));

                let task = task(node, public_key, count + 1);

                let _ignored = tasks.spawn(async move {
                    if let Some(delay) = delay {
                        delay.await;
                    }

                    task.await
                });

                continue;
            }

            let _ignored = response?;
        }

        Ok(())
    }
}

/// Recursively processes a JSON value, replacing variable references
/// like ${variable_name} with corresponding values from the TestContext.
///
/// Used internally by CallStep to:
/// 1. Substitute variables in JSON RPC input arguments before making the call
/// 2. Process expected result templates to compare with actual responses
///
/// For example:
/// - Input args: {"proposalId": "${proposal_id}"}
/// - Expected result: {"status": "${proposal_id}"}
///
/// # Arguments
/// * `value` - JSON value to process, modified in place
/// * `ctx` - Test context containing variable values
///
/// # Errors
/// * If a referenced variable is not found in the context
fn process_json_variables(value: &mut Value, ctx: &TestContext<'_>) -> EyreResult<()> {
    match value {
        Value::String(s) => {
            if s.starts_with("${") && s.ends_with("}") {
                let var_name = s.trim_start_matches("${").trim_end_matches("}");

                let replacement = match var_name {
                    "proposal_id" => ctx.proposal_id.clone(),
                    // Add other fields as needed
                    _ => None,
                };

                if let Some(new_value) = replacement {
                    *s = new_value;
                } else {
                    bail!("Variable '{}' not found in context", var_name);
                }
            }
        }
        Value::Object(obj) => {
            for (_, v) in obj {
                process_json_variables(v, ctx)?;
            }
        }
        Value::Array(arr) => {
            for item in arr {
                process_json_variables(item, ctx)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {
            // These values don't contain variables, so no processing needed
        }
    }
    Ok(())
}
