use application_install::ApplicationInstallStep;
use context_create::ContextCreateStep;
use context_create_alias::ContextCreateAliasStep;
use context_invite_join::ContextInviteJoinStep;
use eyre::Result as EyreResult;
use jsonrpc_call::CallStep;
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

mod application_install;
mod context_create;
mod context_create_alias;
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
    ContextCreateAlias(ContextCreateAliasStep),
    ContextInviteJoin(ContextInviteJoinStep),
    Call(CallStep),
}

impl Test for TestStep {
    fn display_name(&self) -> String {
        match self {
            Self::ApplicationInstall(step) => step.display_name(),
            Self::ContextCreate(step) => step.display_name(),
            Self::ContextCreateAlias(step) => step.display_name(),
            Self::ContextInviteJoin(step) => step.display_name(),
            Self::Call(step) => step.display_name(),
        }
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        match self {
            Self::ApplicationInstall(step) => step.run_assert(ctx).await,
            Self::ContextCreate(step) => step.run_assert(ctx).await,
            Self::ContextCreateAlias(step) => step.run_assert(ctx).await,
            Self::ContextInviteJoin(step) => step.run_assert(ctx).await,
            Self::Call(step) => step.run_assert(ctx).await,
        }
    }
}
