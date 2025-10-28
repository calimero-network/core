use std::fs;
use std::path::{Path, PathBuf};

use wasmparser::{Parser as WasmParser, Payload};

pub fn extract_abi(wasm_file: &PathBuf, output: Option<&Path>, verify: bool) -> eyre::Result<()> {
    // Read the WASM file
    let wasm_bytes = fs::read(wasm_file)?;

    // Parse the WASM file
    let parser = WasmParser::new(0);
    let mut abi_section: Option<Vec<u8>> = None;
    let mut has_get_abi_exports = false;

    for payload in parser.parse_all(&wasm_bytes) {
        match payload? {
            Payload::CustomSection(section) => {
                if section.name() == "calimero_abi_v1" {
                    abi_section = Some(section.data().to_vec());
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export?;
                    if export.name == "get_abi_ptr"
                        || export.name == "get_abi_len"
                        || export.name == "get_abi"
                    {
                        has_get_abi_exports = true;
                    }
                }
            }
            _ => {}
        }
    }

    // Check if we found the ABI section
    let abi_json = match abi_section {
        Some(data) => {
            let json_str = String::from_utf8(data)?;

            // Validate JSON
            drop(serde_json::from_str::<serde_json::Value>(&json_str)?);

            json_str
        }
        None => {
            eyre::bail!("No 'calimero_abi_v1' custom section found in WASM file");
        }
    };

    // Verify if requested
    if verify && !has_get_abi_exports {
        eyre::bail!("Verification failed: get_abi* exports not found in WASM file");
    }

    // Determine output path
    let output_path = output.map_or_else(
        || {
            let mut path = wasm_file.clone();
            let _ = path.set_extension("abi.json");
            path
        },
        Path::to_path_buf,
    );

    // Write the ABI JSON
    fs::write(&output_path, abi_json)?;

    println!("ABI extracted successfully to: {}", output_path.display());

    if verify {
        println!("Verification passed: get_abi* exports found");
    }

    Ok(())
}
