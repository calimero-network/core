//! Bundle verification, manifest extraction, and path validation.
//!
//! Pure functions for working with `.mpk` bundle archives — no
//! `NodeClient` or blob manager state needed.

use std::io::Read;

use crate::bundle::{verify_manifest_signature, BundleManifest, ManifestVerification};
use eyre::bail;
use flate2::read::GzDecoder;
use semver::Version;
use tar::Archive;
use tracing::{debug, warn};

/// Validates that a string is safe for use as a filesystem path component.
///
/// This prevents path traversal attacks where malicious bundle manifests could
/// write files outside the intended `applications` directory.
pub fn validate_path_component(value: &str, field_name: &str) -> eyre::Result<()> {
    if value.contains("..") {
        bail!("{} contains path traversal sequence '..'", field_name);
    }
    if value.contains('/') || value.contains('\\') {
        bail!("{} contains directory separator", field_name);
    }
    if value.contains('\0') {
        bail!("{} contains null byte", field_name);
    }
    if value.len() >= 2 && value.as_bytes().get(1) == Some(&b':') {
        bail!("{} appears to be an absolute path", field_name);
    }
    Ok(())
}

/// Validates that an artifact path is safe for use as a relative filesystem path.
///
/// Unlike `validate_path_component`, this allows subdirectories (forward slashes)
/// but still prevents path traversal attacks.
pub fn validate_artifact_path(value: &str, field_name: &str) -> eyre::Result<()> {
    if value.is_empty() {
        bail!("{} is empty", field_name);
    }
    if value.contains('\0') {
        bail!("{} contains null byte", field_name);
    }
    if value.contains('\\') {
        bail!("{} contains backslash (use forward slashes)", field_name);
    }
    if value.starts_with('/') {
        bail!("{} is an absolute path", field_name);
    }
    if value.as_bytes().get(1) == Some(&b':') {
        bail!("{} appears to be an absolute Windows path", field_name);
    }
    if value.split('/').any(|c| c == "..") {
        bail!("{} contains path traversal component '..'", field_name);
    }
    Ok(())
}

/// Extracts bundle manifest, verifies signature, and returns both verification
/// result and typed manifest. Signature is mandatory.
pub fn verify_and_extract_manifest(
    bundle_data: &[u8],
) -> eyre::Result<(ManifestVerification, BundleManifest)> {
    let (manifest_json, manifest) = extract_bundle_manifest(bundle_data)?;
    let verification = verify_manifest_signature(&manifest_json)?;
    debug!(
        signer_id = %verification.signer_id,
        bundle_hash = %hex::encode(verification.bundle_hash),
        "bundle manifest signature verified"
    );
    Ok((verification, manifest))
}

/// Extracts bundle manifest, verifying signature only if present.
///
/// Used for dev installs and WASM loading of already-installed bundles,
/// where the bundle may have been admitted unsigned. If a signature IS
/// present it is still verified — invalid signatures are always rejected.
///
/// Production installs MUST use `verify_and_extract_manifest` instead,
/// which requires a valid signature.
pub fn extract_manifest_allow_unsigned(
    bundle_data: &[u8],
) -> eyre::Result<(ManifestVerification, BundleManifest)> {
    let (manifest_json, manifest) = extract_bundle_manifest(bundle_data)?;
    let verification = if manifest_json.get("signature").is_some() {
        let v = verify_manifest_signature(&manifest_json)?;
        debug!(
            signer_id = %v.signer_id,
            bundle_hash = %hex::encode(v.bundle_hash),
            "bundle manifest signature verified"
        );
        v
    } else {
        debug!("bundle has no signature field, treating as unsigned dev bundle");
        ManifestVerification {
            signer_id: "dev:unsigned".to_owned(),
            bundle_hash: [0u8; 32],
        }
    };
    Ok((verification, manifest))
}

/// Extract and parse bundle manifest from bundle archive data.
/// Returns both the raw JSON value (for signature verification) and the typed manifest.
pub fn extract_bundle_manifest(
    bundle_data: &[u8],
) -> eyre::Result<(serde_json::Value, BundleManifest)> {
    let tar = GzDecoder::new(bundle_data);
    let mut archive = Archive::new(tar);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;

        if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
            let mut manifest_str = String::new();
            entry.read_to_string(&mut manifest_str)?;

            let manifest_json: serde_json::Value = serde_json::from_str(&manifest_str)
                .map_err(|e| eyre::eyre!("failed to parse manifest.json as JSON: {}", e))?;

            let manifest: BundleManifest = serde_json::from_value(manifest_json.clone())
                .map_err(|e| eyre::eyre!("failed to parse manifest.json: {}", e))?;

            if manifest.package.is_empty() {
                bail!("bundle manifest 'package' field is empty");
            }
            if manifest.app_version.is_empty() {
                bail!("bundle manifest 'appVersion' field is empty");
            }

            validate_path_component(&manifest.package, "package")?;
            validate_path_component(&manifest.app_version, "appVersion")?;

            if let Some(ref wasm) = manifest.wasm {
                validate_artifact_path(&wasm.path, "wasm.path")?;
            }
            if let Some(ref abi) = manifest.abi {
                validate_artifact_path(&abi.path, "abi.path")?;
            }
            for (i, migration) in manifest.migrations.iter().enumerate() {
                validate_artifact_path(&migration.path, &format!("migrations[{}].path", i))?;
            }

            let current_runtime_version = Version::parse(env!("CALIMERO_RELEASE_VERSION"))
                .map_err(|e| eyre::eyre!("failed to parse current runtime version: {}", e))?;
            let min_runtime_version =
                Version::parse(&manifest.min_runtime_version).map_err(|e| {
                    eyre::eyre!(
                        "invalid minRuntimeVersion '{}': {}",
                        manifest.min_runtime_version,
                        e
                    )
                })?;

            if min_runtime_version > current_runtime_version {
                bail!(
                    "bundle requires runtime version {} but current runtime is {}",
                    min_runtime_version,
                    current_runtime_version
                );
            }

            return Ok((manifest_json, manifest));
        }
    }

    bail!("manifest.json not found in bundle")
}

/// Check if a blob contains a bundle archive by peeking at the first few entries.
///
/// Returns true if manifest.json is found, false otherwise.
pub fn is_bundle_blob(blob_bytes: &[u8]) -> bool {
    let tar = GzDecoder::new(blob_bytes);
    let mut archive = Archive::new(tar);

    let entries = match archive.entries() {
        Ok(entries) => entries,
        Err(e) => {
            warn!(
                "Failed to read tar archive entries (possible corruption): {}",
                e
            );
            return false;
        }
    };

    for (i, entry) in entries.enumerate() {
        if i >= 10 {
            break;
        }
        match entry {
            Ok(entry) => {
                if let Ok(path) = entry.path() {
                    if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
                        return true;
                    }
                }
            }
            Err(e) => {
                warn!("Failed to read tar entry {}: {}", i, e);
                break;
            }
        }
    }

    false
}

/// Check if a path points to a bundle archive (.mpk - Mero Package Kit)
pub fn is_bundle_archive(path: &camino::Utf8Path) -> bool {
    path.extension().map(|ext| ext == "mpk").unwrap_or(false)
}
