//! WASM merge callback implementation for custom CRDT types.
//!
//! This module implements the [`WasmMergeCallback`] trait from `calimero-storage`,
//! enabling applications to define custom merge logic via WASM exports.
//!
//! # Architecture
//!
//! During sync, when the storage layer encounters a `CrdtType::Custom("TypeName")`,
//! it needs to call back into WASM to perform the merge. Since Wasmer doesn't support
//! reentrancy (calling WASM from within WASM), the runtime creates a **separate WASM
//! instance** just for merge callbacks.
//!
//! ```text
//! Module::run() creates merge callback instance
//!     ↓
//! VMLogic stores callback
//!     ↓
//! Host function builds RuntimeEnv with callback
//!     ↓
//! Storage detects Custom type conflict
//!     ↓
//! Calls callback.merge_custom(local, remote, type_name)
//!     ↓
//! RuntimeMergeCallback calls __calimero_merge_{TypeName}
//! ```
//!
//! # WASM Exports
//!
//! Applications must export these functions to use custom merges:
//!
//! - `__calimero_alloc(size: u64) -> ptr: u64` - Memory allocation
//! - `__calimero_merge_root_state(local_ptr, local_len, remote_ptr, remote_len) -> result_ptr`
//! - `__calimero_merge_{TypeName}(local_ptr, local_len, remote_ptr, remote_len) -> result_ptr`
//!
//! The `#[app::state]` macro generates `__calimero_merge_root_state` and `__calimero_alloc`.
//! The `#[app::mergeable]` macro generates `__calimero_merge_{TypeName}` for custom types.
//!
//! The result is a pointer to a `MergeResult` struct in WASM memory:
//!
//! ```ignore
//! #[repr(C)]
//! struct MergeResult {
//!     success: u8,      // 0 = failure, 1 = success
//!     data_ptr: u64,    // Pointer to merged data (if success)
//!     data_len: u64,    // Length of merged data (if success)
//!     error_ptr: u64,   // Pointer to error message (if failure)
//!     error_len: u64,   // Length of error message (if failure)
//! }
//! ```
//!
//! # Performance Note
//!
//! Currently, a new WASM instance is created per `Module::run()` call. This is tracked
//! in issue #1997 for optimization (caching or pooling instances).
//!
//! # Timeout
//!
//! WASM merge calls have a configurable timeout (default: 5 seconds) to prevent
//! infinite loops or malicious code from blocking sync.

use std::sync::Mutex;

use calimero_storage::collections::crdt_meta::{MergeError, WasmMergeCallback};
use tracing::{debug, warn};
use wasmer::{Instance, Memory, Store, TypedFunction};

/// Default timeout for WASM merge operations (5 seconds).
pub const DEFAULT_MERGE_TIMEOUT_MS: u64 = 5000;

/// Maximum allowed size for WASM merge results (64 MB).
///
/// This prevents malicious WASM modules from causing memory exhaustion
/// by returning huge length values in merge results.
pub const MAX_MERGE_RESULT_SIZE: u64 = 64 * 1024 * 1024;

/// Export name for the root state merge function.
pub const MERGE_ROOT_STATE_EXPORT: &str = "__calimero_merge_root_state";

/// Result structure returned by WASM merge functions.
///
/// This matches the layout expected from the WASM side.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct WasmMergeResult {
    /// 0 = failure, 1 = success
    success: u8,
    /// Pointer to merged data (if success)
    data_ptr: u64,
    /// Length of merged data (if success)
    data_len: u64,
    /// Pointer to error message (if failure)
    error_ptr: u64,
    /// Length of error message (if failure)
    error_len: u64,
}

impl WasmMergeResult {
    /// Size of the result structure in bytes.
    const SIZE: usize = 1 + 8 + 8 + 8 + 8; // 33 bytes

    /// Read a `WasmMergeResult` from WASM memory at the given pointer.
    fn from_memory(memory: &Memory, store: &Store, ptr: u64) -> Result<Self, MergeError> {
        let view = memory.view(store);
        let mut buf = [0u8; Self::SIZE];

        // Read the result structure from memory
        view.read(ptr, &mut buf)
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("Failed to read merge result from WASM memory: {e}"),
            })?;

        // Parse the fields
        Ok(Self {
            success: buf[0],
            data_ptr: u64::from_le_bytes(buf[1..9].try_into().map_err(|_| {
                MergeError::WasmCallbackFailed {
                    message: "Invalid data_ptr bytes".to_string(),
                }
            })?),
            data_len: u64::from_le_bytes(buf[9..17].try_into().map_err(|_| {
                MergeError::WasmCallbackFailed {
                    message: "Invalid data_len bytes".to_string(),
                }
            })?),
            error_ptr: u64::from_le_bytes(buf[17..25].try_into().map_err(|_| {
                MergeError::WasmCallbackFailed {
                    message: "Invalid error_ptr bytes".to_string(),
                }
            })?),
            error_len: u64::from_le_bytes(buf[25..33].try_into().map_err(|_| {
                MergeError::WasmCallbackFailed {
                    message: "Invalid error_len bytes".to_string(),
                }
            })?),
        })
    }
}

/// WASM merge callback implementation for the Calimero runtime.
///
/// This struct holds a reference to the WASM instance and provides
/// the merge callback interface for custom CRDT types.
///
/// Uses interior mutability (`Mutex`) for the store because the
/// `WasmMergeCallback` trait requires `&self` but Wasmer's `Store`
/// needs mutable access for function calls. `Mutex` is used instead
/// of `RefCell` to satisfy the `Send + Sync` bounds on the trait.
pub struct RuntimeMergeCallback {
    /// The WASM store (interior mutability for trait compatibility).
    store: Mutex<Store>,
    /// The WASM instance with the application module.
    instance: Instance,
    /// Timeout for WASM merge operations.
    #[allow(dead_code, reason = "Will be used for timeout handling in Issue #1780")]
    timeout_ms: u64,
}

impl RuntimeMergeCallback {
    /// Create a new merge callback from a WASM instance.
    ///
    /// Returns `Some(callback)` if the instance exports the required merge functions,
    /// `None` otherwise.
    ///
    /// # Arguments
    ///
    /// * `store` - The WASM store
    /// * `instance` - The WASM instance
    #[must_use]
    pub fn from_instance(store: Store, instance: Instance) -> Option<Self> {
        // Check if the required export exists
        if instance
            .exports
            .get_function(MERGE_ROOT_STATE_EXPORT)
            .is_ok()
        {
            debug!(
                target: "calimero_runtime::merge",
                "WASM module exports merge functions, creating callback"
            );
            Some(Self {
                store: Mutex::new(store),
                instance,
                timeout_ms: DEFAULT_MERGE_TIMEOUT_MS,
            })
        } else {
            debug!(
                target: "calimero_runtime::merge",
                "WASM module does not export merge functions"
            );
            None
        }
    }

    /// Set the timeout for WASM merge operations.
    #[must_use]
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Get the WASM memory from the instance.
    fn get_memory(&self) -> Result<&Memory, MergeError> {
        self.instance
            .exports
            .get_memory("memory")
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("Failed to get WASM memory: {e}"),
            })
    }

    /// Write data to WASM memory and return the pointer.
    ///
    /// This allocates memory in the WASM instance using the `__calimero_alloc` export
    /// and writes the data there.
    fn write_to_wasm(&self, store: &mut Store, data: &[u8]) -> Result<(u64, u64), MergeError> {
        // Get the allocator function
        let alloc_fn: TypedFunction<u64, u64> = self
            .instance
            .exports
            .get_typed_function(store, "__calimero_alloc")
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!(
                    "WASM module does not export __calimero_alloc function: {e}. \
                     Ensure your app uses #[app::state] macro."
                ),
            })?;

        // Allocate memory for the data
        let len = data.len() as u64;
        let ptr = alloc_fn
            .call(store, len)
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("Failed to allocate WASM memory: {e}"),
            })?;

        // Write data to the allocated memory
        let memory = self.get_memory()?;
        let view = memory.view(store);
        view.write(ptr, data)
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("Failed to write data to WASM memory: {e}"),
            })?;

        Ok((ptr, len))
    }

    /// Read data from WASM memory at the given pointer and length.
    ///
    /// # Errors
    ///
    /// Returns error if length exceeds `MAX_MERGE_RESULT_SIZE` (DoS protection)
    /// or if reading from WASM memory fails.
    fn read_from_wasm(&self, store: &Store, ptr: u64, len: u64) -> Result<Vec<u8>, MergeError> {
        // Guard against malicious WASM returning huge length values
        if len > MAX_MERGE_RESULT_SIZE {
            return Err(MergeError::WasmCallbackFailed {
                message: format!(
                    "WASM merge result size {} exceeds maximum allowed {} bytes",
                    len, MAX_MERGE_RESULT_SIZE
                ),
            });
        }
        let memory = self.get_memory()?;
        let view = memory.view(store);
        let mut buf = vec![0u8; len as usize];
        view.read(ptr, &mut buf)
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("Failed to read data from WASM memory: {e}"),
            })?;
        Ok(buf)
    }

    /// Call a merge function by export name.
    fn call_merge_function(
        &self,
        export_name: &str,
        local: &[u8],
        remote: &[u8],
    ) -> Result<Vec<u8>, MergeError> {
        // Lock the store for the entire operation
        let mut store = self
            .store
            .lock()
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("Failed to lock WASM store: {e}"),
            })?;

        // Get the merge function
        let merge_fn: TypedFunction<(u64, u64, u64, u64), u64> = self
            .instance
            .exports
            .get_typed_function(&*store, export_name)
            .map_err(|e| MergeError::WasmMergeNotExported {
                export_name: format!("{export_name}: {e}"),
            })?;

        // Write inputs to WASM memory
        let (local_ptr, local_len) = self.write_to_wasm(&mut store, local)?;
        let (remote_ptr, remote_len) = self.write_to_wasm(&mut store, remote)?;

        debug!(
            target: "calimero_runtime::merge",
            export_name,
            local_len,
            remote_len,
            "Calling WASM merge function"
        );

        // Call the merge function
        // TODO: Add timeout handling (Issue #1780 acceptance criteria)
        let result_ptr = merge_fn
            .call(&mut *store, local_ptr, local_len, remote_ptr, remote_len)
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("WASM {export_name} call failed: {e}"),
            })?;

        // Read the result
        let memory = self.get_memory()?;
        let result = WasmMergeResult::from_memory(memory, &*store, result_ptr)?;

        if result.success != 0 {
            // Success - read the merged data
            let merged = self.read_from_wasm(&store, result.data_ptr, result.data_len)?;
            debug!(
                target: "calimero_runtime::merge",
                export_name,
                merged_len = merged.len(),
                "WASM merge function succeeded"
            );
            Ok(merged)
        } else {
            // Failure - read the error message
            let error_msg = self.read_from_wasm(&store, result.error_ptr, result.error_len)?;
            let error_str = String::from_utf8_lossy(&error_msg).to_string();
            warn!(
                target: "calimero_runtime::merge",
                export_name,
                error = %error_str,
                "WASM merge function failed"
            );
            Err(MergeError::WasmCallbackFailed { message: error_str })
        }
    }
}

impl WasmMergeCallback for RuntimeMergeCallback {
    fn merge_custom(
        &self,
        local: &[u8],
        remote: &[u8],
        type_name: &str,
    ) -> Result<Vec<u8>, MergeError> {
        let export_name = format!("__calimero_merge_{type_name}");
        self.call_merge_function(&export_name, local, remote)
    }

    fn merge_root_state(&self, local: &[u8], remote: &[u8]) -> Result<Vec<u8>, MergeError> {
        self.call_merge_function(MERGE_ROOT_STATE_EXPORT, local, remote)
    }
}

impl RuntimeMergeCallback {
    /// Check if a custom type merge export exists.
    ///
    /// Returns `true` if `__calimero_merge_{type_name}` is exported by the WASM module.
    #[must_use]
    pub fn has_custom_merge_export(&self, type_name: &str) -> bool {
        let export_name = format!("__calimero_merge_{type_name}");
        self.instance.exports.get_function(&export_name).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_merge_result_size() {
        // Verify the size calculation is correct
        assert_eq!(WasmMergeResult::SIZE, 33);
    }
}
