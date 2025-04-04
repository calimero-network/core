use std::collections::BTreeMap;
use std::sync::Arc;

use actix::prelude::{Actor, Context, Handler, Message, ResponseFuture};
use calimero_primitives::context::ContextId;
use calimero_utils_actix::global_runtime;
use tokio::sync::Mutex;
use wasmer::{Engine, Instance, Module, NativeEngineExt, Store};

mod constraint;
pub mod errors;
pub mod logic;
mod memory;
pub mod store;

pub use constraint::Constraint;
use errors::{FunctionCallError, VMRuntimeError};
use logic::{Outcome, VMContext, VMLimits, VMLogic, VMLogicError};
use memory::WasmerTunables;
use store::Storage;

pub type RuntimeResult<T, E = VMRuntimeError> = Result<T, E>;

type ExecuteResponse = Option<(RuntimeResult<Outcome>, Box<dyn Storage + Send>)>;

#[expect(missing_debug_implementations, reason = "not needed")]
#[derive(Message)]
#[rtype(ExecuteResponse)]
pub struct ExecuteRequest {
    pub blob: Vec<u8>,
    pub method_name: String,
    pub context: VMContext<'static>,
    pub storage: Box<dyn Storage + Send>,
}

#[derive(Debug)]
pub struct RuntimeManager {
    pub tasks: BTreeMap<ContextId, Arc<Mutex<()>>>,
    pub limits: VMLimits,
}

impl Actor for RuntimeManager {
    type Context = Context<Self>;
}

impl RuntimeManager {
    pub fn new(limits: VMLimits) -> Self {
        RuntimeManager {
            tasks: BTreeMap::new(),
            limits,
        }
    }
}

impl Handler<ExecuteRequest> for RuntimeManager {
    type Result = ResponseFuture<ExecuteResponse>;

    fn handle(&mut self, msg: ExecuteRequest, _ctx: &mut Self::Context) -> Self::Result {
        let mutex = self
            .tasks
            .entry(msg.context.context_id.into())
            .or_default()
            .clone();

        let limits = self.limits.clone();

        let future = async move {
            let _lock = mutex.lock().await;

            let handle = global_runtime().spawn_blocking(move || {
                let mut msg = msg;

                let result = run(
                    &msg.blob,
                    &msg.method_name,
                    msg.context,
                    &mut *msg.storage,
                    &limits,
                );

                (result, msg.storage)
            });

            handle.await.ok()
        };

        Box::pin(future)
    }
}

pub fn run(
    code: &[u8],
    method_name: &str,
    context: VMContext<'_>,
    storage: &mut dyn Storage,
    limits: &VMLimits,
) -> RuntimeResult<Outcome> {
    // todo! calculate storage key for cached precompiled
    // todo! module, execute that, instead of recompiling
    let mut engine = Engine::default();

    engine.set_tunables(WasmerTunables::new(limits));

    let mut store = Store::new(engine);

    let mut logic = VMLogic::new(storage, context, limits);

    // todo! apply a prepare step
    // todo! - parse the wasm blob, validate and apply transformations
    // todo!   - validations:
    // todo!     - there is no memory import
    // todo!     - there is no _start function
    // todo!   - transformations:
    // todo!     - remove memory export
    // todo!     - remove memory section
    // todo! cache the compiled module in storage for later

    let module = match Module::new(&store, code) {
        Ok(module) => module,
        Err(err) => return Ok(logic.finish(Some(err.into()))),
    };

    let imports = logic.imports(&mut store);

    let instance = match Instance::new(&mut store, &module, &imports) {
        Ok(instance) => instance,
        Err(err) => return Ok(logic.finish(Some(err.into()))),
    };

    let _ = match instance.exports.get_memory("memory") {
        Ok(memory) => logic.with_memory(memory.clone()),
        // todo! test memory returns MethodNotFound
        Err(err) => return Ok(logic.finish(Some(err.into()))),
    };

    let function = match instance.exports.get_function(method_name) {
        Ok(function) => function,
        Err(err) => return Ok(logic.finish(Some(err.into()))),
    };

    let signature = function.ty(&store);

    if !(signature.params().is_empty() && signature.results().is_empty()) {
        return Ok(logic.finish(Some(FunctionCallError::MethodResolutionError(
            errors::MethodResolutionError::InvalidSignature {
                name: method_name.to_owned(),
            },
        ))));
    }

    if let Err(err) = function.call(&mut store, &[]) {
        return match err.downcast::<VMLogicError>() {
            Ok(err) => Ok(logic.finish(Some(err.try_into()?))),
            Err(err) => Ok(logic.finish(Some(err.into()))),
        };
    }

    Ok(logic.finish(None))
}

#[cfg(test)]
mod integration_tests_package_usage {
    use {eyre as _, owo_colors as _, rand as _};
}
