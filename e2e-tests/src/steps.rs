use application_install::ApplicationInstallStep;
use context_create::ContextCreateStep;
use context_invite_join::ContextInviteJoinStep;
use eyre::Result as EyreResult;
use jsonrpc_call::JsonRpcCallStep;
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

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

impl Test for TestStep {
    fn display_name(&self) -> String {
        match self {
            TestStep::ApplicationInstall(step) => step.display_name(),
            TestStep::ContextCreate(step) => step.display_name(),
            TestStep::ContextInviteJoin(step) => step.display_name(),
            TestStep::JsonRpcCall(step) => step.display_name(),
        }
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        match self {
            TestStep::ApplicationInstall(step) => step.run_assert(ctx).await,
            TestStep::ContextCreate(step) => step.run_assert(ctx).await,
            TestStep::ContextInviteJoin(step) => step.run_assert(ctx).await,
            TestStep::JsonRpcCall(step) => step.run_assert(ctx).await,
        }
    }
}
