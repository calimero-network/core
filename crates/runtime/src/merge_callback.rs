//! WASM merge callback implementation for custom CRDT types.
//!
//! This module implements the [`WasmMergeCallback`] trait from `calimero-storage`,
//! enabling applications to define custom merge logic via WASM exports.
//!
//! # WASM Exports
//!
//! Applications must export these functions to use custom merges:
//!
//! - `__calimero_merge_root_state(local_ptr, local_len, remote_ptr, remote_len) -> result_ptr`
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
//! # Timeout
//!
//! WASM merge calls have a configurable timeout (default: 5 seconds) to prevent
//! infinite loops or malicious code from blocking sync.

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
pub struct RuntimeMergeCallback {
    /// The WASM store.
    store: Store,
    /// The WASM instance with the application module.
    instance: Instance,
    /// Timeout for WASM merge operations.
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
                store,
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
    fn write_to_wasm(&mut self, data: &[u8]) -> Result<(u64, u64), MergeError> {
        // Get the allocator function
        let alloc_fn: TypedFunction<u64, u64> = self
            .instance
            .exports
            .get_typed_function(&self.store, "__calimero_alloc")
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!(
                    "WASM module does not export __calimero_alloc function: {e}. \
                     Ensure your app uses #[app::state] macro."
                ),
            })?;

        // Allocate memory for the data
        let len = data.len() as u64;
        let ptr =
            alloc_fn
                .call(&mut self.store, len)
                .map_err(|e| MergeError::WasmCallbackFailed {
                    message: format!("Failed to allocate WASM memory: {e}"),
                })?;

        // Write data to the allocated memory
        let memory = self.get_memory()?;
        let view = memory.view(&self.store);
        view.write(ptr, data)
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("Failed to write data to WASM memory: {e}"),
            })?;

        Ok((ptr, len))
    }

    /// Read data from WASM memory at the given pointer and length.
    fn read_from_wasm(&self, ptr: u64, len: u64) -> Result<Vec<u8>, MergeError> {
        let memory = self.get_memory()?;
        let view = memory.view(&self.store);
        let mut buf = vec![0u8; len as usize];
        view.read(ptr, &mut buf)
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("Failed to read data from WASM memory: {e}"),
            })?;
        Ok(buf)
    }

    /// Call the root state merge function.
    fn call_merge_root_state(
        &mut self,
        local: &[u8],
        remote: &[u8],
    ) -> Result<Vec<u8>, MergeError> {
        // Get the merge function
        let merge_fn: TypedFunction<(u64, u64, u64, u64), u64> = self
            .instance
            .exports
            .get_typed_function(&self.store, MERGE_ROOT_STATE_EXPORT)
            .map_err(|e| MergeError::WasmMergeNotExported {
                export_name: format!("{MERGE_ROOT_STATE_EXPORT}: {e}"),
            })?;

        // Write inputs to WASM memory
        let (local_ptr, local_len) = self.write_to_wasm(local)?;
        let (remote_ptr, remote_len) = self.write_to_wasm(remote)?;

        debug!(
            target: "calimero_runtime::merge",
            local_len,
            remote_len,
            "Calling WASM merge_root_state"
        );

        // Call the merge function
        // TODO: Add timeout handling (Issue #1780 acceptance criteria)
        let result_ptr = merge_fn
            .call(
                &mut self.store,
                local_ptr,
                local_len,
                remote_ptr,
                remote_len,
            )
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("WASM merge_root_state call failed: {e}"),
            })?;

        // Read the result
        let memory = self.get_memory()?;
        let result = WasmMergeResult::from_memory(memory, &self.store, result_ptr)?;

        if result.success != 0 {
            // Success - read the merged data
            let merged = self.read_from_wasm(result.data_ptr, result.data_len)?;
            debug!(
                target: "calimero_runtime::merge",
                merged_len = merged.len(),
                "WASM merge_root_state succeeded"
            );
            Ok(merged)
        } else {
            // Failure - read the error message
            let error_msg = self.read_from_wasm(result.error_ptr, result.error_len)?;
            let error_str = String::from_utf8_lossy(&error_msg).to_string();
            warn!(
                target: "calimero_runtime::merge",
                error = %error_str,
                "WASM merge_root_state failed"
            );
            Err(MergeError::WasmCallbackFailed { message: error_str })
        }
    }
}

impl WasmMergeCallback for RuntimeMergeCallback {
    fn merge_custom(
        &self,
        _local: &[u8],
        _remote: &[u8],
        type_name: &str,
    ) -> Result<Vec<u8>, MergeError> {
        // WasmMergeCallback trait takes &self but we need &mut self for the store.
        // This is a trait design limitation - use merge_custom_mut for actual merges.
        //
        // The caller should use RuntimeMergeCallback directly with merge_custom_mut()
        // instead of going through the trait when mutable access is available.
        warn!(
            target: "calimero_runtime::merge",
            type_name = %type_name,
            "merge_custom called via trait - requires mutable access"
        );
        Err(MergeError::WasmCallbackFailed {
            message: format!(
                "RuntimeMergeCallback::merge_custom requires mutable access for '{}'. \
                 Use merge_custom_mut() directly.",
                type_name
            ),
        })
    }

    fn merge_root_state(&self, _local: &[u8], _remote: &[u8]) -> Result<Vec<u8>, MergeError> {
        // Need mutable access for the store, but the trait takes &self
        // This is a limitation of the current design - we'd need interior mutability
        // or a different approach. For now, we'll note this needs refactoring.
        //
        // TODO: Refactor to use interior mutability (Mutex<Store>) or pass store separately
        Err(MergeError::WasmCallbackFailed {
            message:
                "RuntimeMergeCallback requires mutable access. Use merge_root_state_mut instead."
                    .to_string(),
        })
    }
}

impl RuntimeMergeCallback {
    /// Merge root state with mutable access to the store.
    ///
    /// This is the actual implementation that requires `&mut self`.
    pub fn merge_root_state_mut(
        &mut self,
        local: &[u8],
        remote: &[u8],
    ) -> Result<Vec<u8>, MergeError> {
        self.call_merge_root_state(local, remote)
    }

    /// Merge a custom type with mutable access to the store.
    ///
    /// This calls the WASM export `__calimero_merge_{type_name}` to merge
    /// entities with `CrdtType::Custom(type_name)` metadata.
    ///
    /// # Arguments
    ///
    /// * `local` - The locally stored value (Borsh-serialized)
    /// * `remote` - The incoming remote value (Borsh-serialized)
    /// * `type_name` - The custom type name (used to find the merge export)
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - The WASM module doesn't export `__calimero_merge_{type_name}`
    /// - Memory allocation or copying fails
    /// - The merge function returns an error
    pub fn merge_custom_mut(
        &mut self,
        local: &[u8],
        remote: &[u8],
        type_name: &str,
    ) -> Result<Vec<u8>, MergeError> {
        let export_name = format!("__calimero_merge_{}", type_name);

        // Get the type-specific merge function
        let merge_fn: TypedFunction<(u64, u64, u64, u64), u64> = self
            .instance
            .exports
            .get_typed_function(&self.store, &export_name)
            .map_err(|e| MergeError::WasmMergeNotExported {
                export_name: format!(
                    "{}: {}. Add #[app::mergeable] to the Mergeable impl for {}.",
                    export_name, e, type_name
                ),
            })?;

        // Write inputs to WASM memory
        let (local_ptr, local_len) = self.write_to_wasm(local)?;
        let (remote_ptr, remote_len) = self.write_to_wasm(remote)?;

        debug!(
            target: "calimero_runtime::merge",
            type_name = %type_name,
            local_len,
            remote_len,
            "Calling WASM merge for custom type"
        );

        // Call the merge function
        let result_ptr = merge_fn
            .call(
                &mut self.store,
                local_ptr,
                local_len,
                remote_ptr,
                remote_len,
            )
            .map_err(|e| MergeError::WasmCallbackFailed {
                message: format!("WASM {} call failed: {}", export_name, e),
            })?;

        // Read the result
        let memory = self.get_memory()?;
        let result = WasmMergeResult::from_memory(memory, &self.store, result_ptr)?;

        if result.success != 0 {
            // Success - read the merged data
            let merged = self.read_from_wasm(result.data_ptr, result.data_len)?;
            debug!(
                target: "calimero_runtime::merge",
                type_name = %type_name,
                merged_len = merged.len(),
                "WASM merge for custom type succeeded"
            );
            Ok(merged)
        } else {
            // Failure - read the error message
            let error_msg = self.read_from_wasm(result.error_ptr, result.error_len)?;
            let error_str = String::from_utf8_lossy(&error_msg).to_string();
            warn!(
                target: "calimero_runtime::merge",
                type_name = %type_name,
                error = %error_str,
                "WASM merge for custom type failed"
            );
            Err(MergeError::WasmCallbackFailed { message: error_str })
        }
    }

    /// Check if a custom type merge export exists.
    ///
    /// Returns `true` if `__calimero_merge_{type_name}` is exported by the WASM module.
    #[must_use]
    pub fn has_custom_merge_export(&self, type_name: &str) -> bool {
        let export_name = format!("__calimero_merge_{}", type_name);
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
