pub mod bundle;
mod install;
mod query;

use std::sync::Arc;

use crate::bundle::BundleManifest;
use calimero_primitives::application::{Application, ApplicationBlob, ApplicationId};
use calimero_primitives::blobs::BlobId;
use calimero_store::key;
use calimero_store::key::AsKeyParts;
use eyre::bail;
use futures_util::TryStreamExt;
use sha2::Digest;
use tracing::{debug, warn};

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

/// One locally-retained bytecode version of an application's package — an
/// entry in [`NodeClient::list_application_versions`].
#[derive(Clone, Debug)]
pub struct ApplicationVersionInfo {
    /// Manifest `app_version` (the row's version for raw-wasm apps).
    pub version: String,
    /// The bundle (or raw-wasm) bytecode blob.
    pub blob_id: BlobId,
    /// Blob size in bytes.
    pub size: u64,
    /// Manifest package name.
    pub package: String,
}

impl NodeClient {
    /// Application wasm bytes straight from a bytecode BLOB — used for a
    /// context pinned to a blob the application row no longer references
    /// (version-stable bundle id overwritten in place). Fully in-memory: the
    /// pinned version's extracted dir may be gone, but the blob is still in
    /// the blobstore. `None` when the blob itself is absent locally.
    /// Service names inside the bundle at `blob_id`: one `Some(name)` per
    /// declared service, or a single `None` for single-service bundles. For a
    /// raw (non-bundle) wasm blob the result is also `[None]` — exactly the
    /// `service` values `application_bytes_from_blob` accepts. `Ok(None)`
    /// when the blob is absent locally.
    pub async fn bundle_service_names(
        &self,
        blob_id: &BlobId,
    ) -> eyre::Result<Option<Vec<Option<String>>>> {
        let Some(blob_bytes) = self.get_blob_bytes(blob_id, None).await? else {
            return Ok(None);
        };
        if !Self::is_bundle_blob(&blob_bytes) {
            return Ok(Some(vec![None]));
        }
        let names = tokio::task::spawn_blocking(move || -> eyre::Result<Vec<Option<String>>> {
            let (_, manifest) = bundle::extract_manifest_allow_unsigned(&blob_bytes)?;
            Ok(manifest
                .wasm_artifacts()
                .iter()
                .map(|a| a.name.map(str::to_owned))
                .collect())
        })
        .await
        .map_err(|e| eyre::eyre!("bundle manifest read task failed: {e}"))??;
        Ok(Some(names))
    }

    /// Bundle manifest of the blob at `blob_id`, parsed leniently (unsigned
    /// allowed — admission verification happened at install/upgrade time).
    /// `None` when the blob is absent locally or is not a bundle.
    pub async fn bundle_manifest_for_blob(
        &self,
        blob_id: &BlobId,
    ) -> eyre::Result<Option<BundleManifest>> {
        let Some(blob_bytes) = self.get_blob_bytes(blob_id, None).await? else {
            return Ok(None);
        };
        if !Self::is_bundle_blob(&blob_bytes) {
            return Ok(None);
        }
        let manifest = tokio::task::spawn_blocking(move || {
            bundle::extract_manifest_allow_unsigned(&blob_bytes)
        })
        .await
        .map_err(|e| eyre::eyre!("bundle manifest read task failed: {e}"))??
        .1;
        Ok(Some(manifest))
    }

    /// Manifest `app_version` of the bundle blob at `blob_id`; `None` when
    /// the blob is absent locally, is not a bundle, or fails to parse —
    /// display-only, never an error.
    pub async fn blob_app_version(&self, blob_id: &BlobId) -> Option<String> {
        self.bundle_manifest_for_blob(blob_id)
            .await
            .ok()
            .flatten()
            .map(|m| m.app_version)
    }

    /// Every locally-retained bytecode version of `application_id`'s package:
    /// the application row's blob (latest fetched) plus every blob referenced
    /// by a group's `app_key` or a context's activation marker whose bundle
    /// manifest parses to the same package. Deduped by blob; blobs absent
    /// from the blobstore (or foreign packages) are skipped.
    pub async fn list_application_versions(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Vec<ApplicationVersionInfo>> {
        let Some(app) = self.get_application(application_id)? else {
            bail!("application '{}' not found", application_id);
        };
        let row_blob = app.blob.bytecode;

        // Candidate blob set: the row + group app_keys + activation markers.
        let mut candidates = std::collections::BTreeSet::new();
        let _ = candidates.insert(*row_blob.as_ref());
        {
            let handle = self.datastore.handle();
            // The Group column holds several prefixed key shapes; GroupMeta
            // (0x20) sorts first — seek there and stop at the first foreign
            // prefix (mirrors the governance-store enumeration helper, which
            // is not reachable from this crate without a dependency cycle).
            let mut iter = handle.iter::<key::GroupMeta>()?;
            let first = iter.seek(key::GroupMeta::new([0u8; 32])).transpose();
            let mut group_keys = Vec::new();
            for key_result in first.into_iter().chain(iter.keys()) {
                let group_key = key_result?;
                if group_key.as_key().as_bytes()[0] != key::GROUP_META_PREFIX {
                    break;
                }
                group_keys.push(group_key);
            }
            for group_key in group_keys {
                if let Some(meta) = handle.get(&group_key)? {
                    // Only this application's groups: a foreign group's
                    // app_key would otherwise be fetched + manifest-parsed
                    // just to be discarded by the package filter below.
                    if meta.target_application_id == *application_id {
                        let _ = candidates.insert(meta.app_key);
                    }
                }
            }
            let mut iter = handle.iter::<key::ContextActivatedBlob>()?;
            let mut marker_rows = Vec::new();
            for (k, v) in iter.entries() {
                let (k, marker) = (k?, v?);
                marker_rows.push((k.context_id(), marker.blob));
            }
            for (context_id, blob) in marker_rows {
                // Same cross-application guard: the marker row carries no app
                // id, but its context's meta does — one point-get beats a
                // blob fetch + parse for every foreign context.
                let same_app = handle
                    .get(&key::ContextMeta::new(context_id))?
                    .is_some_and(|c| c.application.application_id() == *application_id);
                if same_app {
                    let _ = candidates.insert(blob);
                }
            }
        }

        let mut versions = Vec::new();
        for candidate in candidates {
            if candidate == [0u8; 32] {
                continue;
            }
            let blob_id = BlobId::from(candidate);
            let Some(blob_bytes) = self.get_blob_bytes(&blob_id, None).await? else {
                continue; // referenced but not locally retained
            };
            let size = blob_bytes.len() as u64;
            if Self::is_bundle_blob(&blob_bytes) {
                let manifest = match tokio::task::spawn_blocking(move || {
                    bundle::extract_manifest_allow_unsigned(&blob_bytes)
                })
                .await
                {
                    Ok(Ok((_, manifest))) => manifest,
                    // Unparseable manifests are skipped, not fatal: foreign
                    // or corrupt blobs must not break the inventory.
                    Ok(Err(err)) => {
                        debug!(%blob_id, %err, "version inventory: skipping unparseable bundle manifest");
                        continue;
                    }
                    Err(err) => {
                        debug!(%blob_id, %err, "version inventory: manifest read task failed; skipping blob");
                        continue;
                    }
                };
                if manifest.package != app.package {
                    continue; // a different application's version blob
                }
                versions.push(ApplicationVersionInfo {
                    version: manifest.app_version,
                    blob_id,
                    size,
                    package: manifest.package,
                });
            } else if blob_id == row_blob {
                // Raw-wasm apps carry no manifest; the row's metadata is the
                // only version identity available.
                versions.push(ApplicationVersionInfo {
                    version: app.version.clone(),
                    blob_id,
                    size,
                    package: app.package.clone(),
                });
            }
        }

        // Newest first; non-semver strings sort lexicographically after.
        versions.sort_by(|a, b| {
            match (
                semver::Version::parse(&a.version),
                semver::Version::parse(&b.version),
            ) {
                (Ok(va), Ok(vb)) => vb.cmp(&va),
                (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                (Err(_), Err(_)) => b.version.cmp(&a.version),
            }
        });
        Ok(versions)
    }

    pub async fn application_bytes_from_blob(
        &self,
        blob_id: &BlobId,
        service_name: Option<&str>,
    ) -> eyre::Result<Option<Arc<[u8]>>> {
        let Some(blob_bytes) = self.get_blob_bytes(blob_id, None).await? else {
            return Ok(None);
        };
        if !Self::is_bundle_blob(&blob_bytes) {
            return Ok(Some(blob_bytes));
        }
        let service_owned = service_name.map(str::to_owned);
        let wasm = tokio::task::spawn_blocking(move || -> eyre::Result<Option<Vec<u8>>> {
            let (_, manifest) = bundle::extract_manifest_allow_unsigned(&blob_bytes)?;
            let wasm_relative_path = match service_owned.as_deref() {
                Some(name) => manifest
                    .services
                    .as_ref()
                    .and_then(|svcs| svcs.iter().find(|s| s.name == name))
                    .map(|s| s.wasm.path.clone())
                    .ok_or_else(|| {
                        eyre::eyre!("service '{}' not found in bundle manifest", name)
                    })?,
                None => manifest
                    .wasm
                    .as_ref()
                    .map(|w| w.path.clone())
                    .unwrap_or_else(|| "app.wasm".to_owned()),
            };
            if wasm_relative_path.contains("..") {
                bail!(
                    "WASM path traversal detected: {} contains '..' component",
                    wasm_relative_path
                );
            }
            bundle::extract_bundle_file(&blob_bytes, &wasm_relative_path)
        })
        .await??;
        Ok(wasm.map(Into::into))
    }
}
