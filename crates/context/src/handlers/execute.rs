use std::borrow::Cow;

use actix::{Handler, Message, ResponseFuture};
use calimero_context_primitives::messages::execute::ExecuteRequest;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::Outcome;
use calimero_utils_actix::global_runtime;
use eyre::WrapErr;
use tokio::sync::OwnedMutexGuard;

use crate::ContextManager;

pub mod storage;

use storage::ContextStorage;

impl Handler<ExecuteRequest> for ContextManager {
    type Result = ResponseFuture<<ExecuteRequest as Message>::Result>;

    fn handle(&mut self, msg: ExecuteRequest, ctx: &mut Self::Context) -> Self::Result {
        todo!("localize the functionalities of the runtime here instead")
    }
}

pub async fn execute(
    context: &OwnedMutexGuard<ContextId>,
    module: calimero_runtime::Module,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    executor: PublicKey,
    mut storage: ContextStorage,
) -> eyre::Result<(Outcome, ContextStorage)> {
    let context_id = **context;

    global_runtime()
        .spawn_blocking(move || {
            let outcome = module.run(context_id, &method, &input, executor, &mut storage)?;

            Ok((outcome, storage))
        })
        .await
        .wrap_err("failed to receive execution response")?
}
