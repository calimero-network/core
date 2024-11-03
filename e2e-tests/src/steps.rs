use context_create::CreateContextStep;
use context_invite_join::InviteJoinContextStep;
use jsonrpc_execute::JsonRpcExecuteStep;
use serde::{Deserialize, Serialize};

mod context_create;
mod context_invite_join;
mod jsonrpc_execute;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestScenario {
    pub steps: Box<[TestStep]>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TestStep {
    ContextCreate(CreateContextStep),
    ContextInviteJoin(InviteJoinContextStep),
    JsonRpcExecute(JsonRpcExecuteStep),
}
