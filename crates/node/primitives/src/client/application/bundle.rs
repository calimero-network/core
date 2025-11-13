//! Bundle-specific functionality for application installation.
//!
//! This module handles bundle archive (.mpk) operations including:
//! - Bundle detection and validation
//! - Manifest extraction and parsing
//! - Artifact extraction with deduplication
//! - Path traversal protection

use std::fs;
use std::io::Read;

use crate::bundle::BundleManifest;
use calimero_primitives::blobs::BlobId;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::bail;
use flate2::read::GzDecoder;
use serde_json;
use sha2::{Digest, Sha256};
use tar::Archive;
use tracing::{debug, warn};

use crate::client::NodeClient;

/// Check if a path points to a bundle archive (.mpk - Mero Package Kit)
pub(super) fn is_bundle_archive(path: &Utf8Path) -> bool {
    path.extension().map(|ext| ext == "mpk").unwrap_or(false)
}

impl NodeClient {
    /// Check if a blob contains a bundle archive by peeking at the first few entries.
    /// This is a lightweight check that only reads the archive structure, not the full content.
    ///
    /// Returns true if manifest.json is found, false otherwise.
    /// Logs warnings for parsing errors to help diagnose corrupted bundles.
    pub fn is_bundle_blob(blob_bytes: &[u8]) -> bool {
        // Quick check: try to parse as gzip/tar and look for manifest.json
        let tar = GzDecoder::new(blob_bytes);
        let mut archive = Archive::new(tar);

        // Only check first 10 entries to avoid reading entire archive
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

        for (i, entry_result) in entries.enumerate() {
            if i >= 10 {
                break; // Give up after 10 entries
            }
            match entry_result {
                Ok(entry) => {
                    match entry.path() {
                        Ok(path) => {
                            if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
                                return true;
                            }
                        }
                        Err(e) => {
                            warn!("Failed to read entry path in tar archive (possible corruption): {}", e);
                            // Continue checking other entries
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to read tar archive entry (possible corruption): {}",
                        e
                    );
                    // Continue checking other entries
                }
            }
        }
        false
    }
}

/// Extract and parse bundle manifest from bundle archive data
pub fn extract_bundle_manifest(bundle_data: &[u8]) -> eyre::Result<BundleManifest> {
    let tar = GzDecoder::new(bundle_data);
    let mut archive = Archive::new(tar);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;

        if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
            let mut manifest_json = String::new();
            entry.read_to_string(&mut manifest_json)?;
            let manifest: BundleManifest = serde_json::from_str(&manifest_json)
                .map_err(|e| eyre::eyre!("failed to parse manifest.json: {}", e))?;

            // Validate required fields
            if manifest.package.is_empty() {
                bail!("bundle manifest 'package' field is empty");
            }
            if manifest.app_version.is_empty() {
                bail!("bundle manifest 'appVersion' field is empty");
            }

            return Ok(manifest);
        }
    }

    bail!("manifest.json not found in bundle")
}

/// Find duplicate artifact in other versions by hash and relative path
/// Only matches files with the same relative path within the bundle to avoid
/// collisions between files with the same name in different directories
fn find_duplicate_artifact(
    node_root: &Utf8Path,
    package: &str,
    current_version: &str,
    hash: &[u8; 32],
    relative_path: &str,
) -> Option<Utf8PathBuf> {
    // Check other versions for the same hash at the same relative path
    let package_dir = node_root.join("applications").join(package);

    if let Ok(entries) = fs::read_dir(package_dir.as_std_path()) {
        for entry in entries.flatten() {
            if let Ok(version_name) = entry.file_name().into_string() {
                if version_name == current_version {
                    continue; // Skip current version
                }

                // Check extracted directory in this version at the same relative path
                let extracted_dir = package_dir.join(&version_name).join("extracted");
                let candidate_path = extracted_dir.join(relative_path);

                if candidate_path.exists() {
                    // Compute hash of candidate file
                    if let Ok(candidate_content) = fs::read(candidate_path.as_std_path()) {
                        let candidate_hash = Sha256::digest(&candidate_content);
                        let candidate_array: [u8; 32] = candidate_hash.into();
                        if candidate_array == *hash {
                            return Some(candidate_path);
                        }
                    }
                }
            }
        }
    }

    None
}

/// Extract bundle artifacts with deduplication
///
/// This function is synchronized per package-version to prevent race conditions
/// when multiple concurrent calls try to extract the same bundle.
pub fn extract_bundle_artifacts(
    bundle_data: &[u8],
    _manifest: &BundleManifest,
    extract_dir: &Utf8Path,
    node_root: &Utf8Path,
    package: &str,
    current_version: &str,
) -> eyre::Result<()> {
    // Create extraction directory
    fs::create_dir_all(extract_dir)?;

    // Use a lock file to prevent concurrent extraction of the same bundle version
    // Lock file path: extract_dir/.extracting.lock
    let lock_file_path = extract_dir.join(".extracting.lock");
    let marker_file_path = extract_dir.join(".extracted");

    // Check if extraction is already complete
    // Only skip if marker exists AND the expected WASM file exists
    // This handles the case where files were deleted but marker remains
    if marker_file_path.exists() {
        // Check if WASM file exists (using manifest to determine path)
        // If marker exists but WASM doesn't, marker is stale - remove it and re-extract
        let wasm_relative_path = _manifest
            .wasm
            .as_ref()
            .map(|w| w.path.as_str())
            .unwrap_or("app.wasm");

        // Validate WASM path to prevent path traversal attacks before checking existence
        if wasm_relative_path.contains("..") {
            bail!(
                "WASM path traversal detected in manifest: {} contains '..' component",
                wasm_relative_path
            );
        }

        let wasm_path = extract_dir.join(wasm_relative_path);

        // Additional validation: ensure the resolved path stays within extract_dir
        if wasm_path.exists() {
            // Validate path traversal even if file exists
            let canonical_wasm = wasm_path.canonicalize_utf8()?;

            // extract_dir might not exist if wasm_relative_path contains subdirectories
            // Reconstruct canonical extract_dir from wasm_path by removing relative path components
            let canonical_extract = if extract_dir.exists() {
                extract_dir.canonicalize_utf8()?
            } else {
                // Reconstruct extract_dir from wasm_path by removing wasm_relative_path components
                // Since we validated wasm_relative_path doesn't contain "..", this is safe
                let wasm_parent = wasm_path
                    .parent()
                    .ok_or_else(|| eyre::eyre!("WASM path has no parent directory"))?;
                let wasm_parent_canonical = wasm_parent.canonicalize_utf8()?;

                // Count depth of wasm_relative_path (number of path components)
                let relative_depth = wasm_relative_path
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .count()
                    .saturating_sub(1); // Subtract 1 for the filename itself

                // Go up relative_depth levels from wasm_parent to get extract_dir
                let mut canonical_extract_candidate = wasm_parent_canonical.clone();
                for _ in 0..relative_depth {
                    if let Some(parent) = canonical_extract_candidate.parent() {
                        canonical_extract_candidate = parent.to_path_buf();
                    } else {
                        bail!("Cannot reconstruct extract_dir from WASM path");
                    }
                }

                canonical_extract_candidate
                    .try_into()
                    .map_err(|_| eyre::eyre!("Failed to convert extract_dir path to Utf8PathBuf"))?
            };

            if !canonical_wasm.starts_with(&canonical_extract) {
                bail!(
                    "WASM path traversal detected: {} escapes extraction directory {}",
                    wasm_relative_path,
                    extract_dir
                );
            }

            debug!(
                package,
                version = current_version,
                "Bundle already extracted (marker file and WASM exist), skipping"
            );
            return Ok(());
        } else {
            // Marker exists but WASM doesn't - remove stale marker and re-extract
            debug!(
                package,
                version = current_version,
                "Marker file exists but WASM not found, removing stale marker"
            );
            let _ = fs::remove_file(&marker_file_path);
        }
    }

    // Try to acquire exclusive lock by creating lock file atomically
    // create_new() is atomic - fails if file exists (works on Unix and Windows)
    let lock_acquired = match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(lock_file_path.as_std_path())
    {
        Ok(_) => {
            // Lock file created - we're the first to extract
            true
        }
        Err(_) => {
            // Lock file already exists - another extraction is in progress
            // Wait and check if extraction completes
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if marker_file_path.exists() {
                    debug!(
                        package,
                        version = current_version,
                        "Bundle extraction completed by another process"
                    );
                    return Ok(());
                }
            }
            // If marker still doesn't exist after waiting, proceed anyway
            // (lock file might be stale from crashed process)
            warn!(
                package,
                version = current_version,
                "Lock file exists but extraction not complete, proceeding anyway"
            );
            false
        }
    };

    // Track if we created a lock file that needs cleanup
    let mut lock_created_by_us = lock_acquired;

    // Only proceed with extraction if we acquired the lock
    // (or if lock is stale and we're proceeding anyway)
    if !lock_acquired {
        // Try to remove stale lock and retry
        let _ = fs::remove_file(&lock_file_path);
        // Check marker one more time
        if marker_file_path.exists() {
            return Ok(());
        }
        // Create lock file again - handle race condition where another thread
        // might have created it between removal and this creation
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(lock_file_path.as_std_path())
        {
            Ok(_) => {
                // Successfully acquired lock, proceed with extraction
                lock_created_by_us = true;
            }
            Err(_) => {
                // Another thread created the lock between removal and creation
                // Wait briefly and check if extraction completed
                std::thread::sleep(std::time::Duration::from_millis(100));
                if marker_file_path.exists() {
                    debug!(
                        package,
                        version = current_version,
                        "Bundle extraction completed by another process after lock retry"
                    );
                    return Ok(());
                }
                // If marker still doesn't exist, the other thread is still extracting
                // Return error to avoid concurrent extraction
                bail!(
                    "Failed to acquire extraction lock after retry - another process is extracting"
                );
            }
        }
    }

    // Ensure lock file is cleaned up even if extraction fails
    // Use a guard to clean up on early return or error
    struct LockGuard {
        path: Utf8PathBuf,
        should_remove: std::cell::Cell<bool>,
    }

    impl Drop for LockGuard {
        fn drop(&mut self) {
            if self.should_remove.get() {
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    let lock_guard = LockGuard {
        path: lock_file_path.clone(),
        should_remove: std::cell::Cell::new(lock_created_by_us),
    };

    let tar = GzDecoder::new(bundle_data);
    let mut archive = Archive::new(tar);

    // Extract all files from bundle
    for entry_result in archive.entries()? {
        let mut entry = entry_result?;

        // Extract path_bytes first, converting to owned to drop borrow
        let path_bytes_owned = {
            let header = entry.header();
            header.path_bytes().into_owned()
        };

        let relative_path = {
            let path_str = std::str::from_utf8(&path_bytes_owned)
                .map_err(|_| eyre::eyre!("invalid UTF-8 in file path"))?;
            path_str.to_string()
        };

        // Skip macOS resource fork files (._* files)
        // Check filename component, not full path, to catch files in subdirectories
        if let Some(file_name) = std::path::Path::new(&relative_path)
            .file_name()
            .and_then(|n| n.to_str())
        {
            if file_name.starts_with("._") {
                continue;
            }
        }

        // Read content (header borrow is dropped)
        let mut content = Vec::new();
        std::io::copy(&mut entry, &mut content)?;

        // Preserve directory structure from bundle
        let dest_path = extract_dir.join(&relative_path);

        // Validate path to prevent path traversal attacks
        // Check that the relative path doesn't contain ".." components that would escape
        if relative_path.contains("..") {
            bail!(
                "Path traversal detected: {} contains '..' component",
                relative_path
            );
        }

        // Additional validation: ensure the resolved path stays within extract_dir
        // Use canonicalize if path exists, otherwise validate by checking parent
        if dest_path.exists() {
            let canonical_dest = dest_path.canonicalize_utf8()?;
            let canonical_extract = extract_dir.canonicalize_utf8()?;
            if !canonical_dest.starts_with(&canonical_extract) {
                bail!(
                    "Path traversal detected: {} escapes extraction directory {}",
                    relative_path,
                    extract_dir
                );
            }
        } else if let Some(parent) = dest_path.parent() {
            // Path doesn't exist yet, validate parent directory
            if parent.exists() {
                let canonical_parent = parent.canonicalize_utf8()?;
                let canonical_extract = extract_dir.canonicalize_utf8()?;
                if !canonical_parent.starts_with(&canonical_extract) {
                    bail!(
                        "Path traversal detected: parent of {} escapes extraction directory {}",
                        relative_path,
                        extract_dir
                    );
                }
            }
        }

        // Create parent directories if needed
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Compute hash
        let hash = Sha256::digest(&content);
        let hash_array: [u8; 32] = hash.into();

        // Check for duplicates in other versions at the same relative path
        if let Some(duplicate_path) = find_duplicate_artifact(
            node_root,
            package,
            current_version,
            &hash_array,
            &relative_path,
        ) {
            // Create hardlink to duplicate file
            if let Err(e) = fs::hard_link(duplicate_path.as_std_path(), dest_path.as_std_path()) {
                // If hardlink fails (e.g., cross-filesystem), fall back to copying
                warn!(
                    file = %relative_path,
                    duplicate = %duplicate_path,
                    error = %e,
                    "hardlink failed, copying instead"
                );
                fs::write(&dest_path, &content)?;
            } else {
                debug!(
                    file = %relative_path,
                    hash = hex::encode(hash),
                    duplicate = %duplicate_path,
                    "deduplicated artifact via hardlink"
                );
            }
        } else {
            // No duplicate found, write new file
            fs::write(&dest_path, &content)?;
            debug!(
                file = %relative_path,
                hash = hex::encode(hash),
                "extracted artifact"
            );
        }
    }

    // Write marker file to indicate extraction is complete
    fs::write(&marker_file_path, b"extracted")?;

    // Remove lock file explicitly on success (guard will skip removal if we already did it)
    if lock_guard.should_remove.get() {
        let _ = fs::remove_file(&lock_file_path);
        lock_guard.should_remove.set(false); // Prevent guard from removing it again
    }

    Ok(())
}
