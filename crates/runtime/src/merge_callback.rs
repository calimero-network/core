//! WASM Merge Callback implementation for custom CRDT types.
//!
//! This module provides the bridge between the storage layer's merge dispatch
//! and the WASM application's custom merge logic for `CrdtType::Custom` types.
//!
//! # Architecture
//!
//! ```text
//! Storage Layer                  Runtime                     WASM App
//! ─────────────                  ───────                     ────────
//! compare_trees_with_callback() → WasmMergeCallback::merge() → __calimero_merge()
//!     ↓                              ↓                           ↓
//! Built-in CRDTs               Type dispatch            Custom merge logic
//! (Counter, Map, etc.)         (by type_name)           (impl Mergeable)
//! ```
//!
//! # Testability
//!
//! The `WasmMergeCallback` trait is already defined in `calimero-storage`.
//! This module provides:
//! - `RuntimeMergeCallback`: Production implementation that calls into WASM
//! - `MockMergeCallback`: Test implementation for unit testing sync logic

use calimero_storage::merge::{WasmMergeCallback, WasmMergeError};
use tracing::{debug, trace, warn};

// ============================================================================
// Production WASM Merge Callback
// ============================================================================

/// Production merge callback that calls into a loaded WASM module.
///
/// This callback is created from a compiled WASM module and calls the
/// `__calimero_merge` export function to perform custom type merging.
///
/// # WASM Export Requirements
///
/// The WASM module must export:
/// ```ignore
/// #[no_mangle]
/// pub extern "C" fn __calimero_merge(
///     type_name_ptr: u32, type_name_len: u32,
///     local_ptr: u32, local_len: u32,
///     remote_ptr: u32, remote_len: u32,
///     local_ts: u64, remote_ts: u64,
///     result_ptr: u32, // Output: pointer to merged data
///     result_len_ptr: u32, // Output: length of merged data
/// ) -> i32; // 0 = success, non-zero = error code
/// ```
pub struct RuntimeMergeCallback {
    /// Marker to prevent construction outside this module.
    /// In production, this would hold the WASM instance.
    _private: (),
}

impl RuntimeMergeCallback {
    /// Create a new runtime merge callback.
    ///
    /// In production, this would take a compiled WASM module.
    /// For now, this is a placeholder that falls back to LWW.
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Create a callback from a WASM module.
    ///
    /// This would validate that the module has the required exports.
    #[must_use]
    pub fn from_module(_module: &crate::Module) -> Option<Self> {
        // TODO: Check if module has __calimero_merge export
        // For now, return None to indicate WASM merge is not available
        // and the storage layer should fall back to registry or LWW
        None
    }
}

impl Default for RuntimeMergeCallback {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmMergeCallback for RuntimeMergeCallback {
    /// Merge custom type data during state sync.
    ///
    /// # KNOWN LIMITATION
    ///
    /// **WASM dispatch is not yet implemented.** This method falls back to:
    /// 1. Type registry (built-in CRDTs like Counter, Map work correctly)
    /// 2. Last-Write-Wins (custom `#[derive(Mergeable)]` types lose CRDT semantics)
    ///
    /// See `TECH-DEBT-SYNC-2026-01.md` for discussion.
    ///
    /// # Impact
    ///
    /// | Type | Behavior | Correct? |
    /// |------|----------|----------|
    /// | Built-in CRDTs | Registry merge | ✅ |
    /// | Custom Mergeable | LWW fallback | ⚠️ NO |
    ///
    /// # Future Work
    ///
    /// To properly support custom Mergeable types:
    /// 1. Store entity type metadata in storage
    /// 2. Implement `from_module()` to load WASM merge functions
    /// 3. Dispatch to `__calimero_merge` export
    fn merge_custom(
        &self,
        type_name: &str,
        local_data: &[u8],
        remote_data: &[u8],
        local_ts: u64,
        remote_ts: u64,
    ) -> Result<Vec<u8>, WasmMergeError> {
        debug!(
            type_name,
            local_len = local_data.len(),
            remote_len = remote_data.len(),
            local_ts,
            remote_ts,
            "RuntimeMergeCallback::merge_custom called"
        );

        // NOTE: WASM merge not implemented - see method docs for limitations
        warn!(
            type_name,
            "WASM merge not yet implemented, falling back to type registry or LWW"
        );

        // Try the type-name registry first (handles built-in CRDTs)
        if let Some(result) = calimero_storage::merge::try_merge_by_type_name(
            type_name,
            local_data,
            remote_data,
            local_ts,
            remote_ts,
        ) {
            trace!(type_name, "Merged via type registry");
            return result.map_err(|e| WasmMergeError::MergeFailed(e.to_string()));
        }

        // Fall back to Last-Write-Wins (WARNING: loses CRDT semantics for custom types!)
        trace!(
            type_name,
            local_ts,
            remote_ts,
            "Falling back to LWW - CRDT semantics lost"
        );
        if remote_ts > local_ts {
            Ok(remote_data.to_vec())
        } else {
            Ok(local_data.to_vec())
        }
    }
}

// ============================================================================
// Mock Merge Callback for Testing
// ============================================================================

/// Mock merge callback for testing sync logic without WASM.
///
/// This allows testing the sync protocol and merge dispatch without
/// requiring actual WASM modules.
///
/// # Example
///
/// ```ignore
/// use calimero_runtime::merge_callback::MockMergeCallback;
///
/// let mut mock = MockMergeCallback::new();
///
/// // Configure specific merge behavior
/// mock.on_merge("MyType", |local, remote, local_ts, remote_ts| {
///     // Custom test merge logic
///     Ok(remote.to_vec())
/// });
///
/// // Use in tests
/// let result = mock.merge_custom("MyType", &[1], &[2], 100, 200);
/// ```
#[derive(Default)]
pub struct MockMergeCallback {
    /// Recorded merge calls for verification.
    calls: std::sync::Mutex<Vec<MergeCall>>,
    /// Custom merge handlers by type name.
    handlers: std::sync::Mutex<
        std::collections::HashMap<
            String,
            Box<dyn Fn(&[u8], &[u8], u64, u64) -> Vec<u8> + Send + Sync>,
        >,
    >,
    /// Default behavior when no handler is registered.
    default_behavior: MockMergeBehavior,
}

/// Recorded merge call for test verification.
#[derive(Debug, Clone)]
pub struct MergeCall {
    /// Type name that was merged.
    pub type_name: String,
    /// Local data that was passed.
    pub local_data: Vec<u8>,
    /// Remote data that was passed.
    pub remote_data: Vec<u8>,
    /// Local timestamp.
    pub local_ts: u64,
    /// Remote timestamp.
    pub remote_ts: u64,
}

/// Default behavior for mock when no handler is registered.
#[derive(Debug, Clone, Copy, Default)]
pub enum MockMergeBehavior {
    /// Always return local data.
    KeepLocal,
    /// Always return remote data.
    KeepRemote,
    /// Use Last-Write-Wins (higher timestamp wins).
    #[default]
    LastWriteWins,
    /// Return an error.
    Error,
}

impl MockMergeCallback {
    /// Create a new mock callback with LWW default behavior.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a mock that always keeps local data.
    #[must_use]
    pub fn keep_local() -> Self {
        Self {
            default_behavior: MockMergeBehavior::KeepLocal,
            ..Default::default()
        }
    }

    /// Create a mock that always keeps remote data.
    #[must_use]
    pub fn keep_remote() -> Self {
        Self {
            default_behavior: MockMergeBehavior::KeepRemote,
            ..Default::default()
        }
    }

    /// Create a mock that always returns an error.
    #[must_use]
    pub fn always_error() -> Self {
        Self {
            default_behavior: MockMergeBehavior::Error,
            ..Default::default()
        }
    }

    /// Register a custom merge handler for a specific type.
    pub fn on_merge<F>(&self, type_name: &str, handler: F)
    where
        F: Fn(&[u8], &[u8], u64, u64) -> Vec<u8> + Send + Sync + 'static,
    {
        self.handlers
            .lock()
            .unwrap()
            .insert(type_name.to_string(), Box::new(handler));
    }

    /// Get all recorded merge calls.
    #[must_use]
    pub fn calls(&self) -> Vec<MergeCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Get the number of merge calls made.
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// Clear recorded calls.
    pub fn clear_calls(&self) {
        self.calls.lock().unwrap().clear();
    }

    /// Assert that a specific type was merged.
    ///
    /// # Panics
    ///
    /// Panics if the type was not merged.
    pub fn assert_merged(&self, type_name: &str) {
        let calls = self.calls.lock().unwrap();
        assert!(
            calls.iter().any(|c| c.type_name == type_name),
            "Expected merge call for type '{}', but got: {:?}",
            type_name,
            calls.iter().map(|c| &c.type_name).collect::<Vec<_>>()
        );
    }

    /// Assert no merges occurred.
    ///
    /// # Panics
    ///
    /// Panics if any merge was called.
    pub fn assert_no_merges(&self) {
        let calls = self.calls.lock().unwrap();
        assert!(
            calls.is_empty(),
            "Expected no merge calls, but got {} calls",
            calls.len()
        );
    }
}

impl WasmMergeCallback for MockMergeCallback {
    fn merge_custom(
        &self,
        type_name: &str,
        local_data: &[u8],
        remote_data: &[u8],
        local_ts: u64,
        remote_ts: u64,
    ) -> Result<Vec<u8>, WasmMergeError> {
        // Record the call
        self.calls.lock().unwrap().push(MergeCall {
            type_name: type_name.to_string(),
            local_data: local_data.to_vec(),
            remote_data: remote_data.to_vec(),
            local_ts,
            remote_ts,
        });

        // Check for custom handler
        if let Some(handler) = self.handlers.lock().unwrap().get(type_name) {
            return Ok(handler(local_data, remote_data, local_ts, remote_ts));
        }

        // Use default behavior
        match self.default_behavior {
            MockMergeBehavior::KeepLocal => Ok(local_data.to_vec()),
            MockMergeBehavior::KeepRemote => Ok(remote_data.to_vec()),
            MockMergeBehavior::LastWriteWins => {
                if remote_ts > local_ts {
                    Ok(remote_data.to_vec())
                } else {
                    Ok(local_data.to_vec())
                }
            }
            MockMergeBehavior::Error => Err(WasmMergeError::MergeFailed(
                "Mock configured to return error".to_string(),
            )),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_callback_records_calls() {
        let mock = MockMergeCallback::new();

        let result = mock
            .merge_custom("TestType", &[1, 2], &[3, 4], 100, 200)
            .unwrap();

        // LWW default: remote wins (200 > 100)
        assert_eq!(result, vec![3, 4]);

        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].type_name, "TestType");
        assert_eq!(calls[0].local_data, vec![1, 2]);
        assert_eq!(calls[0].remote_data, vec![3, 4]);
    }

    #[test]
    fn test_mock_callback_keep_local() {
        let mock = MockMergeCallback::keep_local();

        let result = mock
            .merge_custom("TestType", &[1, 2], &[3, 4], 100, 200)
            .unwrap();

        assert_eq!(result, vec![1, 2]);
    }

    #[test]
    fn test_mock_callback_keep_remote() {
        let mock = MockMergeCallback::keep_remote();

        let result = mock
            .merge_custom("TestType", &[1, 2], &[3, 4], 100, 200)
            .unwrap();

        assert_eq!(result, vec![3, 4]);
    }

    #[test]
    fn test_mock_callback_custom_handler() {
        let mock = MockMergeCallback::new();

        // Register custom handler that concatenates data
        mock.on_merge("ConcatType", |local, remote, _, _| {
            let mut result = local.to_vec();
            result.extend_from_slice(remote);
            result
        });

        let result = mock
            .merge_custom("ConcatType", &[1, 2], &[3, 4], 100, 200)
            .unwrap();

        assert_eq!(result, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_mock_callback_error() {
        let mock = MockMergeCallback::always_error();

        let result = mock.merge_custom("TestType", &[1], &[2], 100, 200);

        assert!(result.is_err());
    }

    #[test]
    fn test_mock_callback_assert_merged() {
        let mock = MockMergeCallback::new();

        mock.merge_custom("TypeA", &[], &[], 0, 0).unwrap();
        mock.merge_custom("TypeB", &[], &[], 0, 0).unwrap();

        mock.assert_merged("TypeA");
        mock.assert_merged("TypeB");
    }

    #[test]
    #[should_panic(expected = "Expected merge call for type 'TypeC'")]
    fn test_mock_callback_assert_merged_fails() {
        let mock = MockMergeCallback::new();

        mock.merge_custom("TypeA", &[], &[], 0, 0).unwrap();

        mock.assert_merged("TypeC");
    }

    #[test]
    fn test_mock_callback_lww_local_wins() {
        let mock = MockMergeCallback::new();

        // Local has higher timestamp
        let result = mock.merge_custom("TestType", &[1], &[2], 200, 100).unwrap();

        assert_eq!(result, vec![1]); // Local wins
    }

    #[test]
    fn test_runtime_callback_fallback() {
        let callback = RuntimeMergeCallback::new();

        // Should fall back to LWW since WASM is not implemented
        let result = callback
            .merge_custom("UnknownType", &[1], &[2], 100, 200)
            .unwrap();

        // Remote wins (200 > 100)
        assert_eq!(result, vec![2]);
    }
}
