use std::path::Path;

use calimero_wasm_abi::embed::write_embedded_state_schema;
use calimero_wasm_abi::schema::Manifest;
use calimero_wasm_abi::validate::validate_manifest;

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
    // The node's reader (`read_embedded_state_schema`) discards any section that
    // fails `validate_manifest` and treats the ABI as absent — a silent failure.
    // Reject it here so an unreadable embed fails the build loudly instead.
    validate_manifest(&manifest).map_err(|e| {
        eyre::eyre!(
            "{} is not a valid manifest (the node would ignore it): {e}",
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
