use std::panic::{catch_unwind, AssertUnwindSafe};

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tracing::{debug, error, info};
use wasmer::{Instance, SerializeError, Store};

// Profiling feature: Only compile these imports when profiling feature is enabled
#[cfg(feature = "profiling")]
use wasmer::sys::{CompilerConfig, Cranelift};

pub mod config;
mod constants;
mod constraint;
pub mod errors;
pub mod logic;
mod memory;
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
    #[must_use]
    pub fn new(mut engine: wasmer::Engine, limits: VMLimits) -> Self {
        // A self-contradictory limits config (e.g. a total register budget
        // smaller than a single register's cap) is a construction-time
        // programming error, not a runtime condition, and is never reachable
        // from guest input. Fail loudly here, at engine construction — once, at
        // startup — instead of letting it misbehave on every execution.
        limits
            .validate_invariants()
            .expect("invalid VMLimits passed to Engine::new");

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

    /// Like [`Engine::default`], but with operator-configured `limits` instead
    /// of [`VMLimits::default`]. The limits are baked into every `Module` this
    /// engine compiles and applied at execution time.
    #[must_use]
    pub fn with_limits(limits: VMLimits) -> Self {
        let engine = Self::create_engine();

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
    /// Two checks:
    ///
    /// * **No imported memory.** Every guest must define and *export* its own
    ///   linear memory named `memory` — the host reads guest state through
    ///   `instance.exports.get_memory("memory")`. A module that instead
    ///   *imports* its memory expects the host to hand it one, which the runtime
    ///   never provides; such a module could only be attempting to alias
    ///   host-supplied memory, so it is rejected outright rather than failing
    ///   deeper in instantiation.
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

        if module.exports().any(|export| export.name() == "_start") {
            return Err(FunctionCallError::ModuleValidationError {
                reason: "guest exports a `_start` function; WASI command entry points are \
                         not supported (guests are invoked by explicit method name)"
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
    fn execute_wasm(
        store: &mut Store,
        module: &wasmer::Module,
        imports: &wasmer::Imports,
        logic: &mut VMLogic<'_>,
        method: &str,
        context_id: &ContextId,
        max_method_name_length: u64,
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
