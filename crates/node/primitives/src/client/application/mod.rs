//! Application management functionality.
//!
//! This module is split into submodules for better organization:
//! - `bundle.rs` - Bundle-specific operations (extraction, manifest parsing)
//! - `install.rs` - Installation from various sources
//! - `query.rs` - Query and listing operations

mod bundle;
mod install;
mod query;

use std::sync::Arc;

use calimero_primitives::application::{Application, ApplicationBlob, ApplicationId};
use calimero_primitives::blobs::BlobId;
use calimero_store::key;
use eyre::bail;
use tracing::warn;

use super::NodeClient;

// Note: Functions from submodules are methods on NodeClient, so they're accessible
// through the NodeClient type. No re-exports needed as they're part of the impl blocks.

impl NodeClient {
    /// Get an application by its ApplicationId
    pub fn get_application(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<Application>> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        let Some(application) = handle.get(&key)? else {
            return Ok(None);
        };

        let application = Application::new(
            *application_id,
            ApplicationBlob {
                bytecode: application.bytecode.blob_id(),
                compiled: application.compiled.blob_id(),
            },
            application.size,
            application.source.parse()?,
            application.metadata.into_vec(),
        );

        Ok(Some(application))
    }

    /// Get the WASM bytecode for an application.
    ///
    /// For bundles, this extracts the WASM from the extracted directory.
    /// For single WASM files, this returns the blob bytes directly.
    pub async fn get_application_bytes(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<Arc<[u8]>>> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        let Some(application) = handle.get(&key)? else {
            return Ok(None);
        };

        // Determine if this is a bundle by checking package/version
        // Bundles have real package/version values, non-bundles use "unknown"/"0.0.0"
        // This avoids repeated decompression on every get_application_bytes call
        let is_bundle =
            application.package.as_ref() != "unknown" && application.version.as_ref() != "0.0.0";

        // Get blob bytes
        let Some(blob_bytes) = self
            .get_blob_bytes(&application.bytecode.blob_id(), None)
            .await?
        else {
            bail!("fatal: application points to dangling blob");
        };

        if is_bundle {
            // This is a bundle, extract WASM from extracted directory or bundle
            // Extract manifest (blocking I/O wrapped in spawn_blocking)
            let blob_bytes_clone = blob_bytes.clone();
            let manifest = tokio::task::spawn_blocking(move || {
                bundle::extract_bundle_manifest(&blob_bytes_clone)
            })
            .await??;
            let package = &manifest.package;
            let version = &manifest.app_version;

            // Resolve relative path against node root (must be done before spawn_blocking)
            let blobstore_root = self.blobstore.root_path();
            let node_root = blobstore_root
                .parent()
                .ok_or_else(|| eyre::eyre!("blobstore root has no parent"))?
                .to_path_buf();
            let extract_dir = node_root
                .join("applications")
                .join(package)
                .join(version)
                .join("extracted");

            // Get WASM path from manifest (fallback to "app.wasm" for backward compatibility)
            let wasm_relative_path = manifest
                .wasm
                .as_ref()
                .map(|w| w.path.as_str())
                .unwrap_or("app.wasm");

            // Validate WASM path to prevent path traversal attacks
            // Check that the relative path doesn't contain ".." components that would escape
            if wasm_relative_path.contains("..") {
                bail!(
                    "WASM path traversal detected: {} contains '..' component",
                    wasm_relative_path
                );
            }

            let wasm_path = extract_dir.join(wasm_relative_path);

            // Additional validation: ensure the resolved path stays within extract_dir
            // Use canonicalize if path exists, otherwise validate components
            if wasm_path.exists() {
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

                    canonical_extract_candidate.try_into().map_err(|_| {
                        eyre::eyre!("Failed to convert extract_dir path to Utf8PathBuf")
                    })?
                };

                // Ensure canonical_wasm is within canonical_extract
                if !canonical_wasm.starts_with(&canonical_extract) {
                    bail!(
                        "WASM path traversal detected: {} escapes extraction directory {}",
                        wasm_relative_path,
                        extract_dir
                    );
                }
            }

            if wasm_path.exists() {
                let wasm_bytes = tokio::fs::read(&wasm_path).await?;
                return Ok(Some(wasm_bytes.into()));
            } else {
                // Fallback: re-extract from bundle blob if extracted files missing
                warn!(
                    wasm_path = %wasm_path,
                    "extracted WASM not found, attempting to re-extract from bundle and persist to disk"
                );

                // Remove marker file if it exists (files were deleted, marker is stale)
                let marker_file_path = extract_dir.join(".extracted");
                if marker_file_path.exists() {
                    let _ = tokio::fs::remove_file(&marker_file_path).await;
                }

                // Re-extract entire bundle to disk (not just WASM) so future calls don't need to re-extract
                // This handles the case where sync_context_config installed the app before blob arrived
                // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
                let blob_bytes_clone = blob_bytes.clone();
                let manifest_clone = manifest.clone();
                let extract_dir_clone = extract_dir.to_path_buf();
                let node_root_clone = node_root.to_path_buf();
                let package_clone = package.to_string();
                let version_clone = version.to_string();
                tokio::task::spawn_blocking(move || {
                    bundle::extract_bundle_artifacts(
                        &blob_bytes_clone,
                        &manifest_clone,
                        &extract_dir_clone,
                        &node_root_clone,
                        &package_clone,
                        &version_clone,
                    )
                })
                .await??;

                // Now read the WASM file that was just extracted
                if wasm_path.exists() {
                    let wasm_bytes = tokio::fs::read(&wasm_path).await?;
                    return Ok(Some(wasm_bytes.into()));
                }

                bail!("WASM file not found in bundle after extraction");
            }
        }

        // Single WASM installation (existing behavior)
        // Reuse blob_bytes that were already fetched for bundle detection
        Ok(Some(blob_bytes))
    }

    /// Check if an application is installed
    pub fn has_application(&self, application_id: &ApplicationId) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        if let Some(application) = handle.get(&key)? {
            return self.has_blob(&application.bytecode.blob_id());
        }

        Ok(false)
    }

    /// Uninstall an application, removing its metadata and extracted files (for bundles)
    pub fn uninstall_application(&self, application_id: &ApplicationId) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        // Get application metadata before deleting to check if it's a bundle
        let application_meta = handle.get(&key)?;

        // Delete the ApplicationMeta entry
        handle.delete(&key)?;

        // Clean up extracted bundle files if this is a bundle
        if let Some(application) = application_meta {
            // Check if this is a bundle by checking package/version
            // Bundles have meaningful package/version (not "unknown"/"0.0.0")
            let is_bundle = application.package.as_ref() != "unknown"
                && application.version.as_ref() != "0.0.0";

            if is_bundle {
                // Construct path to extracted bundle directory
                let blobstore_root = self.blobstore.root_path();
                let node_root = blobstore_root
                    .parent()
                    .ok_or_else(|| eyre::eyre!("blobstore root has no parent"))?;
                let bundle_dir = node_root
                    .join("applications")
                    .join(application.package.as_ref())
                    .join(application.version.as_ref());

                // Delete the entire version directory (includes extracted/ subdirectory)
                if bundle_dir.exists() {
                    use tracing::debug;
                    debug!(
                        package = %application.package,
                        version = %application.version,
                        path = %bundle_dir,
                        "Removing extracted bundle directory"
                    );
                    if let Err(e) = std::fs::remove_dir_all(bundle_dir.as_std_path()) {
                        warn!(
                            package = %application.package,
                            version = %application.version,
                            path = %bundle_dir,
                            error = %e,
                            "Failed to remove extracted bundle directory"
                        );
                        // Don't fail uninstallation if cleanup fails - metadata is already deleted
                    } else {
                        debug!(
                            package = %application.package,
                            version = %application.version,
                            "Successfully removed extracted bundle directory"
                        );
                    }

                    // Also try to remove parent package directory if it's empty
                    let package_dir = node_root
                        .join("applications")
                        .join(application.package.as_ref());
                    if package_dir.exists() {
                        // Check if package directory is empty
                        if let Ok(mut entries) = std::fs::read_dir(package_dir.as_std_path()) {
                            if entries.next().is_none() {
                                // Directory is empty, remove it
                                if let Err(e) = std::fs::remove_dir(package_dir.as_std_path()) {
                                    debug!(
                                        package = %application.package,
                                        error = %e,
                                        "Failed to remove empty package directory (non-fatal)"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Update the compiled blob ID for an application
    pub fn update_compiled_app(
        &self,
        application_id: &ApplicationId,
        compiled_blob_id: &BlobId,
    ) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        let Some(mut application) = handle.get(&key)? else {
            bail!("application not found");
        };

        application.compiled = key::BlobMeta::new(*compiled_blob_id);

        handle.put(&key, &application)?;

        Ok(())
    }
}
