use std::path::Path;

use calimero_wasm_abi::embed::write_embedded_state_schema;
use calimero_wasm_abi::schema::Manifest;

/// Embed `schema` (a state-schema.json) into `wasm` as the `calimero_abi_v1`
/// custom section, in place. Idempotent (replaces any existing section).
pub fn run_embed(wasm: &Path, schema: &Path) -> eyre::Result<()> {
    let schema_bytes = std::fs::read(schema)
        .map_err(|e| eyre::eyre!("failed to read {}: {e}", schema.display()))?;
    let manifest: Manifest = serde_json::from_slice(&schema_bytes).map_err(|e| {
        eyre::eyre!(
            "failed to parse {} as a state-schema manifest: {e}",
            schema.display()
        )
    })?;
    let original =
        std::fs::read(wasm).map_err(|e| eyre::eyre!("failed to read {}: {e}", wasm.display()))?;
    let updated = write_embedded_state_schema(&original, &manifest)
        .map_err(|e| eyre::eyre!("failed to embed schema into {}: {e}", wasm.display()))?;
    std::fs::write(wasm, updated)
        .map_err(|e| eyre::eyre!("failed to write {}: {e}", wasm.display()))?;
    println!(
        "✓ embedded calimero_abi_v1 ({} types) into {}",
        manifest.types.len(),
        wasm.display()
    );
    Ok(())
}
