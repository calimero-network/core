use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tracing::{debug, error, info};
// `CompilerConfig` brings `push_middleware`/`enable_perfmap` into scope for the
// Cranelift config built in `create_engine`.
use wasmer::sys::{CompilerConfig, Cranelift};
use wasmer::{Instance, SerializeError, Store};
use wasmer_middlewares::Metering;

pub mod config;
mod constants;
mod constraint;
pub mod errors;
pub mod logic;
mod memory;
pub mod metering;
mod panic_payload;
pub mod store;

pub use config::{RuntimeConfig, RuntimeLimitsConfig};
pub use constraint::Constraint;
use errors::{
    FunctionCallError, HostError, Location, PanicContext, PrecompiledModuleError, VMRuntimeError,
};
use logic::{CallbackHandlerGuard, Outcome, VMContext, VMLimits, VMLogic, VMLogicError};
use memory::WasmerTunables;
use store::Storage;

pub type RuntimeResult<T, E = VMRuntimeError> = Result<T, E>;

/// Validates a method name for WASM execution.
///
/// Valid method names must:
/// - Not be empty
/// - Not exceed the maximum length limit
/// - Contain only valid characters (ASCII alphanumeric, underscore)
///
/// # Arguments
///
/// * `method` - The method name to validate
/// * `max_length` - The maximum allowed length for the method name
///
/// # Returns
///
/// * `Ok(())` if the method name is valid
/// * `Err(FunctionCallError)` if the method name is invalid
// FunctionCallError carries wasmer's large CompileError/LinkError via #[from];
// boxing the variant would break the derive and ripple through the crate, and
// this is not a hot path.
#[allow(
    clippy::result_large_err,
    reason = "pervasive #[from] error enum; boxing breaks the derive"
)]
fn validate_method_name(method: &str, max_length: u64) -> Result<(), FunctionCallError> {
    // Check for empty method name
    if method.is_empty() {
        return Err(FunctionCallError::MethodResolutionError(
            errors::MethodResolutionError::EmptyMethodName,
        ));
    }

    // Check length limit
    let method_len = method.len();
    if method_len as u64 > max_length {
        return Err(FunctionCallError::MethodResolutionError(
            errors::MethodResolutionError::MethodNameTooLong {
                name: method.to_owned(),
                length: method_len,
                max: max_length,
            },
        ));
    }

    // Validate characters: only allow ASCII alphanumeric and underscore
    // This covers typical WASM export names and Rust/JS function naming conventions
    for (position, character) in method.chars().enumerate() {
        if !character.is_ascii_alphanumeric() && character != '_' {
            return Err(FunctionCallError::MethodResolutionError(
                errors::MethodResolutionError::InvalidMethodNameCharacter {
                    name: method.to_owned(),
                    character,
                    position,
                },
            ));
        }
    }

    Ok(())
}

#[derive(Clone, Debug)]
pub struct Engine {
    limits: VMLimits,
    engine: wasmer::Engine,
}

impl Default for Engine {
    fn default() -> Self {
        Self::with_limits(VMLimits::default())
    }
}

impl Engine {
    /// Wrap a caller-supplied `wasmer::Engine` with the runtime's limits.
    ///
    /// **Metering caveat:** gas metering is instrumented at *compile time* by
    /// the middleware baked into the engine's compiler config. This constructor
    /// takes an already-built engine as-is, so it does **not** add metering: a
    /// plain `wasmer::Engine::default()` passed here compiles *unmetered*
    /// modules, and `limits.max_gas` then has no effect. Prefer
    /// [`Engine::with_limits`] (which builds a metered compiler engine) for any
    /// engine that will `compile` guest code. Passing a headless engine here is
    /// fine — it only deserializes already-instrumented artifacts.
    #[must_use]
    pub fn new(mut engine: wasmer::Engine, limits: VMLimits) -> Self {
        // A self-contradictory limits config (e.g. a total register budget
        // smaller than a single register's cap) is a construction-time
        // programming error: it is never reachable from guest input nor from
        // operator config (which does not expose these fields), only from a code
        // change. A debug assertion catches it in dev/CI/tests without a
        // process-fatal panic in release library code.
        debug_assert!(
            limits.validate_invariants().is_ok(),
            "invalid VMLimits passed to Engine::new: max_registers_capacity must be >= max_register_size"
        );

        // Set tunables if this is a sys engine (native engine)
        if engine.is_sys() {
            use wasmer::sys::NativeEngineExt;
            engine.set_tunables(WasmerTunables::new(&limits));
        }

        Self { limits, engine }
    }

    /// Build the compiling engine used for guest modules.
    ///
    /// The engine is a Cranelift engine carrying the [gas metering
    /// middleware](crate::metering), which instruments every module it compiles
    /// with a decrementing points counter so an untrusted guest cannot run
    /// unbounded (see the module docs for why this is metering and not a
    /// timeout). `initial_gas` is baked in as the counter's starting value, but
    /// it is only a fallback: each execution overrides it per-run via
    /// [`metering::set_gas_limit`] before calling the guest.
    ///
    /// Under the `profiling` feature (and when `ENABLE_WASMER_PROFILING=true`)
    /// PerfMap emission is layered on the same Cranelift config, so profiling
    /// and metering compose rather than being mutually exclusive.
    ///
    /// One engine compiles exactly one module: the metering middleware panics if
    /// reused across modules, and every caller here constructs a fresh engine
    /// per compile, so this is not a constraint in practice.
    fn create_engine(initial_gas: u64) -> wasmer::Engine {
        let mut config = Cranelift::default();

        #[cfg(feature = "profiling")]
        {
            if std::env::var("ENABLE_WASMER_PROFILING")
                .map(|v| v == "true")
                .unwrap_or(false)
            {
                info!("Enabling Wasmer PerfMap profiling for WASM stack traces");
                config.enable_perfmap();
            }
        }

        config.push_middleware(Arc::new(Metering::new(initial_gas, metering::gas_cost)));

        wasmer::Engine::from(config)
    }

    /// Like [`Engine::default`], but with operator-configured `limits` instead
    /// of [`VMLimits::default`]. The limits are baked into every `Module` this
    /// engine compiles and applied at execution time.
    #[must_use]
    pub fn with_limits(limits: VMLimits) -> Self {
        let engine = Self::create_engine(limits.max_gas);

        Self::new(engine, limits)
    }

    #[must_use]
    pub fn headless() -> Self {
        Self::headless_with_limits(VMLimits::default())
    }

    /// Like [`Engine::headless`], but with operator-configured `limits` instead
    /// of [`VMLimits::default`]. The limits are baked into every `Module` this
    /// engine deserializes and applied at execution time.
    #[must_use]
    pub fn headless_with_limits(limits: VMLimits) -> Self {
        // Headless engines lack a compiler, so Wasmer skips perf.map generation.
        // For profiling, use a full engine to enable WASM symbol resolution.
        #[cfg(feature = "profiling")]
        {
            if std::env::var("ENABLE_WASMER_PROFILING")
                .map(|v| v == "true")
                .unwrap_or(false)
            {
                debug!("Using profiling-enabled engine for precompiled module (required for perf.map generation)");
                return Self::with_limits(limits);
            }
        }

        use wasmer::sys::NativeEngineExt;
        let engine = wasmer::Engine::headless();

        Self::new(engine, limits)
    }

    #[allow(
        clippy::result_large_err,
        reason = "pervasive #[from] error enum; boxing breaks the derive"
    )]
    pub fn compile(&self, bytes: &[u8]) -> Result<Module, FunctionCallError> {
        // Check module size before compilation to prevent memory exhaustion attacks
        // Note: `as u64` is safe because usize <= u64 on all supported platforms (32-bit and 64-bit)
        let size = bytes.len() as u64;
        if size > self.limits.max_module_size {
            tracing::warn!(
                size,
                max = self.limits.max_module_size,
                "WASM module size limit exceeded"
            );
            return Err(FunctionCallError::ModuleSizeLimitExceeded {
                size,
                max: self.limits.max_module_size,
            });
        }

        let module = wasmer::Module::new(&self.engine, bytes)?;

        Self::validate_guest_module(&module)?;

        Ok(Module {
            limits: self.limits,
            engine: self.engine.clone(),
            module,
        })
    }

    /// Reject untrusted guest modules whose shape would let them escape the
    /// sandbox contract the runtime relies on. Runs once per compile, after
    /// wasmer has validated the bytes are well-formed WASM.
    ///
    /// Three checks:
    ///
    /// * **No imported memory.** A module that *imports* its linear memory
    ///   expects the host to hand it one, which the runtime never provides; such
    ///   a module could only be attempting to alias host-supplied memory, so it
    ///   is rejected outright rather than failing deeper in instantiation.
    /// * **Exports a memory named `memory`.** The host reads guest state through
    ///   `instance.exports.get_memory("memory")`, so a guest that exports no such
    ///   memory cannot be run. Requiring it here turns what would otherwise be a
    ///   confusing export-not-found failure at instantiation into a clear
    ///   validation error at compile time.
    /// * **No `_start` export.** `_start` is the entry point a WASI *command*
    ///   build emits (`wasm32-wasip1`/`wasm32-wasi` toolchains) — the WASI ABI's
    ///   equivalent of `main`, meant to run once on start. It is a property of
    ///   how the guest was *compiled*, independent of wasmer (the engine we run
    ///   it with). Calimero guests are libraries built for
    ///   `wasm32-unknown-unknown` and invoked by explicit method name, so a
    ///   `_start` export means the module was built against the wrong target
    ///   (and likely expects WASI syscall imports the host does not provide). It
    ///   is refused up front rather than instantiated in a half-supported shape.
    #[allow(
        clippy::result_large_err,
        reason = "pervasive #[from] error enum; boxing breaks the derive"
    )]
    fn validate_guest_module(module: &wasmer::Module) -> Result<(), FunctionCallError> {
        use wasmer::ExternType;

        for import in module.imports() {
            if matches!(import.ty(), ExternType::Memory(_)) {
                return Err(FunctionCallError::ModuleValidationError {
                    reason: format!(
                        "guest imports memory `{}::{}`; guests must define and export \
                         their own memory, not import it from the host",
                        import.module(),
                        import.name()
                    ),
                });
            }
        }

        if module.exports().any(|export| {
            export.name() == "_start" && matches!(export.ty(), ExternType::Function(_))
        }) {
            return Err(FunctionCallError::ModuleValidationError {
                reason: "guest exports a `_start` function; WASI command entry points are \
                         not supported (guests are invoked by explicit method name)"
                    .to_owned(),
            });
        }

        let exports_memory = module.exports().any(|export| {
            export.name() == "memory" && matches!(export.ty(), ExternType::Memory(_))
        });
        if !exports_memory {
            return Err(FunctionCallError::ModuleValidationError {
                reason: "guest does not export a linear memory named `memory`; the host \
                         reads guest state through that export"
                    .to_owned(),
            });
        }

        Ok(())
    }

    /// Deserialize a precompiled (serialized) module produced by
    /// [`Module::to_bytes`].
    ///
    /// # Safety
    ///
    /// `wasmer::Module::deserialize` trusts its input: it maps the bytes
    /// straight into an executable artifact without re-validating them. Feeding
    /// it attacker-controlled bytes is undefined behavior. The caller MUST
    /// ensure `bytes` originate from a trusted source — the only sound
    /// provenance is bytes this very node produced via [`Module::to_bytes`]
    /// (e.g. its own on-disk compilation cache). The `unsafe` marker is the
    /// provenance assertion: every call site must justify, at the point of
    /// call, why its bytes are trusted.
    ///
    /// # Size cap (defense-in-depth)
    ///
    /// Unlike the original design, this method now enforces a configurable size
    /// cap ([`VMLimits::max_precompiled_module_size`]) *before* handing the
    /// bytes to wasmer. Trusted provenance does not preclude a truncated cache
    /// entry or a corrupt-on-disk artifact, and deserialization allocates
    /// proportionally to the input; the cap bounds that allocation regardless.
    /// It is intentionally separate from (and larger than)
    /// [`VMLimits::max_module_size`], since a serialized artifact is bigger than
    /// the source WASM it came from.
    pub unsafe fn from_precompiled(&self, bytes: &[u8]) -> Result<Module, PrecompiledModuleError> {
        // Note: `as u64` is safe because usize <= u64 on all supported platforms.
        let size = bytes.len() as u64;
        if size > self.limits.max_precompiled_module_size {
            tracing::warn!(
                size,
                max = self.limits.max_precompiled_module_size,
                "precompiled WASM module size limit exceeded"
            );
            return Err(PrecompiledModuleError::SizeLimitExceeded {
                size,
                max: self.limits.max_precompiled_module_size,
            });
        }

        let module = wasmer::Module::deserialize(&self.engine, bytes)?;

        Ok(Module {
            limits: self.limits,
            engine: self.engine.clone(),
            module,
        })
    }
}

/// A compiled WASM module ready for execution.
///
/// Cheap to clone: `wasmer::Engine` and `wasmer::Module` are both
/// `Arc`-backed internally, so cloning shares the compiled artifact
/// rather than re-deserializing it. Used by the `ContextManager`
/// module cache to serve repeat execute requests without paying the
/// `Engine::from_precompiled` cost each time.
#[derive(Clone, Debug)]
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

    /// Run a method with no cross-context origin (direct/RPC call). Thin
    /// wrapper over [`run_with_origin`](Self::run_with_origin).
    // The args are orthogonal (identity, method, I/O, storage, node client) with
    // no cohesive grouping, and this is the runtime's primary hot execution entry
    // called from many sites; a param struct would add ceremony without clarity.
    #[allow(
        clippy::too_many_arguments,
        reason = "orthogonal args on the primary execution entry point"
    )]
    pub fn run<'a>(
        &'a self,
        context: ContextId,
        executor: PublicKey,
        method: &str,
        input: &'a [u8],
        storage: &'a mut dyn Storage,
        private_storage: Option<&'a mut dyn Storage>,
        node_client: Option<NodeClient>,
    ) -> RuntimeResult<Outcome> {
        self.run_with_origin(
            context,
            executor,
            method,
            input,
            storage,
            private_storage,
            node_client,
            None,
        )
    }

    /// Run a method, optionally tagged with the source context that dispatched
    /// it via `xcall`. `xcall_origin` is surfaced to the guest through
    /// `env::xcall_origin()` so a target can authorize its caller; `None` for
    /// direct/RPC calls.
    #[allow(clippy::too_many_arguments, reason = "execution context is wide")]
    pub fn run_with_origin<'a>(
        &'a self,
        context: ContextId,
        executor: PublicKey,
        method: &str,
        input: &'a [u8],
        storage: &'a mut dyn Storage,
        private_storage: Option<&'a mut dyn Storage>,
        node_client: Option<NodeClient>,
        xcall_origin: Option<ContextId>,
    ) -> RuntimeResult<Outcome> {
        let context_id = context;
        info!(%context_id, method, "Running WASM method");
        debug!(%context_id, method, input_len = input.len(), "WASM execution input");

        let mut context = VMContext::new(input.into(), *context_id, *executor);
        context.xcall_origin = xcall_origin.map(|origin| *origin);

        let mut logic = VMLogic::new(storage, private_storage, context, &self.limits, node_client);

        // Scope the callback-handler thread-local to this execution. The runtime
        // reuses OS threads, so without this guard a handler name set while
        // running one context could leak into a later execution and misattribute
        // its events. The guard clears any stale value on entry and restores the
        // prior one on drop (kept until the end of this fn, past `finish`).
        let _callback_handler_scope = CallbackHandlerGuard::enter();

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
                self.limits.max_method_name_length,
                self.limits.max_gas,
            )
        }));

        // Determine the error to pass to finish() based on execution result
        let err = match execution_result {
            Ok(Ok(err)) => err,
            Ok(Err(e)) => return Err(e),
            Err(panic_payload) => {
                // Extract panic message from the payload
                let message = panic_payload::panic_payload_to_string(
                    panic_payload.as_ref(),
                    "<unknown panic>",
                );
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
            // Print WASM logs for debugging
            for (i, log) in outcome.logs.iter().enumerate() {
                info!(%context_id, method, log_index = i, log_content = %log, "WASM_LOG");
            }
        }

        Ok(outcome)
    }

    /// Execute the WASM function within a catch_unwind boundary.
    /// This method is separated to allow catch_unwind to capture any panics.
    /// Returns `Ok(Some(error))` if execution failed with an error,
    /// `Ok(None)` if execution succeeded, or `Err` for critical runtime errors.
    #[allow(
        clippy::too_many_arguments,
        reason = "orthogonal execution inputs threaded from run_with_origin"
    )]
    fn execute_wasm(
        store: &mut Store,
        module: &wasmer::Module,
        imports: &wasmer::Imports,
        logic: &mut VMLogic<'_>,
        method: &str,
        context_id: &ContextId,
        max_method_name_length: u64,
        max_gas: u64,
    ) -> RuntimeResult<Option<FunctionCallError>> {
        // Validate method name before attempting to look it up
        if let Err(err) = validate_method_name(method, max_method_name_length) {
            error!(%context_id, method, error=?err, "Invalid method name");
            return Ok(Some(err));
        }
        let instance = match Instance::new(store, module, imports) {
            Ok(instance) => instance,
            Err(err) => {
                error!(%context_id, method, error=?err, "Failed to instantiate WASM module");
                return Ok(Some(err.into()));
            }
        };

        // Charge this whole execution — the merge-registration hook below *and*
        // the guest method — against a single per-run gas budget, overriding the
        // value baked into the module at compile time. Exhaustion traps the
        // guest; we reclassify that trap as `GasExhausted` after the call.
        metering::set_gas_limit(store, &instance, max_gas);

        // Get memory from the WASM instance and attach it to VMLogic.
        // Note: memory.clone() is cheap - it just increments an Arc reference count,
        // not copying actual memory contents. VMLogic::finish() handles cleanup.
        let memory = match instance.exports.get_memory("memory") {
            Ok(memory) => memory.clone(),
            // todo! test memory returns MethodNotFound
            Err(err) => {
                error!(%context_id, method, error=?err, "Failed to get WASM memory");
                return Ok(Some(err.into()));
            }
        };

        // Attach memory to VMLogic, which will clean it up in finish()
        let _ = logic.with_memory(memory);

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
            // A trap whose cause is an exhausted gas budget is reported as
            // `GasExhausted`, not as the generic `unreachable` trap the metering
            // middleware injects to stop the guest. Checked before decoding the
            // trace so the resource-limit case is unambiguous.
            if metering::is_exhausted(store, &instance) {
                error!(%context_id, method, max_gas, "WASM execution exhausted its gas budget");
                return Ok(Some(FunctionCallError::GasExhausted { limit: max_gas }));
            }

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

        if let Some(remaining) = metering::remaining_gas(store, &instance) {
            debug!(
                %context_id,
                method,
                gas_used = max_gas.saturating_sub(remaining),
                gas_remaining = remaining,
                "WASM execution gas accounting"
            );
        }

        Ok(None)
    }
}

#[cfg(test)]
mod integration_tests_package_usage {
    use {eyre as _, owo_colors as _, rand as _, wat as _};
}

/// Integration tests for WASM execution (panic handling, size limits, compilation)
#[cfg(test)]
mod wasm_integration_tests {
    use super::*;
    use crate::store::InMemoryStorage;

    /// Test that a simple WASM module runs successfully (baseline test)
    #[test]
    fn test_wasm_execution_success() {
        // A minimal WASM module with a function that does nothing
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "test_func",
                &[],
                &mut storage,
                None, // No private storage for tests
                None,
            )
            .expect("Failed to run module");

        assert!(
            outcome.returns.is_ok(),
            "Expected successful execution, got: {:?}",
            outcome.returns
        );
    }

    /// Test that calling a non-existent method returns MethodNotFound error
    #[test]
    fn test_wasm_method_not_found() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "existing_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "non_existent_func",
                &[],
                &mut storage,
                None, // No private storage for tests
                None,
            )
            .expect("Failed to run module");

        match &outcome.returns {
            Err(FunctionCallError::MethodResolutionError(
                errors::MethodResolutionError::MethodNotFound { name },
            )) => {
                assert_eq!(name, "non_existent_func");
            }
            other => panic!("Expected MethodNotFound error, got: {other:?}"),
        }
    }

    /// Test that empty method name returns EmptyMethodName error
    #[test]
    fn test_wasm_empty_method_name() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "",
                &[],
                &mut storage,
                None, // No private storage for tests
                None,
            )
            .expect("Failed to run module");

        match &outcome.returns {
            Err(FunctionCallError::MethodResolutionError(
                errors::MethodResolutionError::EmptyMethodName,
            )) => {
                // Expected - empty method name was rejected
            }
            other => panic!("Expected EmptyMethodName error, got: {other:?}"),
        }
    }

    /// Test that method name exceeding max length returns MethodNameTooLong error
    #[test]
    fn test_wasm_method_name_too_long() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        // Create a method name that exceeds the default max length (256 bytes)
        let long_method_name = "a".repeat(300);

        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                &long_method_name,
                &[],
                &mut storage,
                None, // No private storage for tests
                None,
            )
            .expect("Failed to run module");

        match &outcome.returns {
            Err(FunctionCallError::MethodResolutionError(
                errors::MethodResolutionError::MethodNameTooLong { length, max, .. },
            )) => {
                assert_eq!(*length, 300);
                assert_eq!(*max, 256);
            }
            other => panic!("Expected MethodNameTooLong error, got: {other:?}"),
        }
    }

    /// Test that method name with invalid characters returns InvalidMethodNameCharacter error
    #[test]
    fn test_wasm_method_name_invalid_characters() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        // Test various invalid characters
        let invalid_names = [
            ("test func", ' ', 4),   // space
            ("test\nfunc", '\n', 4), // newline
            ("test-func", '-', 4),   // hyphen
            ("test.func", '.', 4),   // dot
            ("test/func", '/', 4),   // slash
        ];

        for (method_name, expected_char, expected_pos) in invalid_names {
            let mut storage = InMemoryStorage::default();
            let outcome = module
                .run(
                    [0; 32].into(),
                    [0; 32].into(),
                    method_name,
                    &[],
                    &mut storage,
                    None, // No private storage for tests
                    None,
                )
                .expect("Failed to run module");

            match &outcome.returns {
                Err(FunctionCallError::MethodResolutionError(
                    errors::MethodResolutionError::InvalidMethodNameCharacter {
                        character,
                        position,
                        ..
                    },
                )) => {
                    assert_eq!(
                        *character, expected_char,
                        "Wrong invalid character for method name: {method_name}"
                    );
                    assert_eq!(
                        *position, expected_pos,
                        "Wrong position for method name: {method_name}"
                    );
                }
                other => panic!(
                    "Expected InvalidMethodNameCharacter error for '{method_name}', got: {other:?}"
                ),
            }
        }
    }

    /// Test that valid method names with various allowed characters work
    #[test]
    fn test_wasm_method_name_valid_characters() {
        // Note: This test verifies that validation passes for valid names.
        // The actual method lookup may fail with MethodNotFound since these
        // methods don't exist in the module, but the validation should pass.
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        // Test valid method names (these should pass validation but may not exist in module)
        let valid_names = [
            "simple",
            "with_underscore",
            "__double_underscore",
            "_leading_underscore",
            "trailing_underscore_",
            "CamelCase",
            "mixedCase123",
            "numbers123",
            "ALLCAPS",
            "a", // single character
        ];

        for method_name in valid_names {
            let mut storage = InMemoryStorage::default();
            let outcome = module
                .run(
                    [0; 32].into(),
                    [0; 32].into(),
                    method_name,
                    &[],
                    &mut storage,
                    None, // No private storage for tests
                    None,
                )
                .expect("Failed to run module");

            // Should get MethodNotFound (not a validation error) since the method doesn't exist
            match &outcome.returns {
                Err(FunctionCallError::MethodResolutionError(
                    errors::MethodResolutionError::MethodNotFound { name },
                )) => {
                    assert_eq!(
                        name, method_name,
                        "Method name should pass validation: {method_name}"
                    );
                }
                // test_func should succeed since it exists
                Ok(_) if method_name == "test_func" => {}
                other => panic!(
                    "Expected MethodNotFound error for valid name '{method_name}', got: {other:?}"
                ),
            }
        }
    }

    /// Test that test_func (valid name) still works correctly
    #[test]
    fn test_wasm_valid_method_name_execution() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "valid_method_name"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "valid_method_name",
                &[],
                &mut storage,
                None, // No private storage for tests
                None,
            )
            .expect("Failed to run module");

        assert!(
            outcome.returns.is_ok(),
            "Expected successful execution with valid method name, got: {:?}",
            outcome.returns
        );
    }

    /// Test that unreachable instruction causes a WasmTrap error (not a crash)
    #[test]
    fn test_wasm_unreachable_trap() {
        // A WASM module with a function that executes unreachable instruction
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "trap_func")
                    unreachable
                )
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "trap_func",
                &[],
                &mut storage,
                None, // No private storage for tests
                None,
            )
            .expect("Failed to run module");

        // The unreachable instruction should cause a WasmTrap::Unreachable error
        match &outcome.returns {
            Err(FunctionCallError::WasmTrap(errors::WasmTrap::Unreachable)) => {
                // Expected - the trap was properly caught and converted to an error
            }
            other => panic!("Expected WasmTrap::Unreachable error, got: {other:?}"),
        }
    }

    /// Test that a WASM module calling the panic host function returns a Panic error
    #[test]
    fn test_wasm_panic_host_function() {
        // A WASM module that calls the panic host function
        // The panic function expects a pointer to a location struct
        let wat = r#"
            (module
                (import "env" "panic" (func $panic (param i64)))
                (memory (export "memory") 1)
                (data (i32.const 0) "test.rs")
                (func (export "panic_func")
                    ;; Store the location struct at offset 100:
                    ;; - ptr to filename (8 bytes): points to 0
                    ;; - len of filename (8 bytes): 7
                    ;; - line (4 bytes): 42
                    ;; - column (4 bytes): 10

                    ;; ptr to filename = 0
                    (i64.store (i32.const 100) (i64.const 0))
                    ;; len of filename = 7
                    (i64.store (i32.const 108) (i64.const 7))
                    ;; line = 42
                    (i32.store (i32.const 116) (i32.const 42))
                    ;; column = 10
                    (i32.store (i32.const 120) (i32.const 10))

                    ;; Call panic with pointer to location struct
                    (call $panic (i64.const 100))
                )
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "panic_func",
                &[],
                &mut storage,
                None, // No private storage for tests
                None,
            )
            .expect("Failed to run module");

        // The panic should be caught and converted to a HostError::Panic
        match &outcome.returns {
            Err(FunctionCallError::HostError(HostError::Panic {
                context, location, ..
            })) => {
                assert!(
                    matches!(context, PanicContext::Guest),
                    "Expected Guest panic context"
                );
                // Verify location information was captured
                match location {
                    Location::At { file, line, column } => {
                        assert_eq!(file, "test.rs");
                        assert_eq!(*line, 42);
                        assert_eq!(*column, 10);
                    }
                    Location::Unknown => panic!("Expected location to be known"),
                }
            }
            other => panic!("Expected HostError::Panic, got: {other:?}"),
        }
    }

    /// A `Storage` whose `get` panics. Used to drive a panic from *inside* a
    /// host function so we can exercise the runtime's per-host-call
    /// `catch_unwind` recovery (the path that replaced the process-global
    /// `set_hook` machinery in `logic/imports.rs`).
    struct PanicStorage;

    impl Storage for PanicStorage {
        fn get(&self, _key: &store::Key) -> Option<store::Value> {
            panic!("storage.get panicked deliberately");
        }
        fn set(&mut self, _key: store::Key, _value: store::Value) -> Option<store::Value> {
            None
        }
        fn remove(&mut self, _key: &store::Key) -> Option<store::Value> {
            None
        }
        fn has(&self, _key: &store::Key) -> bool {
            false
        }
    }

    /// A panic that originates *inside a host function* must be caught by the
    /// runtime's per-host-call `catch_unwind` and surfaced as
    /// `HostError::Panic { context: Host, .. }` with its message intact —
    /// WITHOUT installing a process-global panic hook.
    ///
    /// The message is recovered directly from the unwind payload. Asserting
    /// `location == Unknown` doubles as a regression guard: the deleted
    /// `set_hook` hook captured a *precise* `Location::At { .. }`; the payload
    /// can't carry a location, so it must now degrade to `Unknown`. A revival
    /// of the global-hook approach would make this assertion fail.
    #[test]
    fn test_wasm_host_panic_recovered_without_global_hook() {
        // The guest builds a 16-byte `{ ptr: u64, len: u64 }` buffer descriptor
        // (the host-guest ABI; see `prepare_guest_buf_descriptor`) pointing at a
        // 3-byte key "abc", then calls `storage_read`. The host reads the key and
        // calls `storage.get`, which panics.
        let wat = r#"
            (module
                (import "env" "storage_read" (func $storage_read (param i64 i64) (result i32)))
                (memory (export "memory") 1)
                (func (export "read_probe")
                    ;; descriptor at offset 16: ptr = 100
                    (i64.store (i32.const 16) (i64.const 100))
                    ;; descriptor at offset 24: len = 3
                    (i64.store (i32.const 24) (i64.const 3))
                    ;; key bytes "abc" at offset 100
                    (i32.store8 (i32.const 100) (i32.const 97))
                    (i32.store8 (i32.const 101) (i32.const 98))
                    (i32.store8 (i32.const 102) (i32.const 99))
                    ;; storage_read(descriptor_ptr = 16, register_id = 0)
                    (drop (call $storage_read (i64.const 16) (i64.const 0)))
                )
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        let mut storage = PanicStorage;
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "read_probe",
                &[],
                &mut storage,
                None,
                None,
            )
            .expect("run must return an Outcome, not propagate the unwind");

        match &outcome.returns {
            Err(FunctionCallError::HostError(HostError::Panic {
                context,
                message,
                location,
            })) => {
                assert!(
                    matches!(context, PanicContext::Host),
                    "expected Host panic context, got: {context:?}"
                );
                assert!(
                    message.contains("storage.get panicked deliberately"),
                    "panic message must be recovered from the unwind payload, got: {message:?}"
                );
                // `Unknown` because the unwind payload carries no location. This
                // guards against silently re-introducing the process-global hook
                // (which captured a precise location). If a *non-global* location
                // recovery mechanism is ever added on purpose, update this assertion
                // to expect the recovered location — that is an intended improvement,
                // not a regression.
                assert!(
                    matches!(location, Location::Unknown),
                    "without the global hook the panic location can't be recovered; \
                     expected Unknown, got: {location:?}"
                );
            }
            other => panic!("Expected HostError::Panic, got: {other:?}"),
        }
    }

    /// Test that memory out of bounds causes a WasmTrap error (not a crash)
    #[test]
    fn test_wasm_memory_out_of_bounds() {
        // A WASM module that tries to access memory out of bounds
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "oob_func")
                    ;; Try to load from way beyond the memory limit (1 page = 65536 bytes)
                    (drop (i32.load (i32.const 1000000)))
                )
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "oob_func",
                &[],
                &mut storage,
                None, // No private storage for tests
                None,
            )
            .expect("Failed to run module");

        // Memory out of bounds should cause a WasmTrap error
        match &outcome.returns {
            Err(FunctionCallError::WasmTrap(errors::WasmTrap::MemoryOutOfBounds)) => {
                // Expected - the trap was properly caught and converted to an error
            }
            other => panic!("Expected WasmTrap::MemoryOutOfBounds error, got: {other:?}"),
        }
    }

    /// Test that stack overflow causes a WasmTrap error (not a crash)
    #[test]
    fn test_wasm_stack_overflow() {
        // A WASM module with infinite recursion to cause stack overflow
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func $recurse
                    (call $recurse)
                )
                (func (export "overflow_func")
                    (call $recurse)
                )
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "overflow_func",
                &[],
                &mut storage,
                None, // No private storage for tests
                None,
            )
            .expect("Failed to run module");

        // Stack overflow should cause a WasmTrap error
        match &outcome.returns {
            Err(FunctionCallError::WasmTrap(errors::WasmTrap::StackOverflow)) => {
                // Expected - the trap was properly caught and converted to an error
            }
            other => panic!("Expected WasmTrap::StackOverflow error, got: {other:?}"),
        }
    }

    /// Test that module size limit is enforced during compilation
    #[test]
    fn test_wasm_module_size_limit() {
        use crate::logic::VMLimits;

        // A minimal WASM module
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        // Create an engine with a very small module size limit (smaller than our module)
        let limits = VMLimits {
            max_module_size: 10, // 10 bytes - way too small for any valid module
            ..Default::default()
        };

        let engine = Engine::new(wasmer::Engine::default(), limits);

        // Attempt to compile should fail due to size limit
        let result = engine.compile(&wasm);

        match result {
            Err(FunctionCallError::ModuleSizeLimitExceeded { size, max }) => {
                assert_eq!(max, 10);
                assert!(size > 10, "Module size should be greater than the limit");
            }
            Ok(_) => panic!("Expected ModuleSizeLimitExceeded error, but compilation succeeded"),
            Err(other) => panic!("Expected ModuleSizeLimitExceeded error, got: {other:?}"),
        }
    }

    /// Test that modules within size limit compile successfully
    #[test]
    fn test_wasm_module_within_size_limit() {
        use crate::logic::VMLimits;

        // A minimal WASM module
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        // Create an engine with a large enough module size limit
        let limits = VMLimits {
            max_module_size: 1024 * 1024, // 1 MiB - plenty of room
            ..Default::default()
        };

        let engine = Engine::new(wasmer::Engine::default(), limits);

        // Compilation should succeed
        let result = engine.compile(&wasm);
        assert!(
            result.is_ok(),
            "Expected successful compilation, got: {result:?}"
        );
    }

    /// A guest that imports its linear memory from the host is rejected: guests
    /// must define and export their own memory, never alias a host-supplied one.
    #[test]
    fn guest_importing_memory_is_rejected() {
        let wat = r#"
            (module
                (import "env" "memory" (memory 1))
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::new(wasmer::Engine::default(), VMLimits::default());

        match engine.compile(&wasm) {
            Err(FunctionCallError::ModuleValidationError { reason }) => {
                assert!(reason.contains("memory"), "unexpected reason: {reason}");
            }
            other => panic!("expected ModuleValidationError for imported memory, got: {other:?}"),
        }
    }

    /// A guest exporting a WASI-style `_start` entry point is rejected: Calimero
    /// guests are libraries invoked by explicit method name, not WASI commands.
    #[test]
    fn guest_exporting_start_is_rejected() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "_start"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::new(wasmer::Engine::default(), VMLimits::default());

        match engine.compile(&wasm) {
            Err(FunctionCallError::ModuleValidationError { reason }) => {
                assert!(reason.contains("_start"), "unexpected reason: {reason}");
            }
            other => panic!("expected ModuleValidationError for `_start`, got: {other:?}"),
        }
    }

    /// A guest that exports no linear memory named `memory` is rejected with a
    /// clear validation error rather than failing later at instantiation.
    #[test]
    fn guest_without_memory_export_is_rejected() {
        let wat = r#"
            (module
                (func (export "app_method"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::new(wasmer::Engine::default(), VMLimits::default());

        match engine.compile(&wasm) {
            Err(FunctionCallError::ModuleValidationError { reason }) => {
                assert!(reason.contains("memory"), "unexpected reason: {reason}");
            }
            other => panic!("expected ModuleValidationError for missing memory, got: {other:?}"),
        }
    }

    /// A conforming guest — exports its own memory, no `_start`, no imported
    /// memory — passes validation and compiles.
    #[test]
    fn conforming_guest_passes_validation() {
        let wat = r#"
            (module
                (import "env" "some_host_fn" (func))
                (memory (export "memory") 1)
                (func (export "app_method"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::new(wasmer::Engine::default(), VMLimits::default());

        assert!(
            engine.compile(&wasm).is_ok(),
            "a conforming guest module must pass validation"
        );
    }

    /// The default limits satisfy the registers invariant, and violating it is
    /// caught loudly at engine construction rather than at execution time.
    #[test]
    fn vmlimits_default_satisfies_registers_invariant() {
        VMLimits::default()
            .validate_invariants()
            .expect("default limits must be valid");
    }

    #[test]
    #[should_panic(expected = "max_registers_capacity")]
    fn engine_rejects_registers_capacity_below_register_size() {
        // A total register budget smaller than a single register's cap is
        // self-contradictory; Engine::new must refuse it up front.
        let limits = VMLimits {
            max_registers_capacity: 1,
            ..Default::default()
        };
        let _ = Engine::new(wasmer::Engine::default(), limits);
    }

    /// Test that modules exactly at the size limit compile successfully (boundary condition)
    #[test]
    fn test_wasm_module_at_exact_size_limit() {
        use crate::logic::VMLimits;

        // A minimal WASM module
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        // Create an engine with module size limit exactly equal to the WASM size
        let limits = VMLimits {
            max_module_size: wasm.len() as u64, // Exact size limit
            ..Default::default()
        };

        let engine = Engine::new(wasmer::Engine::default(), limits);

        // Compilation should succeed because check is `size > limit`, not `size >= limit`
        let result = engine.compile(&wasm);
        assert!(
            result.is_ok(),
            "Expected successful compilation at exact size limit, got: {result:?}"
        );
    }

    /// Test that compilation errors are properly wrapped after passing size check
    #[test]
    fn test_wasm_compilation_error_propagation() {
        // Invalid WASM bytes that pass size check but fail compilation
        // This is not valid WASM but is large enough to pass typical size limits
        let invalid_wasm = b"not valid wasm bytes at all - this should fail compilation";

        let engine = Engine::default();

        // Attempt to compile should fail with CompilationError (not size limit)
        let result = engine.compile(invalid_wasm);

        match result {
            Err(FunctionCallError::CompilationError { .. }) => {
                // Expected - wasmer compilation error is properly wrapped
            }
            Err(FunctionCallError::ModuleSizeLimitExceeded { .. }) => {
                panic!("Should not hit size limit for small invalid module")
            }
            Ok(_) => panic!("Expected compilation error for invalid WASM"),
            Err(other) => panic!("Expected CompilationError, got: {other:?}"),
        }
    }

    /// Test edge case where max_module_size is set to 0
    #[test]
    fn test_wasm_module_size_limit_zero() {
        use crate::logic::VMLimits;

        // A minimal WASM module (non-empty)
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        // Create an engine with max_module_size = 0
        let limits = VMLimits {
            max_module_size: 0,
            ..Default::default()
        };

        let engine = Engine::new(wasmer::Engine::default(), limits);

        // Any non-empty module should be rejected
        let result = engine.compile(&wasm);

        match result {
            Err(FunctionCallError::ModuleSizeLimitExceeded { size, max }) => {
                assert_eq!(max, 0);
                assert!(size > 0, "Module size should be greater than 0");
            }
            Ok(_) => panic!("Expected ModuleSizeLimitExceeded error with max_module_size=0"),
            Err(other) => panic!("Expected ModuleSizeLimitExceeded error, got: {other:?}"),
        }

        // Empty byte slice should pass size check (0 > 0 is false) but fail compilation
        let empty_bytes: &[u8] = &[];
        let empty_result = engine.compile(empty_bytes);

        match empty_result {
            Err(FunctionCallError::CompilationError { .. }) => {
                // Expected - empty bytes pass size check but fail compilation
            }
            Err(FunctionCallError::ModuleSizeLimitExceeded { .. }) => {
                panic!("Empty module should pass size check (0 is not > 0)")
            }
            Ok(_) => panic!("Empty bytes should not compile successfully"),
            Err(other) => panic!("Expected CompilationError for empty bytes, got: {other:?}"),
        }
    }

    /// A precompiled artifact this engine produced round-trips back through
    /// `from_precompiled` when it is within the configured cap.
    #[test]
    fn test_from_precompiled_round_trips_within_cap() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");
        let precompiled = module.to_bytes().expect("Failed to serialize module");

        // SAFETY: bytes were just produced by this same engine via `to_bytes`.
        let restored = unsafe { engine.from_precompiled(&precompiled) };
        assert!(
            restored.is_ok(),
            "Expected precompiled round-trip to succeed, got: {restored:?}"
        );
    }

    /// `from_precompiled` enforces `max_precompiled_module_size` before handing
    /// the bytes to wasmer, independent of the source-WASM `max_module_size`.
    #[test]
    fn test_from_precompiled_size_limit_exceeded() {
        use crate::logic::VMLimits;

        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "test_func"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        // Compile with a generous engine, then serialize.
        let precompiled = Engine::default()
            .compile(&wasm)
            .expect("Failed to compile module")
            .to_bytes()
            .expect("Failed to serialize module");

        // A second engine whose precompiled cap is smaller than the artifact.
        let limits = VMLimits {
            max_precompiled_module_size: 1, // 1 byte - far below any artifact
            ..Default::default()
        };
        let engine = Engine::new(wasmer::Engine::default(), limits);

        // SAFETY: bytes came from a trusted compile above; we are exercising the
        // size cap, which must reject before deserialization is attempted.
        let result = unsafe { engine.from_precompiled(&precompiled) };
        match result {
            Err(PrecompiledModuleError::SizeLimitExceeded { size, max }) => {
                assert_eq!(max, 1);
                assert!(size > 1, "artifact should be larger than the cap");
            }
            Ok(_) => panic!("Expected SizeLimitExceeded, but deserialization succeeded"),
            Err(other) => panic!("Expected SizeLimitExceeded, got: {other:?}"),
        }
    }

    /// Bytes within the cap but not a valid artifact surface as a `Deserialize`
    /// error, not a size error — the cap check is separate from validation.
    #[test]
    fn test_from_precompiled_invalid_bytes_surface_deserialize_error() {
        let engine = Engine::default();
        let garbage = vec![0u8; 1024];

        // SAFETY: test-only bytes; here we assert the error path, not soundness.
        let result = unsafe { engine.from_precompiled(&garbage) };
        match result {
            Err(PrecompiledModuleError::Deserialize(_)) => {
                // Expected - within cap, but wasmer rejects the bytes.
            }
            Err(PrecompiledModuleError::SizeLimitExceeded { .. }) => {
                panic!("1 KiB is within the default cap; should not be a size error")
            }
            Ok(_) => panic!("garbage bytes should not deserialize into a module"),
        }
    }

    /// Edge case mirroring `test_wasm_module_size_limit_zero` for the
    /// precompiled path: with `max_precompiled_module_size = 0`, any non-empty
    /// slice is rejected by the cap, while an empty slice passes the size check
    /// (`0 > 0` is false) and surfaces as a deserialization error instead.
    #[test]
    fn test_from_precompiled_size_limit_zero() {
        use crate::logic::VMLimits;

        let limits = VMLimits {
            max_precompiled_module_size: 0,
            ..Default::default()
        };
        let engine = Engine::new(wasmer::Engine::default(), limits);

        // Any non-empty slice is rejected by the cap before reaching wasmer.
        // SAFETY: test-only bytes; the cap rejects before deserialization.
        let non_empty = unsafe { engine.from_precompiled(&[0u8; 8]) };
        match non_empty {
            Err(PrecompiledModuleError::SizeLimitExceeded { size, max }) => {
                assert_eq!(max, 0);
                assert!(size > 0, "non-empty slice should exceed a cap of 0");
            }
            Ok(_) => panic!("Expected SizeLimitExceeded with max_precompiled_module_size=0"),
            Err(other) => panic!("Expected SizeLimitExceeded, got: {other:?}"),
        }

        // An empty slice passes the size check (0 is not > 0) and fails in
        // wasmer's deserialize instead.
        // SAFETY: test-only bytes; asserting the error path, not soundness.
        let empty = unsafe { engine.from_precompiled(&[]) };
        match empty {
            Err(PrecompiledModuleError::Deserialize(_)) => {
                // Expected - empty slice passes the cap but cannot deserialize.
            }
            Err(PrecompiledModuleError::SizeLimitExceeded { .. }) => {
                panic!("empty slice should pass the size check (0 is not > 0)")
            }
            Ok(_) => panic!("empty slice should not deserialize into a module"),
        }
    }
}

/// Tests for WASM gas metering: the bound that stops an untrusted guest from
/// running unbounded computation (a tight loop that calls no host function and
/// so escapes every other limit) and pinning a node thread forever.
#[cfg(test)]
mod gas_metering_tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::logic::VMLimits;
    use crate::store::InMemoryStorage;

    /// An engine whose executions are capped at `max_gas` points, everything
    /// else default. Uses [`Engine::with_limits`] — the constructor that builds
    /// a metered compiler engine — not [`Engine::new`], which would take a
    /// pre-built engine and silently run unmetered. Fresh per call, so the
    /// metering middleware (one module per engine) is never reused.
    fn engine_with_gas(max_gas: u64) -> Engine {
        Engine::with_limits(VMLimits {
            max_gas,
            ..Default::default()
        })
    }

    /// How a run terminated, reduced to something `Send` so it can cross the
    /// watchdog thread boundary (a raw `Outcome` need not be `Send`).
    #[derive(Debug, PartialEq, Eq)]
    enum Verdict {
        Ok,
        GasExhausted,
        OtherError(String),
    }

    fn classify(outcome: &Outcome) -> Verdict {
        match &outcome.returns {
            Ok(_) => Verdict::Ok,
            Err(FunctionCallError::GasExhausted { .. }) => Verdict::GasExhausted,
            Err(other) => Verdict::OtherError(format!("{other:?}")),
        }
    }

    /// Compile `wat` and run `method` under a `max_gas` budget, on a watchdog
    /// thread. If the run does not finish within `timeout` the test fails loudly
    /// instead of hanging CI — which is exactly the failure mode (an unbounded
    /// guest loop) these tests exist to prevent, so a regression that defeats
    /// metering surfaces as a fast, clear failure rather than a stuck job.
    fn run_bounded(wat: &str, method: &'static str, max_gas: u64, timeout: Duration) -> Verdict {
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");
        let (tx, rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            let engine = engine_with_gas(max_gas);
            let module = engine.compile(&wasm).expect("Failed to compile module");
            let mut storage = InMemoryStorage::default();
            let outcome = module
                .run(
                    [0; 32].into(),
                    [0; 32].into(),
                    method,
                    &[],
                    &mut storage,
                    None,
                    None,
                )
                .expect("run must return an Outcome");
            // Ignore send errors: if the receiver already timed out, the test
            // has failed and this thread is being abandoned.
            let _ = tx.send(classify(&outcome));
        });

        match rx.recv_timeout(timeout) {
            Ok(verdict) => {
                handle.join().expect("watchdog thread panicked");
                verdict
            }
            Err(mpsc::RecvTimeoutError::Timeout) => panic!(
                "execution of `{method}` was not bounded by gas within {timeout:?}: metering \
                 failed to trap the guest — this is the node-thread-pinning DoS the meter prevents"
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("watchdog thread died without producing a verdict")
            }
        }
    }

    /// A busy-loop counting down from `local.get`-loaded iterations. Enough
    /// operators per iteration to burn gas predictably; no host calls, so gas is
    /// the only thing that can stop it.
    const COUNTDOWN_WAT: &str = r#"
        (module
            (memory (export "memory") 1)
            (func (export "spin")
                (local $i i32)
                (local.set $i (i32.const 100000))
                (block $exit
                    (loop $again
                        (br_if $exit (i32.eqz (local.get $i)))
                        (local.set $i (i32.sub (local.get $i) (i32.const 1)))
                        (br $again)
                    )
                )
            )
        )
    "#;

    /// The core regression test: an *infinite* loop that calls no host function
    /// is trapped by gas rather than pinning the thread forever.
    #[test]
    fn infinite_loop_is_trapped_by_gas() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "spin_forever")
                    (loop $again (br $again))
                )
            )
        "#;
        // Small budget so exhaustion is near-instant; watchdog is generous.
        let verdict = run_bounded(wat, "spin_forever", 100_000, Duration::from_secs(30));
        assert_eq!(
            verdict,
            Verdict::GasExhausted,
            "an infinite guest loop must trap with GasExhausted"
        );
    }

    /// A bounded computation succeeds when the budget covers it and traps with
    /// `GasExhausted` when it does not — the meter enforces the configured limit
    /// rather than being all-or-nothing.
    #[test]
    fn budget_gates_a_bounded_computation() {
        // A million-iteration countdown needs well over a million points.
        let tight = run_bounded(COUNTDOWN_WAT, "spin", 100_000, Duration::from_secs(30));
        assert_eq!(
            tight,
            Verdict::GasExhausted,
            "a budget far below the work required must be exhausted"
        );

        let generous = run_bounded(
            COUNTDOWN_WAT,
            "spin",
            1_000_000_000,
            Duration::from_secs(30),
        );
        assert_eq!(
            generous,
            Verdict::Ok,
            "a budget well above the work required must let the run complete"
        );
    }

    /// Gas exhaustion is reported as `GasExhausted`, distinct from a guest that
    /// genuinely executes `unreachable`: the latter, given ample gas, must still
    /// surface as `WasmTrap::Unreachable` so the two causes are never conflated.
    #[test]
    fn genuine_unreachable_is_not_reported_as_gas_exhaustion() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "boom") unreachable)
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");
        let module = engine_with_gas(1_000_000_000)
            .compile(&wasm)
            .expect("Failed to compile module");
        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "boom",
                &[],
                &mut storage,
                None,
                None,
            )
            .expect("Failed to run module");

        match &outcome.returns {
            Err(FunctionCallError::WasmTrap(errors::WasmTrap::Unreachable)) => {}
            other => panic!("expected WasmTrap::Unreachable, got: {other:?}"),
        }
    }

    /// A normal, cheap method runs to completion under the default budget —
    /// metering must not perturb ordinary execution.
    #[test]
    fn cheap_method_succeeds_under_default_budget() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "noop"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");
        let module = Engine::default()
            .compile(&wasm)
            .expect("Failed to compile module");
        let mut storage = InMemoryStorage::default();
        let outcome = module
            .run(
                [0; 32].into(),
                [0; 32].into(),
                "noop",
                &[],
                &mut storage,
                None,
                None,
            )
            .expect("Failed to run module");
        assert!(
            outcome.returns.is_ok(),
            "a trivial method must succeed under the default gas budget, got: {:?}",
            outcome.returns
        );
    }

    /// Every module the engine compiles is instrumented: the metering globals
    /// the middleware injects are present on the instantiated module. This is
    /// the invariant the runtime relies on when it calls `set_gas_limit` /
    /// `is_exhausted`.
    #[test]
    fn compiled_module_carries_metering_globals() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "noop"))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");
        let engine = Engine::default();
        let module = engine.compile(&wasm).expect("Failed to compile module");

        let mut store = Store::new(module.engine.clone());
        let imports = wasmer::Imports::new();
        let instance =
            Instance::new(&mut store, &module.module, &imports).expect("instantiation failed");

        assert!(
            crate::metering::is_metered(&store, &instance),
            "a module compiled by the runtime engine must carry the metering globals"
        );
    }

    /// The gas charge for a fixed computation is reproducible across independent
    /// compiles: the exact budget at which a run flips from success to
    /// exhaustion is identical for two freshly built engines. Reproducibility is
    /// the property that makes gas safe for a replicated state machine — every
    /// node must charge the same or their outcomes fork.
    #[test]
    fn gas_charge_is_deterministic_across_compiles() {
        // Find the minimal budget at which `spin` completes, via exponential
        // growth then binary search. The threshold is a pure function of the
        // module and the cost model, so it must be stable.
        fn min_gas_to_complete() -> u64 {
            let ok = |gas: u64| {
                run_bounded(COUNTDOWN_WAT, "spin", gas, Duration::from_secs(30)) == Verdict::Ok
            };
            // Exponential search for an upper bound that succeeds.
            let mut hi = 1_000_000u64;
            while !ok(hi) {
                hi = hi.checked_mul(2).expect("threshold search overflowed");
            }
            let mut lo = hi / 2; // known to fail (or 500k, safely below the ~1M+ needed)
                                 // Binary search for the boundary.
            while lo + 1 < hi {
                let mid = lo + (hi - lo) / 2;
                if ok(mid) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            hi
        }

        let first = min_gas_to_complete();
        let second = min_gas_to_complete();
        assert_eq!(
            first, second,
            "the gas threshold for a fixed computation must be identical across compiles \
             (first={first}, second={second}) — divergence would fork replicated state"
        );
    }

    /// Gas metering survives serialization: a module serialized and restored via
    /// the precompiled path (headless engine, no compiler) still traps an
    /// infinite loop. This guards the claim that the metering counter is baked
    /// into the artifact, not reconstructed at compile time.
    #[test]
    fn metering_survives_serialize_round_trip() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "spin_forever")
                    (loop $again (br $again))
                )
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let limits = VMLimits {
            max_gas: 100_000,
            ..Default::default()
        };
        let compiling = Engine::with_limits(limits);
        let module = compiling.compile(&wasm).expect("Failed to compile module");
        let serialized = module.to_bytes().expect("Failed to serialize module");

        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let headless = Engine::headless_with_limits(limits);
            // SAFETY: bytes were just produced by `to_bytes` in this test.
            let restored = unsafe { headless.from_precompiled(&serialized) }
                .expect("Failed to restore precompiled module");
            let mut storage = InMemoryStorage::default();
            let outcome = restored
                .run(
                    [0; 32].into(),
                    [0; 32].into(),
                    "spin_forever",
                    &[],
                    &mut storage,
                    None,
                    None,
                )
                .expect("run must return an Outcome");
            let _ = tx.send(classify(&outcome));
        });

        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(verdict) => {
                handle.join().expect("watchdog thread panicked");
                assert_eq!(
                    verdict,
                    Verdict::GasExhausted,
                    "a deserialized (precompiled) module must still be gas-metered"
                );
            }
            Err(_) => panic!(
                "deserialized module's infinite loop was not bounded by gas: metering did not \
                 survive serialization"
            ),
        }
    }

    /// Gas exhausted inside the optional `__calimero_register_merge` hook (which
    /// runs before the method and whose own errors are non-fatal) still fails
    /// the overall execution: the method call that follows immediately re-traps
    /// on the drained budget and surfaces `GasExhausted`, rather than the run
    /// silently proceeding as if nothing happened.
    #[test]
    fn gas_exhausted_in_register_hook_fails_execution() {
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "__calimero_register_merge")
                    (loop $again (br $again))
                )
                (func (export "app_method"))
            )
        "#;
        let verdict = run_bounded(wat, "app_method", 100_000, Duration::from_secs(30));
        assert_eq!(
            verdict,
            Verdict::GasExhausted,
            "exhausting gas in the merge-registration hook must fail the execution"
        );
    }
}
