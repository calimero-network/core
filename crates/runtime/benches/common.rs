//! Common utilities for runtime benchmarks

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{Engine, Module};

/// Create a default context ID for benchmarks
pub fn default_context_id() -> ContextId {
    ContextId::from([0; 32])
}

/// Create a default executor public key for benchmarks
pub fn default_executor() -> PublicKey {
    PublicKey::from([0; 32])
}

/// Compile the benchmark WASM module
///
/// Note: This assumes the WASM file exists at the specified path.
/// The WASM should be built from `apps/collections-benchmark-rust` first.
pub fn compile_benchmark_module() -> Module {
    let engine = Engine::default();

    // Try to load WASM from the built app
    // Path is relative to workspace root
    let wasm_path = "../../apps/collections-benchmark-rust/res/collections_benchmark_rust.wasm";

    // For now, we'll use a placeholder that will fail with a clear error
    // In practice, the WASM should be built before running benchmarks
    let wasm_bytes = match std::fs::read(wasm_path) {
        Ok(bytes) => bytes,
        Err(_) => {
            panic!(
                "WASM file not found at {}. Please build the collections-benchmark-rust app first:\n\
                cd apps/collections-benchmark-rust && cargo build --release --target wasm32-unknown-unknown",
                wasm_path
            );
        }
    };

    engine
        .compile(&wasm_bytes)
        .expect("Failed to compile WASM module")
}

/// Initialize the benchmark app state
pub fn init_app(module: &Module, storage: &mut InMemoryStorage) {
    let context_id = default_context_id();
    let executor = default_executor();

    module
        .run(context_id, executor, "init", &[], storage, None, None)
        .expect("Failed to initialize app");
}

/// Create JSON input for a method with a single string parameter
pub fn json_input(key: &str, value: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ key: value })).expect("Failed to serialize JSON input")
}

/// Create JSON input for a method with multiple parameters
pub fn json_input_multi(params: &[(&str, &str)]) -> Vec<u8> {
    let mut obj = serde_json::Map::new();
    for (key, value) in params {
        obj.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    serde_json::to_vec(&serde_json::Value::Object(obj)).expect("Failed to serialize JSON input")
}
