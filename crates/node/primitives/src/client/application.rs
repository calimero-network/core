use std::io::{self, ErrorKind, Read};
use std::sync::Arc;

use crate::bundle::BundleManifest;
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
use semver::Version;
use serde_json;
use sha2::{Digest, Sha256};
use std::fs;
use tar::Archive;
use tokio::fs::File;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{debug, trace, warn};

use super::NodeClient;

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

    pub async fn get_application_bytes(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<Arc<[u8]>>> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        let Some(application) = handle.get(&key)? else {
            return Ok(None);
        };

        // Get blob bytes to check if it's a bundle
        let Some(blob_bytes) = self
            .get_blob_bytes(&application.bytecode.blob_id(), None)
            .await?
        else {
            bail!("fatal: application points to dangling blob");
        };

        // Check if this is a bundle by inspecting the blob (blocking I/O wrapped in spawn_blocking)
        let blob_bytes_clone = blob_bytes.clone();
        let is_bundle =
            tokio::task::spawn_blocking(move || Self::is_bundle_blob(&blob_bytes_clone)).await?;

        if is_bundle {
            // This is a bundle, extract WASM from extracted directory or bundle
            // Extract manifest (blocking I/O wrapped in spawn_blocking)
            let blob_bytes_clone = blob_bytes.clone();
            let manifest = tokio::task::spawn_blocking(move || {
                Self::extract_bundle_manifest(&blob_bytes_clone)
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
            let wasm_path = extract_dir.join(wasm_relative_path);

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

    pub fn install_application(
        &self,
        blob_id: &BlobId,
        size: u64,
        source: &ApplicationSource,
        metadata: Vec<u8>,
        package: &str,
        version: &str,
        is_bundle: bool,
    ) -> eyre::Result<ApplicationId> {
        let application = types::ApplicationMeta::new(
            key::BlobMeta::new(*blob_id),
            size,
            source.to_string().into_boxed_str(),
            metadata.into_boxed_slice(),
            key::BlobMeta::new(BlobId::from([0; 32])),
            package.to_owned().into_boxed_str(),
            version.to_owned().into_boxed_str(),
        );

        let application_id = if is_bundle {
            // For bundles: use only package and version for deterministic ApplicationId
            // This allows overwriting apps and supports multi-version installations
            let components = (&application.package, &application.version);
            ApplicationId::from(*Hash::hash_borsh(&components)?)
        } else {
            // For single WASM: use current logic (blob_id, size, source, metadata)
            // Maintains backward compatibility for non-bundle installations
            let components = (
                application.bytecode,
                application.size,
                &application.source,
                &application.metadata,
            );
            ApplicationId::from(*Hash::hash_borsh(&components)?)
        };

        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(application_id);

        handle.put(&key, &application)?;

        Ok(application_id)
    }

    /// Check if a path points to a bundle archive (.mpk - Mero Package Kit)
    fn is_bundle_archive(path: &Utf8Path) -> bool {
        path.extension().map(|ext| ext == "mpk").unwrap_or(false)
    }

    pub async fn install_application_from_path(
        &self,
        path: Utf8PathBuf,
        metadata: Vec<u8>,
    ) -> eyre::Result<ApplicationId> {
        let metadata_len = metadata.len();
        debug!(
            path = %path,
            metadata_len,
            "install_application_from_path started"
        );

        let path = match path.canonicalize_utf8() {
            Ok(canonicalized) => canonicalized,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                bail!("application file not found at {}", path);
            }
            Err(err) => return Err(err.into()),
        };
        trace!(path = %path, "application path canonicalized");

        // Detect bundle vs single WASM
        if Self::is_bundle_archive(&path) {
            return self.install_bundle_from_path(path, metadata).await;
        }

        // For non-bundle installations, use defaults (package/version are not part of ApplicationId)
        let package = "unknown";
        let version = "0.0.0";

        // Existing single WASM installation path
        let file = match File::open(&path).await {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                bail!("application file not found at {}", path);
            }
            Err(err) => return Err(err.into()),
        };
        trace!(path = %path, "application file opened");

        let expected_size = file.metadata().await?.len();
        debug!(
            path = %path,
            expected_size,
            "install_application_from_path discovered file size"
        );

        let (blob_id, size) = self
            .add_blob(file.compat(), Some(expected_size), None)
            .await?;
        debug!(
            %blob_id,
            expected_size,
            stored_size = size,
            "application blob added via add_blob"
        );

        let Ok(uri) = Url::from_file_path(path) else {
            bail!("non-absolute path")
        };

        self.install_application(
            &blob_id,
            size,
            &uri.as_str().parse()?,
            metadata,
            package,
            version,
            false, // is_bundle: false for single WASM
        )
    }

    pub async fn install_application_from_url(
        &self,
        url: Url,
        metadata: Vec<u8>,
        expected_hash: Option<&Hash>,
    ) -> eyre::Result<ApplicationId> {
        let uri = url.as_str().parse()?;

        let response = reqwest::Client::new().get(url.clone()).send().await?;

        let expected_size = response.content_length();

        // Check if URL indicates a bundle archive (.mpk - Mero Package Kit)
        let is_bundle = url.path().ends_with(".mpk");

        if is_bundle {
            // Download entire bundle into memory
            let bundle_data = response.bytes().await?.to_vec();

            // Store entire bundle as a single blob
            let cursor = Cursor::new(bundle_data.as_slice());
            let (bundle_blob_id, stored_size) = self
                .add_blob(cursor, Some(bundle_data.len() as u64), expected_hash)
                .await?;

            debug!(
                %bundle_blob_id,
                bundle_size = bundle_data.len(),
                stored_size,
                "bundle downloaded and stored as blob"
            );

            // Extract bundle to parse manifest and extract artifacts
            // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
            let bundle_data_clone = bundle_data.clone();
            let manifest = tokio::task::spawn_blocking(move || {
                Self::extract_bundle_manifest(&bundle_data_clone)
            })
            .await??;

            // Extract package and version from manifest
            let package = &manifest.package;
            let version = &manifest.app_version;

            // Extract artifacts with deduplication
            // Use node root (parent of blobstore) instead of blobstore root
            // Must be done before spawn_blocking
            let blobstore_root = self.blobstore.root_path();
            let node_root = blobstore_root
                .parent()
                .ok_or_else(|| eyre::eyre!("blobstore root has no parent"))?
                .to_path_buf();
            // Extract directory is derived from package and version
            let extract_dir = node_root
                .join("applications")
                .join(package)
                .join(version)
                .join("extracted");

            // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
            let bundle_data_clone = bundle_data.clone();
            let manifest_clone = manifest.clone();
            let extract_dir_clone = extract_dir.clone();
            let node_root_clone = node_root.clone();
            let package_clone = package.to_string();
            let version_clone = version.to_string();
            tokio::task::spawn_blocking(move || {
                Self::extract_bundle_artifacts(
                    &bundle_data_clone,
                    &manifest_clone,
                    &extract_dir_clone,
                    &node_root_clone,
                    &package_clone,
                    &version_clone,
                )
            })
            .await??;

            // Install application with bundle blob_id
            // No metadata needed - bundle detection happens via is_bundle_blob()
            return self.install_application(
                &bundle_blob_id,
                stored_size,
                &uri,
                vec![], // Empty metadata - bundles don't need metadata
                package,
                version,
                true, // is_bundle: true for bundles
            );
        }

        // Single WASM installation (existing behavior)
        // For non-bundle installations, use defaults (package/version are not part of ApplicationId)
        let package = "unknown";
        let version = "0.0.0";

        let (blob_id, size) = self
            .add_blob(
                response
                    .bytes_stream()
                    .map_err(io::Error::other)
                    .into_async_read(),
                expected_size,
                expected_hash,
            )
            .await?;

        self.install_application(&blob_id, size, &uri, metadata, package, version, false)
        // is_bundle: false for single WASM
    }

    /// Install a bundle archive (.mpk - Mero Package Kit) containing WASM, ABI, and migrations
    /// Note: metadata parameter is ignored for bundles - metadata is always extracted from manifest
    async fn install_bundle_from_path(
        &self,
        path: Utf8PathBuf,
        _metadata: Vec<u8>,
    ) -> eyre::Result<ApplicationId> {
        debug!(
            path = %path,
            "install_bundle_from_path started"
        );

        // Clone path for deletion after installation
        let bundle_path = path.clone();

        // Read bundle file
        let bundle_data = tokio::fs::read(&path).await?;
        let bundle_size = bundle_data.len() as u64;

        // Store entire bundle as a single blob
        let cursor = Cursor::new(bundle_data.as_slice());
        let (bundle_blob_id, stored_size) = self.add_blob(cursor, Some(bundle_size), None).await?;

        debug!(
            %bundle_blob_id,
            bundle_size,
            stored_size,
            "bundle stored as blob"
        );

        // Extract bundle to parse manifest and extract artifacts
        // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
        let bundle_data_clone = bundle_data.clone();
        let manifest =
            tokio::task::spawn_blocking(move || Self::extract_bundle_manifest(&bundle_data_clone))
                .await??;

        // Extract package and version from manifest (ignore provided values)
        let package = &manifest.package;
        let version = &manifest.app_version;

        // Extract artifacts with deduplication
        // Use node root (parent of blobstore) instead of blobstore root
        // Must be done before spawn_blocking
        let blobstore_root = self.blobstore.root_path();
        let node_root = blobstore_root
            .parent()
            .ok_or_else(|| eyre::eyre!("blobstore root has no parent"))?
            .to_path_buf();
        // Extract directory is derived from package and version
        let extract_dir = node_root
            .join("applications")
            .join(package)
            .join(version)
            .join("extracted");

        // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
        let bundle_data_clone = bundle_data.clone();
        let manifest_clone = manifest.clone();
        let extract_dir_clone = extract_dir.clone();
        let node_root_clone = node_root.clone();
        let package_clone = package.to_string();
        let version_clone = version.to_string();
        tokio::task::spawn_blocking(move || {
            Self::extract_bundle_artifacts(
                &bundle_data_clone,
                &manifest_clone,
                &extract_dir_clone,
                &node_root_clone,
                &package_clone,
                &version_clone,
            )
        })
        .await??;

        let Ok(uri) = Url::from_file_path(path) else {
            bail!("non-absolute path")
        };

        // Install application with bundle blob_id
        // No metadata needed - bundle detection happens via is_bundle_blob()
        let application_id = self.install_application(
            &bundle_blob_id,
            stored_size,
            &uri.as_str().parse()?,
            vec![], // Empty metadata - bundles don't need metadata
            package,
            version,
            true, // is_bundle: true for bundles
        )?;

        // Delete bundle file after successful installation (it's now stored as a blob)
        // Only attempt deletion if file exists to avoid "No such file" errors
        if bundle_path.exists() {
            if let Err(e) = tokio::fs::remove_file(&bundle_path).await {
                warn!(
                    path = %bundle_path,
                    error = %e,
                    "Failed to delete bundle file after installation"
                );
                // Don't fail installation if deletion fails - bundle is already installed
            } else {
                debug!(
                    path = %bundle_path,
                    "Deleted bundle file after successful installation"
                );
            }
        } else {
            debug!(
                path = %bundle_path,
                "Bundle file already removed or doesn't exist, skipping deletion"
            );
        }

        Ok(application_id)
    }

    /// Extract and parse bundle manifest from bundle archive data
    fn extract_bundle_manifest(bundle_data: &[u8]) -> eyre::Result<BundleManifest> {
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

    /// Check if a blob contains a bundle archive by peeking at the first few entries.
    /// This is a lightweight check that only reads the archive structure, not the full content.
    pub fn is_bundle_blob(blob_bytes: &[u8]) -> bool {
        // Quick check: try to parse as gzip/tar and look for manifest.json
        let tar = GzDecoder::new(blob_bytes);
        let mut archive = Archive::new(tar);

        // Only check first 10 entries to avoid reading entire archive
        if let Ok(entries) = archive.entries() {
            for (i, entry_result) in entries.enumerate() {
                if i >= 10 {
                    break; // Give up after 10 entries
                }
                if let Ok(entry) = entry_result {
                    if let Ok(path) = entry.path() {
                        if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Install an application from a bundle blob that's already in the blobstore.
    /// This is used when a bundle blob is received via blob sharing or discovery.
    /// No metadata needed - bundle detection happens via is_bundle_blob()
    pub async fn install_application_from_bundle_blob(
        &self,
        blob_id: &BlobId,
        source: &ApplicationSource,
    ) -> eyre::Result<ApplicationId> {
        debug!(
            %blob_id,
            "install_application_from_bundle_blob started"
        );

        // Get bundle bytes from blobstore
        let Some(bundle_bytes) = self.get_blob_bytes(blob_id, None).await? else {
            bail!("bundle blob not found");
        };

        // Extract manifest for package/version (needed for extraction path)
        // No metadata needed - bundle detection happens via is_bundle_blob()
        // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
        let bundle_bytes_clone = bundle_bytes.clone();
        let manifest =
            tokio::task::spawn_blocking(move || Self::extract_bundle_manifest(&bundle_bytes_clone))
                .await??;
        let package = &manifest.package;
        let version = &manifest.app_version;

        // Extract artifacts with deduplication
        // Must be done before spawn_blocking
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

        // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
        let bundle_bytes_clone = bundle_bytes.clone();
        let manifest_clone = manifest.clone();
        let extract_dir_clone = extract_dir.clone();
        let node_root_clone = node_root.clone();
        let package_clone = package.to_string();
        let version_clone = version.to_string();
        tokio::task::spawn_blocking(move || {
            Self::extract_bundle_artifacts(
                &bundle_bytes_clone,
                &manifest_clone,
                &extract_dir_clone,
                &node_root_clone,
                &package_clone,
                &version_clone,
            )
        })
        .await??;
        let size = bundle_bytes.len() as u64;

        debug!(
            %blob_id,
            package,
            version,
            size,
            "bundle extracted and ready for installation"
        );

        // Install application
        // No metadata needed - bundle detection happens via is_bundle_blob()
        self.install_application(blob_id, size, source, vec![], package, version, true)
        // is_bundle: true for bundles
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
    fn extract_bundle_artifacts(
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
            let wasm_path = extract_dir.join(wasm_relative_path);

            if wasm_path.exists() {
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

            // Create parent directories if needed
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }

            // Compute hash
            let hash = Sha256::digest(&content);
            let hash_array: [u8; 32] = hash.into();

            // Check for duplicates in other versions at the same relative path
            if let Some(duplicate_path) = Self::find_duplicate_artifact(
                node_root,
                package,
                current_version,
                &hash_array,
                &relative_path,
            ) {
                // Create hardlink to duplicate file
                if let Err(e) = fs::hard_link(duplicate_path.as_std_path(), dest_path.as_std_path())
                {
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

        // Remove lock file
        let _ = fs::remove_file(&lock_file_path);

        Ok(())
    }

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
                    debug!(
                        package = %application.package,
                        version = %application.version,
                        path = %bundle_dir,
                        "Removing extracted bundle directory"
                    );
                    if let Err(e) = fs::remove_dir_all(bundle_dir.as_std_path()) {
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
                        if let Ok(mut entries) = fs::read_dir(package_dir.as_std_path()) {
                            if entries.next().is_none() {
                                // Directory is empty, remove it
                                if let Err(e) = fs::remove_dir(package_dir.as_std_path()) {
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

    pub fn list_applications(&self) -> eyre::Result<Vec<Application>> {
        let handle = self.datastore.handle();

        let mut iter = handle.iter::<key::ApplicationMeta>()?;

        let mut applications = vec![];

        for (id, app) in iter.entries() {
            let (id, app) = (id?, app?);
            applications.push(Application::new(
                id.application_id(),
                ApplicationBlob {
                    bytecode: app.bytecode.blob_id(),
                    compiled: app.compiled.blob_id(),
                },
                app.size,
                app.source.parse()?,
                app.metadata.to_vec(),
            ));
        }

        Ok(applications)
    }

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

    /// List all packages
    pub fn list_packages(&self) -> eyre::Result<Vec<String>> {
        let handle = self.datastore.handle();
        let mut iter = handle.iter::<key::ApplicationMeta>()?;
        let mut packages = std::collections::HashSet::new();

        for (id, app) in iter.entries() {
            let (_, app) = (id?, app?);
            let _ = packages.insert(app.package.to_string());
        }

        Ok(packages.into_iter().collect())
    }

    /// List all versions of a package
    pub fn list_versions(&self, package: &str) -> eyre::Result<Vec<String>> {
        let handle = self.datastore.handle();
        let mut iter = handle.iter::<key::ApplicationMeta>()?;
        let mut versions = Vec::new();

        for (id, app) in iter.entries() {
            let (_, app) = (id?, app?);
            if app.package.as_ref() == package {
                versions.push(app.version.to_string());
            }
        }

        Ok(versions)
    }

    /// Get the latest version of a package
    pub fn get_latest_version(&self, package: &str) -> eyre::Result<Option<ApplicationId>> {
        let handle = self.datastore.handle();
        let mut iter = handle.iter::<key::ApplicationMeta>()?;
        let mut latest_version: Option<(String, ApplicationId)> = None;

        for (id, app) in iter.entries() {
            let (id, app) = (id?, app?);
            if app.package.as_ref() == package {
                let version_str = app.version.to_string();
                match &latest_version {
                    None => latest_version = Some((version_str, id.application_id())),
                    Some((current_version_str, _)) => {
                        // Try semantic version comparison first
                        let is_newer = match (
                            Version::parse(&version_str),
                            Version::parse(current_version_str),
                        ) {
                            (Ok(new_version), Ok(current_version)) => {
                                // Both are valid semantic versions - use proper comparison
                                new_version > current_version
                            }
                            (Ok(_), Err(_)) => {
                                // New version is valid semver, current is not - prefer semver
                                true
                            }
                            (Err(_), Ok(_)) => {
                                // Current version is valid semver, new is not - keep current
                                false
                            }
                            (Err(_), Err(_)) => {
                                // Neither is valid semver - fall back to lexicographic comparison
                                version_str > *current_version_str
                            }
                        };

                        if is_newer {
                            latest_version = Some((version_str, id.application_id()));
                        }
                    }
                }
            }
        }

        Ok(latest_version.map(|(_, id)| id))
    }

    /// Install application by package and version
    pub async fn install_by_package_version(
        &self,
        _package: &str,
        _version: &str,
        source: &ApplicationSource,
        metadata: Vec<u8>,
    ) -> eyre::Result<ApplicationId> {
        // For now, we'll use the source URL to download the application
        // In a real implementation, you might want to resolve the package/version to a URL
        let url = source.to_string().parse()?;
        self.install_application_from_url(url, metadata, None).await
    }
}
