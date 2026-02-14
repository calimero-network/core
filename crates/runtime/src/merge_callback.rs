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
//! infinite loops or malicious code from blocking sync. When a timeout occurs, the
//! caller is unblocked and `MergeError::WasmTimeout` is returned. Note that due to
//! Wasmer limitations, the underlying WASM execution may continue in a background
//! thread until completion.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use calimero_storage::collections::crdt_meta::{MergeError, WasmMergeCallback};
use tracing::{debug, warn};
use wasmer::{Instance, Memory, Store, TypedFunction};

/// Default timeout for WASM merge operations (5 seconds).
pub const DEFAULT_MERGE_TIMEOUT_MS: u64 = 5000;

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
/// Uses interior mutability (`Arc<Mutex>`) for the store because the
/// `WasmMergeCallback` trait requires `&self` but Wasmer's `Store`
/// needs mutable access for function calls. `Arc<Mutex>` is used to
/// satisfy the `Send + Sync` bounds on the trait and to enable
/// timeout enforcement via background threads.
pub struct RuntimeMergeCallback {
    /// The WASM store (interior mutability for trait compatibility).
    store: Arc<Mutex<Store>>,
    /// The WASM instance with the application module.
    instance: Arc<Instance>,
    /// Timeout for WASM merge operations in milliseconds.
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
                store: Arc::new(Mutex::new(store)),
                instance: Arc::new(instance),
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

    /// Call a merge function by export name with timeout enforcement.
    ///
    /// The WASM call is executed in a background thread with a timeout. If the
    /// timeout expires, the caller is unblocked and `MergeError::WasmTimeout` is
    /// returned. Note that due to Wasmer limitations, the underlying WASM
    /// execution may continue in the background thread until completion.
    fn call_merge_function(
        &self,
        export_name: &str,
        local: &[u8],
        remote: &[u8],
    ) -> Result<Vec<u8>, MergeError> {
        let store = Arc::clone(&self.store);
        let instance = Arc::clone(&self.instance);
        let export_name_owned = export_name.to_string();
        let local_owned = local.to_vec();
        let remote_owned = remote.to_vec();
        let timeout_ms = self.timeout_ms;

        debug!(
            target: "calimero_runtime::merge",
            export_name,
            local_len = local.len(),
            remote_len = remote.len(),
            timeout_ms,
            "Calling WASM merge function with timeout"
        );

        // Channel to receive the result from the worker thread
        let (tx, rx) = mpsc::sync_channel(1);

        // Spawn a thread to execute the WASM call
        std::thread::spawn(move || {
            let result = Self::execute_merge_call(
                &store,
                &instance,
                &export_name_owned,
                &local_owned,
                &remote_owned,
            );
            // Send result; ignore error if receiver dropped (timeout occurred)
            let _ = tx.send(result);
        });

        // Wait for result with timeout
        match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                warn!(
                    target: "calimero_runtime::merge",
                    export_name,
                    timeout_ms,
                    "WASM merge operation timed out"
                );
                Err(MergeError::WasmTimeout { timeout_ms })
            }
            Err(RecvTimeoutError::Disconnected) => {
                warn!(
                    target: "calimero_runtime::merge",
                    export_name,
                    "WASM merge thread panicked or disconnected"
                );
                Err(MergeError::WasmCallbackFailed {
                    message: "WASM merge thread panicked".to_string(),
                })
            }
        }
    }

    /// Execute the actual merge call within the worker thread.
    ///
    /// This is a static helper method that performs the WASM call with the
    /// provided store and instance references.
    fn execute_merge_call(
        store: &Arc<Mutex<Store>>,
        instance: &Arc<Instance>,
        export_name: &str,
        local: &[u8],
        remote: &[u8],
    ) -> Result<Vec<u8>, MergeError> {
        // Lock the store for the entire operation
        let mut store_guard = store.lock().map_err(|e| MergeError::WasmCallbackFailed {
            message: format!("Failed to lock WASM store: {e}"),
        })?;

        // Get the merge function
        let merge_fn: TypedFunction<(u64, u64, u64, u64), u64> = instance
            .exports
            .get_typed_function(&*store_guard, export_name)
            .map_err(|e| MergeError::WasmMergeNotExported {
                export_name: format!("{export_name}: {e}"),
            })?;

        // Get the allocator function
        let alloc_fn: TypedFunction<u64, u64> = instance
            .exports
            .get_typed_function(&*store_guard, "__calimero_alloc")
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!(
                    "WASM module does not export __calimero_alloc function: {e}. \
                     Ensure your app uses #[app::state] macro."
                ),
            })?;

        // Get memory
        let memory =
            instance
                .exports
                .get_memory("memory")
                .map_err(|e| MergeError::WasmCallbackFailed {
                    message: format!("Failed to get WASM memory: {e}"),
                })?;

        // Write local data to WASM memory
        let local_len = local.len() as u64;
        let local_ptr = alloc_fn.call(&mut *store_guard, local_len).map_err(|e| {
            MergeError::WasmCallbackFailed {
                message: format!("Failed to allocate WASM memory for local data: {e}"),
            }
        })?;
        memory
            .view(&*store_guard)
            .write(local_ptr, local)
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("Failed to write local data to WASM memory: {e}"),
            })?;

        // Write remote data to WASM memory
        let remote_len = remote.len() as u64;
        let remote_ptr = alloc_fn.call(&mut *store_guard, remote_len).map_err(|e| {
            MergeError::WasmCallbackFailed {
                message: format!("Failed to allocate WASM memory for remote data: {e}"),
            }
        })?;
        memory
            .view(&*store_guard)
            .write(remote_ptr, remote)
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("Failed to write remote data to WASM memory: {e}"),
            })?;

        // Call the merge function
        let result_ptr = merge_fn
            .call(
                &mut *store_guard,
                local_ptr,
                local_len,
                remote_ptr,
                remote_len,
            )
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("WASM {export_name} call failed: {e}"),
            })?;

        // Read the result
        let result = WasmMergeResult::from_memory(memory, &*store_guard, result_ptr)?;

        if result.success != 0 {
            // Success - read the merged data
            let view = memory.view(&*store_guard);
            let mut merged = vec![0u8; result.data_len as usize];
            view.read(result.data_ptr, &mut merged).map_err(|e| {
                MergeError::WasmCallbackFailed {
                    message: format!("Failed to read merged data from WASM memory: {e}"),
                }
            })?;
            debug!(
                target: "calimero_runtime::merge",
                export_name,
                merged_len = merged.len(),
                "WASM merge function succeeded"
            );
            Ok(merged)
        } else {
            // Failure - read the error message
            let view = memory.view(&*store_guard);
            let mut error_msg = vec![0u8; result.error_len as usize];
            view.read(result.error_ptr, &mut error_msg).map_err(|e| {
                MergeError::WasmCallbackFailed {
                    message: format!("Failed to read error message from WASM memory: {e}"),
                }
            })?;
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
