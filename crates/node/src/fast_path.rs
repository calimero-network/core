use calimero_node_primitives::capabilities::{AppManifest, Capability, CapabilityValidator};
use calimero_node_primitives::clock::Hlc;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use serde_json::Value;
use thiserror::Error;
use tracing::{debug, info};

/// Fast path executor for pure operations
/// 
/// This executor can apply CRDT updates directly without WASM execution
/// for operations that have been declared as pure in the application manifest.
#[derive(Debug)]
pub struct FastPathExecutor {
    /// Application manifest with capability declarations
    manifest: AppManifest,
    /// Audit counter for mismatch detection
    audit_mismatches: std::sync::atomic::AtomicU64,
}

impl FastPathExecutor {
    /// Create a new fast path executor
    pub fn new(manifest: AppManifest) -> Self {
        Self {
            manifest,
            audit_mismatches: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Check if an operation can use fast path
    pub fn can_use_fast_path(&self, method: &str, payload_size: usize) -> bool {
        CapabilityValidator::can_use_fast_path(&self.manifest, method, payload_size)
    }

    /// Execute a pure operation using fast path
    /// 
    /// This applies the operation directly to the CRDT state without WASM execution.
    /// Returns a mock Outcome that mimics the WASM execution result.
    pub fn execute_fast_path(
        &self,
        context_id: &ContextId,
        executor: &PublicKey,
        method: &str,
        payload: &[u8],
        node_id: [u8; 32],
    ) -> Result<FastPathOutcome, FastPathError> {
        debug!(
            context_id=%context_id,
            executor=%executor,
            method,
            payload_size=payload.len(),
            "Executing fast path operation"
        );

        // Parse payload as JSON
        let payload_json: Value = serde_json::from_slice(payload)
            .map_err(|e| FastPathError::InvalidPayload(format!("Failed to parse JSON: {}", e)))?;

        // Get the capability for this method
        let capability = self
            .manifest
            .get_method_capability(method)
            .ok_or_else(|| FastPathError::UnknownMethod(method.to_string()))?;

        // Generate HLC timestamp for the operation
        let mut hlc = Hlc::new(node_id);
        let timestamp = hlc.now();

        // Apply the operation based on capability
        let result = match capability {
            Capability::PureKvSet => self.apply_kv_set(payload_json, timestamp),
            Capability::PureCounterInc => self.apply_counter_inc(payload_json, timestamp),
            Capability::PureCounterDec => self.apply_counter_dec(payload_json, timestamp),
            Capability::PureSetAdd => self.apply_set_add(payload_json, timestamp),
            Capability::PureSetRemove => self.apply_set_remove(payload_json, timestamp),
            Capability::PureMapPut => self.apply_map_put(payload_json, timestamp),
            Capability::PureMapRemove => self.apply_map_remove(payload_json, timestamp),
            Capability::PureListAppend => self.apply_list_append(payload_json, timestamp),
            Capability::PureListRemove => self.apply_list_remove(payload_json, timestamp),
            _ => return Err(FastPathError::UnsupportedOperation(format!("Capability {:?} not implemented", capability))),
        }?;

        info!(
            context_id=%context_id,
            method,
            capability=?capability,
            "Fast path operation completed successfully"
        );

        Ok(result)
    }

    /// Apply a key-value set operation
    fn apply_kv_set(&self, payload: Value, _timestamp: Hlc) -> Result<FastPathOutcome, FastPathError> {
        let key = payload["key"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'key' field".to_string()))?;
        let value = payload["value"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'value' field".to_string()))?;

        // In a real implementation, this would update the actual CRDT state
        // For now, we'll create a mock outcome
        debug!(key, value, "Applying KV set operation");

        Ok(FastPathOutcome {
            returns: Ok(Some(vec![])), // Empty result
            logs: vec![format!("Fast path KV set: {} = {}", key, value)],
            events: vec![],
            root_hash: Some([0; 32]), // Mock root hash
            artifact: vec![], // No artifact for fast path
        })
    }

    /// Apply a counter increment operation
    fn apply_counter_inc(&self, payload: Value, _timestamp: Hlc) -> Result<FastPathOutcome, FastPathError> {
        let counter_name = payload["counter"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'counter' field".to_string()))?;

        debug!(counter_name, "Applying counter increment operation");

        Ok(FastPathOutcome {
            returns: Ok(Some(vec![])),
            logs: vec![format!("Fast path counter increment: {}", counter_name)],
            events: vec![],
            root_hash: Some([0; 32]),
            artifact: vec![],
        })
    }

    /// Apply a counter decrement operation
    fn apply_counter_dec(&self, payload: Value, _timestamp: Hlc) -> Result<FastPathOutcome, FastPathError> {
        let counter_name = payload["counter"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'counter' field".to_string()))?;

        debug!(counter_name, "Applying counter decrement operation");

        Ok(FastPathOutcome {
            returns: Ok(Some(vec![])),
            logs: vec![format!("Fast path counter decrement: {}", counter_name)],
            events: vec![],
            root_hash: Some([0; 32]),
            artifact: vec![],
        })
    }

    /// Apply a set add operation
    fn apply_set_add(&self, payload: Value, _timestamp: Hlc) -> Result<FastPathOutcome, FastPathError> {
        let set_name = payload["set"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'set' field".to_string()))?;
        let item = payload["item"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'item' field".to_string()))?;

        debug!(set_name, item, "Applying set add operation");

        Ok(FastPathOutcome {
            returns: Ok(Some(vec![])),
            logs: vec![format!("Fast path set add: {} to {}", item, set_name)],
            events: vec![],
            root_hash: Some([0; 32]),
            artifact: vec![],
        })
    }

    /// Apply a set remove operation
    fn apply_set_remove(&self, payload: Value, _timestamp: Hlc) -> Result<FastPathOutcome, FastPathError> {
        let set_name = payload["set"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'set' field".to_string()))?;
        let item = payload["item"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'item' field".to_string()))?;

        debug!(set_name, item, "Applying set remove operation");

        Ok(FastPathOutcome {
            returns: Ok(Some(vec![])),
            logs: vec![format!("Fast path set remove: {} from {}", item, set_name)],
            events: vec![],
            root_hash: Some([0; 32]),
            artifact: vec![],
        })
    }

    /// Apply a map put operation
    fn apply_map_put(&self, payload: Value, _timestamp: Hlc) -> Result<FastPathOutcome, FastPathError> {
        let map_name = payload["map"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'map' field".to_string()))?;
        let key = payload["key"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'key' field".to_string()))?;
        let value = payload["value"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'value' field".to_string()))?;

        debug!(map_name, key, value, "Applying map put operation");

        Ok(FastPathOutcome {
            returns: Ok(Some(vec![])),
            logs: vec![format!("Fast path map put: {}[{}] = {}", map_name, key, value)],
            events: vec![],
            root_hash: Some([0; 32]),
            artifact: vec![],
        })
    }

    /// Apply a map remove operation
    fn apply_map_remove(&self, payload: Value, _timestamp: Hlc) -> Result<FastPathOutcome, FastPathError> {
        let map_name = payload["map"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'map' field".to_string()))?;
        let key = payload["key"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'key' field".to_string()))?;

        debug!(map_name, key, "Applying map remove operation");

        Ok(FastPathOutcome {
            returns: Ok(Some(vec![])),
            logs: vec![format!("Fast path map remove: {}[{}]", map_name, key)],
            events: vec![],
            root_hash: Some([0; 32]),
            artifact: vec![],
        })
    }

    /// Apply a list append operation
    fn apply_list_append(&self, payload: Value, _timestamp: Hlc) -> Result<FastPathOutcome, FastPathError> {
        let list_name = payload["list"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'list' field".to_string()))?;
        let item = payload["item"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'item' field".to_string()))?;

        debug!(list_name, item, "Applying list append operation");

        Ok(FastPathOutcome {
            returns: Ok(Some(vec![])),
            logs: vec![format!("Fast path list append: {} to {}", item, list_name)],
            events: vec![],
            root_hash: Some([0; 32]),
            artifact: vec![],
        })
    }

    /// Apply a list remove operation
    fn apply_list_remove(&self, payload: Value, _timestamp: Hlc) -> Result<FastPathOutcome, FastPathError> {
        let list_name = payload["list"]
            .as_str()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'list' field".to_string()))?;
        let index = payload["index"]
            .as_u64()
            .ok_or_else(|| FastPathError::InvalidPayload("Missing 'index' field".to_string()))?;

        debug!(list_name, index, "Applying list remove operation");

        Ok(FastPathOutcome {
            returns: Ok(Some(vec![])),
            logs: vec![format!("Fast path list remove: index {} from {}", index, list_name)],
            events: vec![],
            root_hash: Some([0; 32]),
            artifact: vec![],
        })
    }

    /// Get the audit mismatch counter
    pub fn get_audit_mismatches(&self) -> u64 {
        self.audit_mismatches.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Increment the audit mismatch counter
    pub fn increment_audit_mismatches(&self) {
        self.audit_mismatches.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Record an audit mark for background checking
    pub fn record_audit_mark(&self, context_id: &ContextId, method: &str, payload: &[u8]) {
        debug!(
            context_id=%context_id,
            method,
            payload_size=payload.len(),
            "Recording audit mark for background checking"
        );
        // In a real implementation, this would store the operation for later
        // background verification against WASM execution
    }
}

/// Mock outcome for fast path operations
#[derive(Debug)]
pub struct FastPathOutcome {
    pub returns: Result<Option<Vec<u8>>, String>,
    pub logs: Vec<String>,
    pub events: Vec<String>,
    pub root_hash: Option<[u8; 32]>,
    pub artifact: Vec<u8>,
}

/// Errors that can occur during fast path execution
#[derive(Debug, Error)]
pub enum FastPathError {
    #[error("Invalid payload: {0}")]
    InvalidPayload(String),
    #[error("Unknown method: {0}")]
    UnknownMethod(String),
    #[error("Operation not supported: {0}")]
    UnsupportedOperation(String),
    #[error("CRDT state error: {0}")]
    CrdtStateError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_node_primitives::capabilities::Capability;

    fn create_test_manifest() -> AppManifest {
        let mut manifest = AppManifest::new("test-app".to_string(), "1.0.0".to_string());
        manifest.add_capability(Capability::PureKvSet);
        manifest.add_capability(Capability::PureCounterInc);
        manifest.map_method("set".to_string(), Capability::PureKvSet);
        manifest.map_method("inc".to_string(), Capability::PureCounterInc);
        manifest
    }

    fn create_test_context_id() -> ContextId {
        [1u8; 32].into()
    }

    fn create_test_public_key() -> PublicKey {
        [2u8; 32].into()
    }

    #[test]
    fn test_fast_path_executor_creation() {
        let manifest = create_test_manifest();
        let executor = FastPathExecutor::new(manifest);
        
        assert!(executor.can_use_fast_path("set", 512));
        assert!(!executor.can_use_fast_path("set", 2048)); // Too large
        assert!(!executor.can_use_fast_path("unknown", 512)); // Unknown method
    }

    #[test]
    fn test_kv_set_fast_path() {
        let manifest = create_test_manifest();
        let executor = FastPathExecutor::new(manifest);
        
        let payload = serde_json::json!({
            "key": "test_key",
            "value": "test_value"
        });
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        
        let context_id = create_test_context_id();
        let executor_pk = create_test_public_key();
        let node_id = [1u8; 32];
        
        let result = executor.execute_fast_path(&context_id, &executor_pk, "set", &payload_bytes, node_id);
        assert!(result.is_ok());
        
        let outcome = result.unwrap();
        assert!(outcome.returns.is_ok());
        assert!(!outcome.logs.is_empty());
        assert!(outcome.logs[0].contains("Fast path KV set"));
    }

    #[test]
    fn test_counter_inc_fast_path() {
        let manifest = create_test_manifest();
        let executor = FastPathExecutor::new(manifest);
        
        let payload = serde_json::json!({
            "counter": "test_counter"
        });
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        
        let context_id = create_test_context_id();
        let executor_pk = create_test_public_key();
        let node_id = [1u8; 32];
        
        let result = executor.execute_fast_path(&context_id, &executor_pk, "inc", &payload_bytes, node_id);
        assert!(result.is_ok());
        
        let outcome = result.unwrap();
        assert!(outcome.returns.is_ok());
        assert!(!outcome.logs.is_empty());
        assert!(outcome.logs[0].contains("Fast path counter increment"));
    }

    #[test]
    fn test_invalid_payload() {
        let manifest = create_test_manifest();
        let executor = FastPathExecutor::new(manifest);
        
        let payload = serde_json::json!({
            "invalid": "payload"
        });
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        
        let context_id = create_test_context_id();
        let executor_pk = create_test_public_key();
        let node_id = [1u8; 32];
        
        let result = executor.execute_fast_path(&context_id, &executor_pk, "set", &payload_bytes, node_id);
        assert!(result.is_err());
        
        match result.unwrap_err() {
            FastPathError::InvalidPayload(_) => {},
            _ => panic!("Expected InvalidPayload error"),
        }
    }

    #[test]
    fn test_unknown_method() {
        let manifest = create_test_manifest();
        let executor = FastPathExecutor::new(manifest);
        
        let payload = serde_json::json!({});
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        
        let context_id = create_test_context_id();
        let executor_pk = create_test_public_key();
        let node_id = [1u8; 32];
        
        let result = executor.execute_fast_path(&context_id, &executor_pk, "unknown", &payload_bytes, node_id);
        assert!(result.is_err());
        
        match result.unwrap_err() {
            FastPathError::UnknownMethod(_) => {},
            _ => panic!("Expected UnknownMethod error"),
        }
    }
}
