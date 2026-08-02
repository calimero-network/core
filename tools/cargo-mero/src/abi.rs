//! The app's own ABI manifest, the only one there is.
//!
//! `#[app::logic]` generates `__calimero_abi()` from the `AbiType` impls the
//! app's types carry, so the compiler resolves aliases, macro-generated and
//! re-exported types before the manifest is assembled.

use std::collections::BTreeSet;
use std::process::Command;

use calimero_wasm_abi::{validate_manifest, Manifest};
use camino::Utf8Path;
use eyre::{bail, eyre, Context, Result};

/// Compiles the generated `__calimero_abi()` in. Never set for a wasm build, so
/// a shipped app carries none of this.
const ABI_CFG: &str = "--cfg calimero_abi --check-cfg cfg(calimero_abi)";

/// The manifest the app itself builds. Apps are cdylibs and cannot be run, so
/// extraction rides the test harness: the generated `__calimero_abi_dump` test
/// writes the manifest to `$CALIMERO_ABI_OUT`.
///
/// `Ok(None)` means the crate generates no entry point at all - it has no
/// `#[app::logic]`, or its SDK predates one.
pub fn extract(manifest_path: &Utf8Path, features: &BTreeSet<String>) -> Result<Option<Manifest>> {
    let out = tempfile::NamedTempFile::new().wrap_err("failed to create temp file")?;

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    // The resolved set spelled out in full, so this build describes the same
    // schema the wasm was compiled with - naming the caller's `--features`
    // instead would fail on a sibling service that does not declare them.
    let features: Vec<&str> = features.iter().map(String::as_str).collect();
    // No `--exact`: the test sits in a module named after the state type.
    let output = Command::new(cargo)
        .args(["test", "--lib", "--no-default-features", "--manifest-path"])
        .arg(manifest_path)
        .args(["--features", &features.join(",")])
        .arg("__calimero_abi_dump")
        .env("CALIMERO_ABI_OUT", out.path())
        .env("RUSTFLAGS", crate::build::remapped_rustflags(ABI_CFG))
        .output()
        .wrap_err("failed to spawn `cargo test`")?;
    if !output.status.success() {
        bail!(
            "failed to extract the ABI of {manifest_path}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let json = std::fs::read(out.path())
        .wrap_err_with(|| format!("failed to read the ABI dumped by {manifest_path}"))?;
    if json.is_empty() {
        return Ok(None);
    }

    let manifest: Manifest = serde_json::from_slice(&json)
        .wrap_err_with(|| format!("{manifest_path} dumped an unparseable ABI manifest"))?;
    validate_manifest(&manifest)
        .map_err(|e| eyre!("{manifest_path} built an invalid ABI manifest: {e}"))?;
    Ok(Some(manifest))
}
