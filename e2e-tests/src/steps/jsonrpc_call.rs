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
}

impl Test for CallStep {
    fn display_name(&self) -> String {
        format!("call ({}, {:?})", self.method_name, self.target)
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let context_id;

        let mut public_keys = HashMap::new();
        if let Some(ref inviter_public_key) = ctx.inviter_public_key {
            drop(public_keys.insert(ctx.inviter.clone(), inviter_public_key.clone()));
        } else {
            bail!("Inviter public key is required for JsonRpcExecuteStep");
        }

        match self.target {
            CallTarget::Inviter => {
                if let Some(ref alias) = ctx.context_alias {
                    context_id = alias;
                } else {
                    bail!("Alias is required for JsonRpcExecuteStep on the Inviter node");
                };
            }
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

        let mut tasks = JoinSet::new();

        let task = |node: String, public_key: String, count: u8| {
            let task = ctx.meroctl.json_rpc_execute(
                &node,
                context_id,
                &self.method_name,
                &self.args_json,
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

                let Some(expected_result_json) = &self.expected_result_json else {
                    continue;
                };

                if expected_result_json == output {
                    continue;
                }

                if !can_retry {
                    bail!(
                        "JSON RPC result output mismatch:\nexpected: {}\nactual  : {}",
                        expected_result_json,
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
