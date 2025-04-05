use std::sync::Arc;

use actix::{Handler, Message, ResponseFuture};
use calimero_context_primitives::messages::execute::ExecuteRequest;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::{Outcome, VMContext, VMLimits};
use calimero_runtime::Constraint;
use calimero_utils_actix::global_runtime;
use eyre::Context;
use storage::ContextStorage;
use tokio::sync::Mutex;

use crate::ContextManager;

mod storage;

impl Handler<ExecuteRequest> for ContextManager {
    type Result = ResponseFuture<<ExecuteRequest as Message>::Result>;

    fn handle(&mut self, msg: ExecuteRequest, ctx: &mut Self::Context) -> Self::Result {
        todo!("localize the functionalities of the runtime here instead")
    }
}

pub async fn execute(
    context: Arc<Mutex<ContextId>>,
    blob: Arc<impl AsRef<[u8]> + Send + Sync + 'static>,
    method: impl AsRef<str> + Send + 'static,
    input: impl AsRef<[u8]> + Send + 'static,
    executor: PublicKey,
    mut storage: ContextStorage,
) -> eyre::Result<(Outcome, ContextStorage)> {
    let context_id = context.lock_owned().await;

    let limits = default_limits()?;

    global_runtime()
        .spawn_blocking(move || {
            let context = VMContext::new(input.as_ref(), **context_id, *executor);

            let outcome = calimero_runtime::run(
                (*blob).as_ref(),
                method.as_ref(),
                context,
                &limits,
                &mut storage,
            )?;

            Ok((outcome, storage))
        })
        .await
        .wrap_err("failed to receive execution response")?
}

// TODO: also this would be nice to have global default with per application customization
fn default_limits() -> eyre::Result<VMLimits> {
    Ok(VMLimits {
        max_memory_pages: 1 << 10,                      // 1 KiB
        max_stack_size: 200 << 10,                      // 200 KiB
        max_registers: 100,                             //
        max_register_size: (100 << 20).validate()?,     // 100 MiB
        max_registers_capacity: 1 << 30,                // 1 GiB
        max_logs: 100,                                  //
        max_log_size: 16 << 10,                         // 16 KiB
        max_events: 100,                                //
        max_event_kind_size: 100,                       //
        max_event_data_size: 16 << 10,                  // 16 KiB
        max_storage_key_size: (1 << 20).try_into()?,    // 1 MiB
        max_storage_value_size: (10 << 20).try_into()?, // 10 MiB
    })
}
