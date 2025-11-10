use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tracing::{debug, error, info};
use wasmer::{CompileError, DeserializeError, Instance, NativeEngineExt, SerializeError, Store};

mod constants;
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

#[derive(Clone, Debug)]
pub struct Engine {
    limits: VMLimits,
    engine: wasmer::Engine,
}

impl Default for Engine {
    fn default() -> Self {
        let limits = VMLimits::default();

        let engine = wasmer::Engine::default();

        Self::new(engine, limits)
    }
}

impl Engine {
    #[must_use]
    pub fn new(mut engine: wasmer::Engine, limits: VMLimits) -> Self {
        engine.set_tunables(WasmerTunables::new(&limits));

        Self { limits, engine }
    }

    #[must_use]
    pub fn headless() -> Self {
        let limits = VMLimits::default();

        let engine = wasmer::Engine::headless();

        Self::new(engine, limits)
    }

    pub fn compile(&self, bytes: &[u8]) -> Result<Module, CompileError> {
        // todo! apply a prepare step
        // todo! - parse the wasm blob, validate and apply transformations
        // todo!   - validations:
        // todo!     - there is no memory import
        // todo!     - there is no _start function
        // todo!   - transformations:
        // todo!     - remove memory export
        // todo!     - remove memory section
        // todo! cache the compiled module in storage for later

        let module = wasmer::Module::new(&self.engine, bytes)?;

        Ok(Module {
            limits: self.limits,
            engine: self.engine.clone(),
            module,
        })
    }

    pub unsafe fn from_precompiled(&self, bytes: &[u8]) -> Result<Module, DeserializeError> {
        let module = wasmer::Module::deserialize(&self.engine, bytes)?;

        Ok(Module {
            limits: self.limits,
            engine: self.engine.clone(),
            module,
        })
    }
}

#[derive(Debug)]
pub struct Module {
    limits: VMLimits,
    engine: wasmer::Engine,
    module: wasmer::Module,
}

impl Module {
    pub fn to_bytes(&self) -> Result<Box<[u8]>, SerializeError> {
        let bytes = self.module.serialize()?;

        Ok(Vec::into_boxed_slice(bytes.into()))
    }

    pub fn run(
        &self,
        context: ContextId,
        executor: PublicKey,
        method: &str,
        input: &[u8],
        storage: &mut dyn Storage,
        node_client: Option<NodeClient>,
    ) -> RuntimeResult<Outcome> {
        let context_id = context;
        info!(%context_id, method, "Running WASM method");
        debug!(%context_id, method, input_len = input.len(), "WASM execution input");

        let context = VMContext::new(input.into(), *context_id, *executor);

        let mut logic = VMLogic::new(storage, context, &self.limits, node_client);

        let mut store = Store::new(self.engine.clone());

        let imports = logic.imports(&mut store);

        let instance = match Instance::new(&mut store, &self.module, &imports) {
            Ok(instance) => instance,
            Err(err) => {
                error!(%context_id, method, error=?err, "Failed to instantiate WASM module");
                return Ok(logic.finish(Some(err.into())));
            }
        };

        let _ = match instance.exports.get_memory("memory") {
            Ok(memory) => logic.with_memory(memory.clone()),
            // todo! test memory returns MethodNotFound
            Err(err) => {
                error!(%context_id, method, error=?err, "Failed to get WASM memory");
                return Ok(logic.finish(Some(err.into())));
            }
        };

        // Call the auto-generated registration hook if it exists.
        // This enables automatic CRDT merge during sync.
        // Note: This is optional and failures are non-fatal (especially for JS apps).
        if let Ok(register_fn) = instance
            .exports
            .get_typed_function::<(), ()>(&store, "__calimero_register_merge")
        {
            match register_fn.call(&mut store) {
                Ok(()) => {
                    debug!(%context_id, "Successfully registered CRDT merge function");
                }
                Err(err) => {
                    // Log but don't fail - registration is optional (backward compat)
                    // JS apps may not have this function properly initialized yet.
                    debug!(
                        %context_id,
                        error=?err,
                        "Failed to call merge registration hook (non-fatal, continuing)"
                    );
                }
            }
        }

        let function = match instance.exports.get_function(method) {
            Ok(function) => function,
            Err(err) => {
                error!(%context_id, method, error=?err, "Method not found in WASM module");
                return Ok(logic.finish(Some(err.into())));
            }
        };

        let signature = function.ty(&store);

        if !(signature.params().is_empty() && signature.results().is_empty()) {
            error!(%context_id, method, "Invalid method signature");
            return Ok(logic.finish(Some(FunctionCallError::MethodResolutionError(
                errors::MethodResolutionError::InvalidSignature {
                    name: method.to_owned(),
                },
            ))));
        }

        if let Err(err) = function.call(&mut store, &[]) {
            let traces = err
                .trace()
                .iter()
                .map(|frame| {
                    let module = frame.module_name();
                    let func = frame.function_name().unwrap_or("<unknown-func>");
                    let offset = frame.func_offset();
                    let offset = if offset == 0 {
                        String::new()
                    } else {
                        format!("@0x{offset:x}")
                    };
                    format!("{module}::{func}{offset}")
                })
                .collect::<Vec<_>>();
            let trace_joined = if traces.is_empty() {
                None
            } else {
                Some(traces.join(" -> "))
            };

            let message = err.message();
            let message_str = if message.is_empty() {
                "<no error message>"
            } else {
                message.as_str()
            };

            error!(
                %context_id,
                method,
                error_debug = ?err,
                error_message = %message_str,
                wasm_trace = trace_joined.as_deref(),
                "WASM method execution failed"
            );

            return match err.downcast::<VMLogicError>() {
                Ok(err) => Ok(logic.finish(Some(err.try_into()?))),
                Err(err) => Ok(logic.finish(Some(err.into()))),
            };
        }

        let outcome = logic.finish(None);
        info!(%context_id, method, "WASM method execution completed");
        debug!(
            %context_id,
            method,
            has_return = outcome.returns.is_ok(),
            logs_count = outcome.logs.len(),
            events_count = outcome.events.len(),
            "WASM execution outcome"
        );

        Ok(outcome)
    }
}

#[cfg(test)]
mod integration_tests_package_usage {
    use {eyre as _, owo_colors as _, rand as _};
}
