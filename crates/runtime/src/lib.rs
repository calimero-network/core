use std::panic::{catch_unwind, AssertUnwindSafe};

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tracing::{debug, error, info};
use wasmer::{CompileError, DeserializeError, Instance, SerializeError, Store};

// Profiling feature: Only compile these imports when profiling feature is enabled
#[cfg(feature = "profiling")]
use wasmer::sys::{CompilerConfig, Cranelift};

mod constants;
mod constraint;
pub mod errors;
pub mod logic;
mod memory;
pub mod store;

pub use constraint::Constraint;
use errors::{FunctionCallError, HostError, Location, PanicContext, VMRuntimeError};
use logic::{ContextHost, Outcome, VMContext, VMLimits, VMLogic, VMLogicError};
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

        let engine = Self::create_engine();

        Self::new(engine, limits)
    }
}

impl Engine {
    #[must_use]
    pub fn new(mut engine: wasmer::Engine, limits: VMLimits) -> Self {
        // Set tunables if this is a sys engine (native engine)
        if engine.is_sys() {
            use wasmer::sys::NativeEngineExt;
            engine.set_tunables(WasmerTunables::new(&limits));
        }

        Self { limits, engine }
    }

    /// Create an engine, using Cranelift compiler for profiling builds with PerfMap support
    fn create_engine() -> wasmer::Engine {
        #[cfg(feature = "profiling")]
        {
            if std::env::var("ENABLE_WASMER_PROFILING")
                .map(|v| v == "true")
                .unwrap_or(false)
            {
                info!("Enabling Wasmer PerfMap profiling for WASM stack traces");
                // Create Cranelift config and enable PerfMap file generation
                let mut config = Cranelift::default();
                config.enable_perfmap();
                return wasmer::Engine::from(config);
            }
        }

        // Default engine (no profiling)
        wasmer::Engine::default()
    }

    #[must_use]
    pub fn headless() -> Self {
        let limits = VMLimits::default();

        // Headless engines lack a compiler, so Wasmer skips perf.map generation.
        // For profiling, use a full engine to enable WASM symbol resolution.
        #[cfg(feature = "profiling")]
        {
            if std::env::var("ENABLE_WASMER_PROFILING")
                .map(|v| v == "true")
                .unwrap_or(false)
            {
                debug!("Using profiling-enabled engine for precompiled module (required for perf.map generation)");
                let engine = Self::create_engine();
                return Self::new(engine, limits);
            }
        }

        use wasmer::sys::NativeEngineExt;
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
        context_host: Option<Box<dyn ContextHost>>,
    ) -> RuntimeResult<Outcome> {
        let context_id = context;
        info!(%context_id, method, "Running WASM method");
        debug!(%context_id, method, input_len = input.len(), "WASM execution input");

        let context = VMContext::new(input.into(), *context_id, *executor);

        let mut logic = VMLogic::new(storage, context, &self.limits, node_client, context_host);

        let mut store = Store::new(self.engine.clone());

        let imports = logic.imports(&mut store);

        // Wrap WASM execution in catch_unwind to prevent panics from crashing the node.
        // This catches any unhandled panics during instance creation, memory access,
        // or function execution and converts them to proper error responses.
        let execution_result = catch_unwind(AssertUnwindSafe(|| {
            Self::execute_wasm(
                &mut store,
                &self.module,
                &imports,
                &mut logic,
                method,
                &context_id,
            )
        }));

        // Determine the error to pass to finish() based on execution result
        let err = match execution_result {
            Ok(Ok(err)) => err,
            Ok(Err(e)) => return Err(e),
            Err(panic_payload) => {
                // Extract panic message from the payload
                let message = extract_panic_message(&panic_payload);
                error!(
                    %context_id,
                    method,
                    panic_message = %message,
                    "WASM execution panicked"
                );
                Some(FunctionCallError::HostError(HostError::Panic {
                    context: PanicContext::Guest,
                    message,
                    location: Location::Unknown,
                }))
            }
        };

        let outcome = logic.finish(err);
        if outcome.returns.is_ok() {
            info!(%context_id, method, "WASM method execution completed");
            debug!(
                %context_id,
                method,
                has_return = outcome.returns.as_ref().is_ok_and(Option::is_some),
                logs_count = outcome.logs.len(),
                events_count = outcome.events.len(),
                "WASM execution outcome"
            );
        }

        Ok(outcome)
    }

    /// Execute the WASM function within a catch_unwind boundary.
    /// This method is separated to allow catch_unwind to capture any panics.
    /// Returns `Ok(Some(error))` if execution failed with an error,
    /// `Ok(None)` if execution succeeded, or `Err` for critical runtime errors.
    fn execute_wasm(
        store: &mut Store,
        module: &wasmer::Module,
        imports: &wasmer::Imports,
        logic: &mut VMLogic<'_>,
        method: &str,
        context_id: &ContextId,
    ) -> RuntimeResult<Option<FunctionCallError>> {
        let instance = match Instance::new(store, module, imports) {
            Ok(instance) => instance,
            Err(err) => {
                error!(%context_id, method, error=?err, "Failed to instantiate WASM module");
                return Ok(Some(err.into()));
            }
        };

        let _ = match instance.exports.get_memory("memory") {
            Ok(memory) => logic.with_memory(memory.clone()),
            // todo! test memory returns MethodNotFound
            Err(err) => {
                error!(%context_id, method, error=?err, "Failed to get WASM memory");
                return Ok(Some(err.into()));
            }
        };

        // Call the auto-generated registration hook if it exists.
        // This enables automatic CRDT merge during sync.
        // Note: This is optional and failures are non-fatal (especially for JS apps).
        if let Ok(register_fn) = instance
            .exports
            .get_typed_function::<(), ()>(store, "__calimero_register_merge")
        {
            match register_fn.call(store) {
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
                return Ok(Some(err.into()));
            }
        };

        let signature = function.ty(store);

        if !(signature.params().is_empty() && signature.results().is_empty()) {
            error!(%context_id, method, "Invalid method signature");
            return Ok(Some(FunctionCallError::MethodResolutionError(
                errors::MethodResolutionError::InvalidSignature {
                    name: method.to_owned(),
                },
            )));
        }

        if let Err(err) = function.call(store, &[]) {
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
                Ok(err) => Ok(Some(err.try_into()?)),
                Err(err) => Ok(Some(err.into())),
            };
        }

        Ok(None)
    }
}

/// Extracts a human-readable message from a panic payload.
/// Panics can carry either a `&'static str` or a `String` as their message.
fn extract_panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<unknown panic>".to_owned()
    }
}

#[cfg(test)]
mod integration_tests_package_usage {
    use {eyre as _, owo_colors as _, rand as _};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_panic_message_with_str() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("test panic message");
        let message = extract_panic_message(&payload);
        assert_eq!(message, "test panic message");
    }

    #[test]
    fn test_extract_panic_message_with_string() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("owned panic message"));
        let message = extract_panic_message(&payload);
        assert_eq!(message, "owned panic message");
    }

    #[test]
    fn test_extract_panic_message_with_unknown_type() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(42_i32);
        let message = extract_panic_message(&payload);
        assert_eq!(message, "<unknown panic>");
    }
}
