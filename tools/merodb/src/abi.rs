use std::fs;
use std::path::Path;

use calimero_wasm_abi::schema::Manifest;
use eyre::Result;

/// Load state schema from a JSON value
///
/// The JSON value should be in the format produced by `calimero-abi state`:
/// ```json
/// {
///   "state_root": "TypeName",
///   "types": { ... }
/// }
/// ```
///
/// This creates a schema containing only the state root type and its dependencies,
/// which is sufficient for deserializing state.
pub fn load_state_schema_from_json_value(schema_value: &serde_json::Value) -> Result<Manifest> {
    // Extract state_root and types
    let state_root = schema_value
        .get("state_root")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("State schema missing 'state_root' field"))?
        .to_string();

    let types_value = schema_value
        .get("types")
        .ok_or_else(|| eyre::eyre!("State schema missing 'types' field"))?;

    // Parse types into BTreeMap<String, TypeDef>
    use calimero_wasm_abi::schema::TypeDef;
    use std::collections::BTreeMap;
    let types: BTreeMap<String, TypeDef> = serde_json::from_value(types_value.clone())
        .map_err(|e| eyre::eyre!("Failed to parse types from state schema: {}", e))?;

    // Create a schema with just the state types (Manifest is used as the container type)
    let schema = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        types,
        methods: Vec::new(),
        events: Vec::new(),
        state_root: Some(state_root),
    };

    Ok(schema)
}

/// Load state schema from a JSON file
///
/// The JSON file should be in the format produced by `calimero-abi state`:
/// ```json
/// {
///   "state_root": "TypeName",
///   "types": { ... }
/// }
/// ```
///
/// This creates a schema containing only the state root type and its dependencies,
/// which is sufficient for deserializing state.
pub fn load_state_schema_from_json(schema_path: &Path) -> Result<Manifest> {
    let schema_json = fs::read_to_string(schema_path)
        .map_err(|e| eyre::eyre!("Failed to read state schema file: {}", e))?;

    let schema_value: serde_json::Value = serde_json::from_str(&schema_json)
        .map_err(|e| eyre::eyre!("Failed to parse state schema JSON: {}", e))?;

    load_state_schema_from_json_value(&schema_value)
}
