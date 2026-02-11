use std::panic::{catch_unwind, AssertUnwindSafe};

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tracing::{debug, error, info};
use wasmer::{DeserializeError, Instance, SerializeError, Store};

// Profiling feature: Only compile these imports when profiling feature is enabled
#[cfg(feature = "profiling")]
use wasmer::sys::{CompilerConfig, Cranelift};

mod constants;
mod constraint;
pub mod errors;
pub mod logic;
mod memory;
mod panic_payload;
pub mod store;

pub use constraint::Constraint;
use errors::{FunctionCallError, HostError, Location, PanicContext, VMRuntimeError};
use logic::{ContextHost, Outcome, VMContext, VMLimits, VMLogic, VMLogicError};
use memory::WasmerTunables;
use store::Storage;

pub type RuntimeResult<T, E = VMRuntimeError> = Result<T, E>;

// Re-export RuntimeMergeCallback for use by the sync system
// Note: The callback is defined at the end of this file

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

    /// # Safety
    ///
    /// This function deserializes a precompiled WASM module. The caller must ensure
    /// the bytes come from a trusted source (e.g., previously compiled by this engine).
    ///
    /// # Security Note
    ///
    /// No size limit check is performed here. This is an accepted security trade-off because:
    /// 1. Precompiled modules have already been validated during their original compilation
    /// 2. The serialized format may differ significantly in size from the original WASM binary
    /// 3. The `unsafe` marker already requires callers to ensure the bytes are from a trusted source
    ///
    /// **Audit requirement**: All call sites using this method should be reviewed to ensure
    /// precompiled bytes originate from trusted sources only (e.g., the node's own compilation cache).
    ///
    /// If precompiled bytes could come from an untrusted source, callers should implement
    /// their own size validation before calling this method.
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

    pub fn run<'a>(
        &'a self,
        context: ContextId,
        executor: PublicKey,
        method: &str,
        input: &'a [u8],
        storage: &'a mut dyn Storage,
        private_storage: Option<&'a mut dyn Storage>,
        node_client: Option<NodeClient>,
        context_host: Option<Box<dyn ContextHost>>,
    ) -> RuntimeResult<Outcome> {
        let context_id = context;
        info!(%context_id, method, "Running WASM method");
        debug!(%context_id, method, input_len = input.len(), "WASM execution input");

        let context = VMContext::new(input.into(), *context_id, *executor);

        let mut logic = VMLogic::new(
            storage,
            private_storage,
            context,
            &self.limits,
            node_client,
            context_host,
        );

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

    /// Check if this module has the `__calimero_merge_root_state` export.
    ///
    /// Returns `true` if the module exports the merge function, indicating
    /// that it can participate in CRDT merge during sync.
    #[must_use]
    pub fn has_merge_export(&self) -> bool {
        self.module
            .exports()
            .any(|e| e.name() == "__calimero_merge_root_state")
    }

    /// Check if this module has the `__calimero_merge` export for custom types.
    #[must_use]
    pub fn has_custom_merge_export(&self) -> bool {
        self.module
            .exports()
            .any(|e| e.name() == "__calimero_merge")
    }
}

// ════════════════════════════════════════════════════════════════════════════
// WASM Merge Callback Implementation
// ════════════════════════════════════════════════════════════════════════════

use calimero_storage::{WasmMergeCallback, WasmMergeError};
use std::time::Duration;

/// Default timeout for WASM merge operations (5 seconds).
pub const DEFAULT_MERGE_TIMEOUT: Duration = Duration::from_secs(5);

/// WASM merge callback that calls into a compiled WASM module.
///
/// This callback implements `WasmMergeCallback` and can be used during state
/// synchronization to merge custom types and root state via WASM exports.
///
/// # Example
///
/// ```ignore
/// let engine = Engine::default();
/// let module = engine.compile(&wasm_bytes)?;
///
/// if let Some(callback) = RuntimeMergeCallback::from_module(&module) {
///     // Module has merge exports, can use for sync
///     let merged = callback.merge_root_state(local_data, remote_data)?;
/// }
/// ```
pub struct RuntimeMergeCallback {
    engine: wasmer::Engine,
    module: wasmer::Module,
    timeout: Duration,
}

impl RuntimeMergeCallback {
    /// Create a merge callback from a compiled module.
    ///
    /// Returns `Some` if the module has at least one merge export
    /// (`__calimero_merge_root_state` or `__calimero_merge`).
    /// Returns `None` if neither export exists.
    #[must_use]
    pub fn from_module(module: &Module) -> Option<Self> {
        if module.has_merge_export() || module.has_custom_merge_export() {
            Some(Self {
                engine: module.engine.clone(),
                module: module.module.clone(),
                timeout: DEFAULT_MERGE_TIMEOUT,
            })
        } else {
            None
        }
    }

    /// Create a merge callback with a custom timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Call the WASM merge function and extract the result.
    ///
    /// This handles:
    /// 1. Creating a WASM instance
    /// 2. Copying data to WASM memory
    /// 3. Calling the merge function
    /// 4. Extracting and deserializing the result
    fn call_merge_function(
        &self,
        export_name: &str,
        local_data: &[u8],
        remote_data: &[u8],
    ) -> Result<Vec<u8>, WasmMergeError> {
        let mut store = Store::new(self.engine.clone());

        // Create instance with empty imports (merge functions don't need host functions)
        let imports = wasmer::Imports::new();
        let instance = Instance::new(&mut store, &self.module, &imports)
            .map_err(|e| WasmMergeError::MergeFailed(format!("Instance creation failed: {e}")))?;

        // Get memory export
        let memory = instance
            .exports
            .get_memory("memory")
            .map_err(|e| WasmMergeError::MergeFailed(format!("Memory not found: {e}")))?;

        // Allocate memory for input data
        // Layout: [local_data][remote_data]
        let local_ptr = 1024u64; // Start after some buffer for stack
        let remote_ptr = local_ptr + local_data.len() as u64;
        let total_needed = remote_ptr + remote_data.len() as u64;

        // Ensure memory is large enough (grow if needed)
        let current_pages = memory.view(&store).size().0 as u64;
        let pages_needed = (total_needed / 65536) + 1;
        if pages_needed > current_pages {
            let _ = memory
                .grow(&mut store, (pages_needed - current_pages) as u32)
                .map_err(|e| WasmMergeError::MergeFailed(format!("Memory grow failed: {e}")))?;
        }

        // Copy data to WASM memory
        let view = memory.view(&store);
        view.write(local_ptr, local_data)
            .map_err(|e| WasmMergeError::MergeFailed(format!("Write local failed: {e}")))?;
        view.write(remote_ptr, remote_data)
            .map_err(|e| WasmMergeError::MergeFailed(format!("Write remote failed: {e}")))?;

        // Get and call the merge function
        let merge_fn = instance
            .exports
            .get_typed_function::<(u64, u64, u64, u64), u64>(&store, export_name)
            .map_err(|_| WasmMergeError::ExportNotFound(export_name.to_owned()))?;

        // TODO: Add timeout support using tokio::time::timeout in async context
        // For now, we call synchronously (timeout is a future enhancement)
        let result = merge_fn
            .call(
                &mut store,
                local_ptr,
                local_data.len() as u64,
                remote_ptr,
                remote_data.len() as u64,
            )
            .map_err(|e| WasmMergeError::MergeFailed(format!("WASM call failed: {e}")))?;

        // Unpack result (high 32 bits = ptr, low 32 bits = len)
        let result_ptr = result >> 32;
        let result_len = (result & 0xFFFF_FFFF) as usize;

        if result_len == 0 {
            return Err(WasmMergeError::MergeFailed(
                "Empty result from WASM".to_owned(),
            ));
        }

        // Read result from WASM memory
        let view = memory.view(&store);
        let mut result_bytes = vec![0u8; result_len];
        view.read(result_ptr, &mut result_bytes)
            .map_err(|e| WasmMergeError::MergeFailed(format!("Read result failed: {e}")))?;

        // Parse Borsh-encoded result
        Self::parse_merge_result(result_bytes)
    }
}

impl RuntimeMergeCallback {
    /// Call the `__calimero_merge` WASM export for custom type merging.
    ///
    /// This function handles the additional type_name parameter compared to root state merge.
    fn call_custom_merge_function(
        &self,
        type_name: &str,
        local_data: &[u8],
        remote_data: &[u8],
    ) -> Result<Vec<u8>, WasmMergeError> {
        let mut store = Store::new(self.engine.clone());

        // Create instance with empty imports (merge functions don't need host functions)
        let imports = wasmer::Imports::new();
        let instance = Instance::new(&mut store, &self.module, &imports)
            .map_err(|e| WasmMergeError::MergeFailed(format!("Instance creation failed: {e}")))?;

        // Call the registration hook to populate the merge registry.
        // This is required before calling __calimero_merge, which looks up
        // merge functions by type name in the registry.
        if let Ok(register_fn) = instance
            .exports
            .get_typed_function::<(), ()>(&store, "__calimero_register_merge")
        {
            register_fn.call(&mut store).map_err(|e| {
                WasmMergeError::MergeFailed(format!("Registration hook failed: {e}"))
            })?;
        }

        // Get memory export
        let memory = instance
            .exports
            .get_memory("memory")
            .map_err(|e| WasmMergeError::MergeFailed(format!("Memory not found: {e}")))?;

        // Allocate memory for input data
        // Layout: [type_name][local_data][remote_data]
        let type_name_bytes = type_name.as_bytes();
        let type_name_ptr = 1024u64; // Start after some buffer for stack
        let local_ptr = type_name_ptr + type_name_bytes.len() as u64;
        let remote_ptr = local_ptr + local_data.len() as u64;
        let total_needed = remote_ptr + remote_data.len() as u64;

        // Ensure memory is large enough (grow if needed)
        let current_pages = memory.view(&store).size().0 as u64;
        let pages_needed = (total_needed / 65536) + 1;
        if pages_needed > current_pages {
            let _ = memory
                .grow(&mut store, (pages_needed - current_pages) as u32)
                .map_err(|e| WasmMergeError::MergeFailed(format!("Memory grow failed: {e}")))?;
        }

        // Copy data to WASM memory
        let view = memory.view(&store);
        view.write(type_name_ptr, type_name_bytes)
            .map_err(|e| WasmMergeError::MergeFailed(format!("Write type_name failed: {e}")))?;
        view.write(local_ptr, local_data)
            .map_err(|e| WasmMergeError::MergeFailed(format!("Write local failed: {e}")))?;
        view.write(remote_ptr, remote_data)
            .map_err(|e| WasmMergeError::MergeFailed(format!("Write remote failed: {e}")))?;

        // Get and call the merge function
        // Signature: (type_name_ptr, type_name_len, local_ptr, local_len, remote_ptr, remote_len) -> u64
        let merge_fn = instance
            .exports
            .get_typed_function::<(u64, u64, u64, u64, u64, u64), u64>(&store, "__calimero_merge")
            .map_err(|_| WasmMergeError::ExportNotFound("__calimero_merge".to_owned()))?;

        let result = merge_fn
            .call(
                &mut store,
                type_name_ptr,
                type_name_bytes.len() as u64,
                local_ptr,
                local_data.len() as u64,
                remote_ptr,
                remote_data.len() as u64,
            )
            .map_err(|e| WasmMergeError::MergeFailed(format!("WASM call failed: {e}")))?;

        // Unpack result (high 32 bits = ptr, low 32 bits = len)
        let result_ptr = result >> 32;
        let result_len = (result & 0xFFFF_FFFF) as usize;

        if result_len == 0 {
            return Err(WasmMergeError::MergeFailed(
                "Empty result from WASM".to_owned(),
            ));
        }

        // Read result from WASM memory
        let view = memory.view(&store);
        let mut result_bytes = vec![0u8; result_len];
        view.read(result_ptr, &mut result_bytes)
            .map_err(|e| WasmMergeError::MergeFailed(format!("Read result failed: {e}")))?;

        // Parse Borsh-encoded result (same as call_merge_function)
        Self::parse_merge_result(result_bytes)
    }

    /// Parse the Borsh-encoded MergeResultInternal from WASM.
    ///
    /// Format: variant byte (0=Success, 1=Error) + length (4 bytes) + data
    fn parse_merge_result(result_bytes: Vec<u8>) -> Result<Vec<u8>, WasmMergeError> {
        if result_bytes.is_empty() {
            return Err(WasmMergeError::MergeFailed("Empty result bytes".to_owned()));
        }

        match result_bytes[0] {
            0 => {
                // Success: next 4 bytes are length, then data
                if result_bytes.len() < 5 {
                    return Err(WasmMergeError::MergeFailed(
                        "Invalid success result format".to_owned(),
                    ));
                }
                let data_len = u32::from_le_bytes([
                    result_bytes[1],
                    result_bytes[2],
                    result_bytes[3],
                    result_bytes[4],
                ]) as usize;
                if result_bytes.len() < 5 + data_len {
                    return Err(WasmMergeError::MergeFailed(
                        "Truncated success data".to_owned(),
                    ));
                }
                Ok(result_bytes[5..5 + data_len].to_vec())
            }
            1 => {
                // Error: next 4 bytes are length, then error message
                if result_bytes.len() < 5 {
                    return Err(WasmMergeError::MergeFailed(
                        "Invalid error result format".to_owned(),
                    ));
                }
                let msg_len = u32::from_le_bytes([
                    result_bytes[1],
                    result_bytes[2],
                    result_bytes[3],
                    result_bytes[4],
                ]) as usize;
                let msg = if result_bytes.len() >= 5 + msg_len {
                    String::from_utf8_lossy(&result_bytes[5..5 + msg_len]).to_string()
                } else {
                    "Unknown error".to_owned()
                };
                Err(WasmMergeError::MergeFailed(msg))
            }
            variant => Err(WasmMergeError::MergeFailed(format!(
                "Unknown result variant: {variant}"
            ))),
        }
    }
}

impl WasmMergeCallback for RuntimeMergeCallback {
    fn merge_custom(
        &self,
        type_name: &str,
        local_data: &[u8],
        remote_data: &[u8],
        _local_ts: u64,
        _remote_ts: u64,
    ) -> Result<Vec<u8>, WasmMergeError> {
        debug!(
            type_name,
            local_len = local_data.len(),
            remote_len = remote_data.len(),
            "RuntimeMergeCallback::merge_custom called"
        );

        self.call_custom_merge_function(type_name, local_data, remote_data)
    }

    fn merge_root_state(
        &self,
        local_data: &[u8],
        remote_data: &[u8],
    ) -> Result<Vec<u8>, WasmMergeError> {
        debug!(
            local_len = local_data.len(),
            remote_len = remote_data.len(),
            "RuntimeMergeCallback::merge_root_state called"
        );

        self.call_merge_function("__calimero_merge_root_state", local_data, remote_data)
    }
}

// Make RuntimeMergeCallback Send + Sync safe
// The wasmer Module and Engine are designed to be Send + Sync
unsafe impl Send for RuntimeMergeCallback {}
unsafe impl Sync for RuntimeMergeCallback {}

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
        let mut limits = VMLimits::default();
        limits.max_module_size = 10; // 10 bytes - way too small for any valid module

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
        let mut limits = VMLimits::default();
        limits.max_module_size = 1024 * 1024; // 1 MiB - plenty of room

        let engine = Engine::new(wasmer::Engine::default(), limits);

        // Compilation should succeed
        let result = engine.compile(&wasm);
        assert!(
            result.is_ok(),
            "Expected successful compilation, got: {result:?}"
        );
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
        let mut limits = VMLimits::default();
        limits.max_module_size = wasm.len() as u64; // Exact size limit

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
        let mut limits = VMLimits::default();
        limits.max_module_size = 0;

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
}
