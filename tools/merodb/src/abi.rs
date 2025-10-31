use std::fs;
use std::path::Path;

use calimero_wasm_abi::schema::Manifest;
use eyre::Result;
use wasmparser::{Parser, Payload};

/// Extract ABI from a WASM file
///
/// Reads the WASM file and extracts the ABI schema from the "calimero_abi_v1" custom section
pub fn extract_abi_from_wasm(wasm_path: &Path) -> Result<Manifest> {
    // Read the WASM file
    let wasm_bytes = fs::read(wasm_path)?;

    // Parse the WASM file
    let parser = Parser::new(0);
    let mut abi_section: Option<Vec<u8>> = None;

    for payload in parser.parse_all(&wasm_bytes) {
        if let Payload::CustomSection(section) = payload? {
            if section.name() == "calimero_abi_v1" {
                abi_section = Some(section.data().to_vec());
            }
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

    // Parse the Manifest
    let manifest: Manifest = serde_json::from_str(&abi_json)?;

    Ok(manifest)
}
