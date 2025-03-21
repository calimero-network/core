use std::collections::HashMap;

use eyre::{bail, OptionExt, Result as EyreResult};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tokio::time;

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallStep {
    pub method_name: String,
    pub args_json: serde_json::Value,
    pub expected_result_json: Option<serde_json::Value>,
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

        if self.method_name == "approve_proposal" || self.method_name == "get_proposal_messages" {
            if let Some(ref proposal_id) = ctx.proposal_id {
                args_json["proposal_id"] = serde_json::Value::String(proposal_id.clone());
            } else {
                bail!("Proposal ID is required for JsonRpcExecuteStep");
            }
        }

        if self.method_name == "send_proposal_messages" {
            println!("send_proposal_messages ctx.proposal_id: {:?}", ctx.proposal_id);
            if let Some(ref proposal_id) = ctx.proposal_id {
                args_json["proposal_id"] = serde_json::Value::String(proposal_id.clone());
                args_json["message"]["proposal_id"] = serde_json::Value::String(proposal_id.clone());
            } else {
                bail!("Proposal ID is required for JsonRpcExecuteStep");
            }
        }

        println!("args_json: {:?}", args_json);

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
                println!("response: {:?}", response);
                let output = response
                    .get("result")
                    .ok_or_eyre("result not found in JSON RPC response")?
                    .get("output")
                    .ok_or_eyre("output not found in JSON RPC response result")?;

                if self.method_name == "create_new_proposal" {
                    let proposal_id_str = output.as_str()
                        .ok_or_eyre("Expected proposal ID to be a string")?
                        .to_string();
                    
                    ctx.proposal_id = Some(proposal_id_str);
                    println!("ctx.proposal_id: {:?}", ctx.proposal_id);
                }

                let modified_expected_result = if self.method_name == "get_proposal_messages" && self.expected_result_json.is_some() {
                    let mut expected_clone = self.expected_result_json.clone().unwrap();
                    
                    if let (Some(proposal_id), Some(array)) = (ctx.proposal_id.clone(), expected_clone.as_array_mut()) {
                        if let Some(first_msg) = array.first_mut() {
                            if let Some(obj) = first_msg.as_object_mut() {
                                let _unused = obj.insert(
                                    "proposal_id".to_string(),
                                    serde_json::Value::String(proposal_id.clone())
                                );
                            }
                        }
                    }
                    
                    Some(expected_clone)
                } else {
                    self.expected_result_json.clone()
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
