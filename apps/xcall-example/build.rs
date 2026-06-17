use std::fs;
use std::path::Path;

use calimero_wasm_abi::emitter::emit_manifest;

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");

    // Parse the source code
    let src_path = Path::new("src/lib.rs");
    let src_content = fs::read_to_string(src_path).expect("Failed to read src/lib.rs");

    // Generate ABI manifest using the emitter
    let mut manifest = emit_manifest(&src_content).expect("Failed to emit ABI manifest");

    // The emitter lists methods/events in source order, but this is the one app
    // that embeds the *full* abi.json (to carry `xcall_callable`), and the node's
    // reader runs `validate_manifest`, which requires lexicographically sorted
    // method and event names — otherwise it discards the section as invalid and
    // the L3 xcall gate silently never arms. Sort here so the embed validates.
    manifest.methods.sort_by(|a, b| a.name.cmp(&b.name));
    manifest.events.sort_by(|a, b| a.name.cmp(&b.name));

    // Serialize the manifest to JSON
    let json = serde_json::to_string_pretty(&manifest).expect("Failed to serialize manifest");

    // Write the ABI JSON to the res directory
    let res_dir = Path::new("res");
    if !res_dir.exists() {
        fs::create_dir_all(res_dir).expect("Failed to create res directory");
    }

    let abi_path = res_dir.join("abi.json");
    fs::write(&abi_path, json).expect("Failed to write ABI JSON");

    // Extract and write the state schema
    if let Ok(mut state_schema) = manifest.extract_state_schema() {
        state_schema.schema_version = "wasm-abi/1".to_owned();

        let state_schema_json =
            serde_json::to_string_pretty(&state_schema).expect("Failed to serialize state schema");
        let state_schema_path = res_dir.join("state-schema.json");
        fs::write(&state_schema_path, state_schema_json)
            .expect("Failed to write state schema JSON");
    }
}
