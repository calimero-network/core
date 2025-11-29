use std::fs;
use std::path::Path;

use calimero_wasm_abi::emitter::emit_manifest_from_crate;

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/custom_types.rs");

    // Parse all source files
    let lib_content = fs::read_to_string("src/lib.rs").expect("Failed to read src/lib.rs");
    let custom_types_content =
        fs::read_to_string("src/custom_types.rs").expect("Failed to read src/custom_types.rs");

    let sources = vec![
        ("lib.rs".to_owned(), lib_content),
        ("custom_types.rs".to_owned(), custom_types_content),
    ];

    // Generate ABI manifest from all source files
    let manifest = emit_manifest_from_crate(&sources).expect("Failed to emit ABI manifest");

    // Write ABI to JSON file for testing
    let abi_json = serde_json::to_string_pretty(&manifest).expect("Failed to serialize manifest");
    let res_dir = Path::new("res");
    if !res_dir.exists() {
        fs::create_dir_all(res_dir).expect("Failed to create res directory");
    }
    fs::write("res/abi.json", abi_json).expect("Failed to write ABI JSON");

    // Extract and write the state schema
    if let Ok(mut state_schema) = manifest.extract_state_schema() {
        state_schema.schema_version = "wasm-abi/1".to_owned();

        let state_schema_json =
            serde_json::to_string_pretty(&state_schema).expect("Failed to serialize state schema");
        fs::write("res/state-schema.json", state_schema_json)
            .expect("Failed to write state schema JSON");
    }
}
