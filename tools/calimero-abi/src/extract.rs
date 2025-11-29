use std::fs;
use std::path::{Path, PathBuf};

use calimero_wasm_abi::schema::Manifest;
use serde_json::Value;
use wasmparser::{Parser as WasmParser, Payload};

/// Extract the state schema (state root type and all its dependencies) from a WASM file
pub fn extract_state_schema(wasm_file: &PathBuf, output: Option<&Path>) -> eyre::Result<()> {
    // Read the WASM file
    let wasm_bytes = fs::read(wasm_file)?;

    // Parse the WASM file
    let parser = WasmParser::new(0);
    let mut abi_section: Option<Vec<u8>> = None;

    for payload in parser.parse_all(&wasm_bytes) {
        if let Payload::CustomSection(section) = payload? {
            if section.name() == "calimero_abi_v1" {
                abi_section = Some(section.data().to_vec());
                break;
            }
        }
    }

    // Check if we found the ABI section
    let abi_json = match abi_section {
        Some(data) => String::from_utf8(data)
            .map_err(|e| eyre::eyre!("ABI data is not valid UTF-8: {}", e))?,
        None => {
            eyre::bail!("No 'calimero_abi_v1' custom section found in WASM file");
        }
    };

    // Parse the ABI manifest
    let manifest: Manifest = serde_json::from_str(&abi_json)
        .map_err(|e| eyre::eyre!("Failed to parse ABI manifest: {}", e))?;

    // Extract state schema using the Manifest method
    let state_schema_manifest = manifest
        .extract_state_schema()
        .map_err(|e| eyre::eyre!("Failed to extract state schema: {}", e))?;

    // Build the state schema JSON (same format as build-time emission)
    let state_schema = serde_json::json!({
        "state_root": state_schema_manifest.state_root,
        "types": state_schema_manifest.types
    });

    // Serialize with pretty printing
    let schema_json = serde_json::to_string_pretty(&state_schema)
        .map_err(|e| eyre::eyre!("Failed to serialize state schema: {}", e))?;

    // Determine output path
    let output_path = output.map_or_else(
        || {
            let mut path = wasm_file.clone();
            let _ = path.set_extension("state-schema.json");
            path
        },
        Path::to_path_buf,
    );

    // Write the state schema JSON
    fs::write(&output_path, schema_json)?;

    println!(
        "State schema extracted successfully to: {}",
        output_path.display()
    );
    if let Some(ref root) = state_schema_manifest.state_root {
        println!("State root: {}", root);
    }
    println!(
        "Found {} type definitions",
        state_schema_manifest.types.len()
    );

    Ok(())
}

/// Extract just the types schema from a WASM file
pub fn extract_types_schema(wasm_file: &PathBuf, output: Option<&Path>) -> eyre::Result<()> {
    // Read the WASM file
    let wasm_bytes = fs::read(wasm_file)?;

    // Parse the WASM file
    let parser = WasmParser::new(0);
    let mut abi_section: Option<Vec<u8>> = None;

    for payload in parser.parse_all(&wasm_bytes) {
        if let Payload::CustomSection(section) = payload? {
            if section.name() == "calimero_abi_v1" {
                abi_section = Some(section.data().to_vec());
                break;
            }
        }
    }

    // Check if we found the ABI section
    let abi_json = match abi_section {
        Some(data) => String::from_utf8(data)
            .map_err(|e| eyre::eyre!("ABI data is not valid UTF-8: {}", e))?,
        None => {
            eyre::bail!("No 'calimero_abi_v1' custom section found in WASM file");
        }
    };

    // Parse the ABI JSON
    let abi_value: Value =
        serde_json::from_str(&abi_json).map_err(|e| eyre::eyre!("Invalid ABI JSON: {}", e))?;

    // Extract just the types field
    let types_schema = abi_value
        .get("types")
        .ok_or_else(|| eyre::eyre!("ABI JSON missing 'types' field"))?;

    // Serialize the types schema with pretty printing
    let types_json = serde_json::to_string_pretty(types_schema)
        .map_err(|e| eyre::eyre!("Failed to serialize types schema: {}", e))?;

    // Determine output path
    let output_path = output.map_or_else(
        || {
            let mut path = wasm_file.clone();
            let _ = path.set_extension("types.json");
            path
        },
        Path::to_path_buf,
    );

    // Write the types schema JSON
    fs::write(&output_path, types_json)?;

    // Count the number of types
    let type_count = if let Value::Object(types_map) = types_schema {
        types_map.len()
    } else {
        0
    };

    println!(
        "Types schema extracted successfully to: {}",
        output_path.display()
    );
    println!("Found {} type definitions", type_count);

    Ok(())
}

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
            // Fall back to reading from abi.json file
            // First, try to find workspace root and app directory
            let wasm_dir = wasm_file
                .parent()
                .ok_or_else(|| eyre::eyre!("WASM file has no parent directory"))?;

            // Extract app name from WASM filename (e.g., "abi_conformance.wasm" -> "abi_conformance")
            let wasm_stem = wasm_file
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| eyre::eyre!("WASM file has no valid filename"))?;

            // Find workspace root by looking for Cargo.toml going up from WASM file
            let mut workspace_root = wasm_dir.to_path_buf();
            let mut found_workspace = false;
            for _ in 0..10 {
                // Limit search depth
                if workspace_root.join("Cargo.toml").exists() {
                    found_workspace = true;
                    break;
                }
                if let Some(parent) = workspace_root.parent() {
                    workspace_root = parent.to_path_buf();
                } else {
                    break;
                }
            }

            let mut abi_json_paths = Vec::new();

            // If we found workspace root, look in apps/{app_name}/res/abi.json
            if found_workspace {
                abi_json_paths.push(
                    workspace_root
                        .join("apps")
                        .join(wasm_stem)
                        .join("res/abi.json"),
                );
                // Also try with underscores converted to hyphens (e.g., "kv-store" vs "kv_store")
                let app_name_hyphenated = wasm_stem.replace('_', "-");
                if app_name_hyphenated != wasm_stem {
                    abi_json_paths.push(
                        workspace_root
                            .join("apps")
                            .join(&app_name_hyphenated)
                            .join("res/abi.json"),
                    );
                }
            }

            // Also check relative to WASM file location (for backwards compatibility)
            abi_json_paths.extend(vec![
                wasm_dir.join("abi.json"),
                wasm_dir.join("res/abi.json"),
                wasm_dir
                    .parent()
                    .map(|p| p.join("res/abi.json"))
                    .unwrap_or_else(|| wasm_dir.join("abi.json")),
            ]);

            let mut found_abi = None;
            for path in &abi_json_paths {
                if path.exists() {
                    found_abi = Some(fs::read_to_string(path)?);
                    break;
                }
            }

            match found_abi {
                Some(json_str) => {
                    // Validate JSON
                    drop(serde_json::from_str::<serde_json::Value>(&json_str)?);
                    json_str
                }
                None => {
                    eyre::bail!(
                        "No 'calimero_abi_v1' custom section found in WASM file and no abi.json file found. \
                        Checked: {}",
                        abi_json_paths.iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
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
