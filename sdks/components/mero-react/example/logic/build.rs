use std::fs;
use std::path::Path;

use calimero_wasm_abi::emitter::emit_manifest;

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");

    // Parse the source code
    let src_path = Path::new("src/lib.rs");
    let src_content = fs::read_to_string(src_path).expect("Failed to read src/lib.rs");

    // Generate ABI manifest using the emitter
    let manifest = emit_manifest(&src_content).expect("Failed to emit ABI manifest");

    // Serialize the manifest to JSON
    let json = serde_json::to_string_pretty(&manifest).expect("Failed to serialize manifest");

    // Write the ABI JSON to the res directory
    let res_dir = Path::new("res");
    if !res_dir.exists() {
        fs::create_dir_all(res_dir).expect("Failed to create res directory");
    }

    let abi_path = res_dir.join("abi.json");
    fs::write(&abi_path, json).expect("Failed to write ABI JSON");

    println!("cargo:rerun-if-changed={}", abi_path.display());
}
