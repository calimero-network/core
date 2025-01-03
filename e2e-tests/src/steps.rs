use application_install::ApplicationInstallStep;
use context_create::ContextCreateStep;
use context_invite_join::ContextInviteJoinStep;
use jsonrpc_call::JsonRpcCallStep;
use serde::{Deserialize, Serialize};

mod application_install;
mod context_create;
mod context_invite_join;
mod jsonrpc_call;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestScenario {
    pub steps: Box<[TestStep]>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TestStep {
    ApplicationInstall(ApplicationInstallStep),
    ContextCreate(ContextCreateStep),
    ContextInviteJoin(ContextInviteJoinStep),
    JsonRpcCall(JsonRpcCallStep),
}
