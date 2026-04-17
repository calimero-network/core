pub mod bundle;
mod install;
mod query;

use std::io::{self, ErrorKind};
use std::sync::Arc;

use crate::bundle::{BundleManifest, ManifestVerification};
use calimero_primitives::application::{
    Application, ApplicationBlob, ApplicationId, ApplicationSource,
};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use camino::{Utf8Path, Utf8PathBuf};
use eyre::bail;
use flate2::read::GzDecoder;
use futures_util::{io::Cursor, TryStreamExt};
use reqwest::Url;
use sha2::{Digest, Sha256};
use std::fs;
use tar::Archive;
use tokio::fs::File;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{debug, trace, warn};

use super::NodeClient;

const MAX_ERROR_BODY_LEN: usize = 256;

impl NodeClient {
    pub fn get_application(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<Application>> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        let Some(application) = handle.get(&key)? else {
            return Ok(None);
        };

        let services = application
            .services
            .iter()
            .map(|s| {
                (
                    s.name.to_string(),
                    ApplicationBlob {
                        bytecode: s.bytecode.blob_id(),
                        compiled: s.compiled.blob_id(),
                    },
                )
            })
            .collect();

        let mut app = Application::new(
            *application_id,
            ApplicationBlob {
                bytecode: application.bytecode.blob_id(),
                compiled: application.compiled.blob_id(),
            },
            application.size,
            application.source.parse()?,
            application.metadata.into_vec(),
        )
        .with_bundle_info(
            application.signer_id.to_string(),
            application.package.to_string(),
            application.version.to_string(),
        );
        app.services = services;

        Ok(Some(app))
    }

    pub async fn get_application_bytes(
        &self,
        application_id: &ApplicationId,
        service_name: Option<&str>,
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
            // This is a bundle, extract WASM from extracted directory or bundle.
            // The bundle was already admitted (possibly unsigned in dev mode),
            // so use the unsigned-tolerant extractor here.
            let blob_bytes_clone = Arc::clone(&blob_bytes);
            let (_, manifest) = tokio::task::spawn_blocking(move || {
                bundle::extract_manifest_allow_unsigned(&blob_bytes_clone)
            })
            .await??;
            let package = &manifest.package;
            let version = &manifest.app_version;

            // Resolve relative path against node root (must be done before spawn_blocking)
            let blobstore_root = self.blob_manager.root_path();
            let node_root = blobstore_root
                .parent()
                .ok_or_else(|| eyre::eyre!("blobstore root has no parent"))?
                .to_path_buf();
            let extract_dir = node_root
                .join("applications")
                .join(package)
                .join(version)
                .join("extracted");

            // Resolve WASM path from manifest, supporting both single and multi-service bundles
            let wasm_relative_path = match service_name {
                Some(name) => {
                    // Multi-service: find the named service in the services array
                    manifest
                        .services
                        .as_ref()
                        .and_then(|svcs| svcs.iter().find(|s| s.name == name))
                        .map(|s| s.wasm.path.as_str())
                        .ok_or_else(|| {
                            eyre::eyre!("service '{}' not found in bundle manifest", name)
                        })?
                }
                None => {
                    // Single-service: use top-level wasm field (fallback to "app.wasm")
                    manifest
                        .wasm
                        .as_ref()
                        .map(|w| w.path.as_str())
                        .unwrap_or("app.wasm")
                }
            };

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
                let blob_bytes_clone = Arc::clone(&blob_bytes);
                let manifest_clone = manifest.clone();
                let extract_dir_clone = extract_dir.to_path_buf();
                let node_root_clone = node_root.to_path_buf();
                let package_clone = package.to_string();
                let version_clone = version.to_string();
                tokio::task::spawn_blocking(move || {
                    Self::extract_bundle_artifacts(
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

    pub fn has_application(&self, application_id: &ApplicationId) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        if let Some(application) = handle.get(&key)? {
            return self.has_blob(&application.bytecode.blob_id());
        }

        Ok(false)
    }

    // Installation, uninstallation, and bundle extraction are in `install` submodule.
    // Query and management functions are in `query` submodule.
}

#[cfg(test)]
mod tests;
