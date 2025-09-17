use std::path::Path;
use std::{env, fs};

use calimero_wasm_abi::embed::generate_embed_code;
use calimero_wasm_abi::emitter::emit_manifest;

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/types/id.rs");

    // Parse the source code
    let src_path = Path::new("src/lib.rs");
    let src_content = fs::read_to_string(src_path).expect("Failed to read src/lib.rs");

    // Generate ABI manifest using the emitter
    let manifest = emit_manifest(&src_content).expect("Failed to emit ABI manifest");

    // Write ABI to JSON file for testing
    let abi_json = serde_json::to_string_pretty(&manifest).expect("Failed to serialize manifest");
    let res_dir = Path::new("res");
    if !res_dir.exists() {
        fs::create_dir_all(res_dir).expect("Failed to create res directory");
    }
    fs::write("res/abi.json", abi_json).expect("Failed to write ABI JSON");

    // Generate the embed code
    let embed_code = generate_embed_code(&manifest);

    // Write the generated code to a file that will be included in the build
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let generated_path = Path::new(&out_dir).join("generated_abi.rs");

    fs::write(&generated_path, embed_code).expect("Failed to write generated ABI code");

    // Tell Cargo to include our generated file
    println!(
        "cargo:rustc-env=GENERATED_ABI_PATH={}",
        generated_path.display()
    );
}
