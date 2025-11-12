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

        // Check if this is a bundle installation
        let metadata_json: Result<serde_json::Value, _> =
            serde_json::from_slice(&application.metadata);
        if let Ok(meta) = metadata_json {
            if meta
                .get("bundle")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                // This is a bundle, extract WASM from extracted directory
                if let Some(extract_dir_str) = meta.get("extract_dir").and_then(|v| v.as_str()) {
                    // Resolve relative path against node root
                    let blobstore_root = self.blobstore.root_path();
                    let node_root = blobstore_root
                        .parent()
                        .ok_or_else(|| eyre::eyre!("blobstore root has no parent"))?;
                    let extract_dir = node_root.join(extract_dir_str);
                    let wasm_path = extract_dir.join("app.wasm");

                    if wasm_path.exists() {
                        let wasm_bytes = tokio::fs::read(&wasm_path).await?;
                        return Ok(Some(wasm_bytes.into()));
                    } else {
                        // Fallback: re-extract from bundle blob if extracted files missing
                        warn!(
                            wasm_path = %wasm_path,
                            "extracted WASM not found, attempting to re-extract from bundle"
                        );

                        // Get bundle blob and re-extract
                        let Some(bundle_bytes) = self
                            .get_blob_bytes(&application.bytecode.blob_id(), None)
                            .await?
                        else {
                            bail!("fatal: bundle blob not found");
                        };

                        // Parse manifest to find WASM path
                        let manifest: BundleManifest =
                            Self::extract_bundle_manifest(&bundle_bytes)?;
                        if let Some(wasm_artifact) = manifest.wasm {
                            // Extract WASM from bundle
                            let tar = GzDecoder::new(bundle_bytes.as_ref());
                            let mut archive = Archive::new(tar);

                            for entry in archive.entries()? {
                                let mut entry = entry?;
                                let path = entry.path()?;
                                let file_name =
                                    path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                                if file_name == wasm_artifact.path || file_name == "app.wasm" {
                                    let mut wasm_content = Vec::new();
                                    entry.read_to_end(&mut wasm_content)?;
                                    return Ok(Some(wasm_content.into()));
                                }
                            }
                        }

                        bail!("WASM file not found in bundle");
                    }
                }
            }
        }

        // Single WASM installation (existing behavior)
        let Some(bytes) = self
            .get_blob_bytes(&application.bytecode.blob_id(), None)
            .await?
        else {
            bail!("fatal: application points to dangling blob");
        };

        Ok(Some(bytes))
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

        let application_id = {
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

    /// Check if a path points to a bundle archive (tar.gz)
    fn is_bundle_archive(path: &Utf8Path) -> bool {
        path.extension()
            .map(|ext| matches!(ext, "gz" | "tgz"))
            .unwrap_or(false)
            || path.as_str().ends_with(".tar.gz")
    }

    pub async fn install_application_from_path(
        &self,
        path: Utf8PathBuf,
        metadata: Vec<u8>,
        package: &str,
        version: &str,
    ) -> eyre::Result<ApplicationId> {
        let metadata_len = metadata.len();
        debug!(
            path = %path,
            package,
            version,
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
            return self
                .install_bundle_from_path(path, metadata, package, version)
                .await;
        }

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
        )
    }

    pub async fn install_application_from_url(
        &self,
        url: Url,
        metadata: Vec<u8>,
        expected_hash: Option<&Hash>,
        package: &str,
        version: &str,
    ) -> eyre::Result<ApplicationId> {
        let uri = url.as_str().parse()?;

        let response = reqwest::Client::new().get(url.clone()).send().await?;

        let expected_size = response.content_length();

        // Check if URL indicates a bundle archive
        let is_bundle = url.path().ends_with(".tar.gz")
            || url.path().ends_with(".tgz")
            || url.path().ends_with(".gz")
            || response
                .headers()
                .get("content-type")
                .and_then(|ct| ct.to_str().ok())
                .map(|ct| ct.contains("gzip") || ct.contains("tar"))
                .unwrap_or(false);

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
            let manifest = Self::extract_bundle_manifest(&bundle_data)?;

            // Validate manifest matches provided package/version
            if manifest.package != package {
                bail!(
                    "manifest package '{}' does not match provided package '{}'",
                    manifest.package,
                    package
                );
            }
            if manifest.app_version != version {
                bail!(
                    "manifest version '{}' does not match provided version '{}'",
                    manifest.app_version,
                    version
                );
            }

            // Extract artifacts with deduplication
            // Use node root (parent of blobstore) instead of blobstore root
            let blobstore_root = self.blobstore.root_path();
            let node_root = blobstore_root
                .parent()
                .ok_or_else(|| eyre::eyre!("blobstore root has no parent"))?;
            // Use relative path for extract_dir so all nodes get the same ApplicationId
            let extract_dir_relative = format!("applications/{}/{}/extracted", package, version);
            let extract_dir = node_root.join(&extract_dir_relative);

            Self::extract_bundle_artifacts(
                &bundle_data,
                &manifest,
                &extract_dir,
                node_root,
                package,
                version,
            )?;

            // Store bundle manifest in metadata
            // Use relative path so ApplicationId is consistent across nodes
            let bundle_metadata = serde_json::to_vec(&serde_json::json!({
                "bundle": true,
                "manifest": manifest,
                "extract_dir": extract_dir_relative,
            }))?;

            // Combine user metadata with bundle metadata
            let combined_metadata = if metadata.is_empty() {
                bundle_metadata
            } else {
                let mut combined = bundle_metadata;
                combined.extend_from_slice(&metadata);
                combined
            };

            // Install application with bundle blob_id
            return self.install_application(
                &bundle_blob_id,
                stored_size,
                &uri,
                combined_metadata,
                package,
                version,
            );
        }

        // Single WASM installation (existing behavior)
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

        self.install_application(&blob_id, size, &uri, metadata, package, version)
    }

    /// Install a bundle archive (tar.gz) containing WASM, ABI, and migrations
    async fn install_bundle_from_path(
        &self,
        path: Utf8PathBuf,
        metadata: Vec<u8>,
        package: &str,
        version: &str,
    ) -> eyre::Result<ApplicationId> {
        debug!(
            path = %path,
            package,
            version,
            "install_bundle_from_path started"
        );

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
        let manifest = Self::extract_bundle_manifest(&bundle_data)?;

        // Validate manifest matches provided package/version
        if manifest.package != package {
            bail!(
                "manifest package '{}' does not match provided package '{}'",
                manifest.package,
                package
            );
        }
        if manifest.app_version != version {
            bail!(
                "manifest version '{}' does not match provided version '{}'",
                manifest.app_version,
                version
            );
        }

        // Extract artifacts with deduplication
        // Use node root (parent of blobstore) instead of blobstore root
        let blobstore_root = self.blobstore.root_path();
        let node_root = blobstore_root
            .parent()
            .ok_or_else(|| eyre::eyre!("blobstore root has no parent"))?;
        // Use relative path for extract_dir so all nodes get the same ApplicationId
        let extract_dir_relative = format!("applications/{}/{}/extracted", package, version);
        let extract_dir = node_root.join(&extract_dir_relative);

        Self::extract_bundle_artifacts(
            &bundle_data,
            &manifest,
            &extract_dir,
            node_root,
            package,
            version,
        )?;

        // Store bundle manifest in metadata
        // Use relative path so ApplicationId is consistent across nodes
        let bundle_metadata = serde_json::to_vec(&serde_json::json!({
            "bundle": true,
            "manifest": manifest,
            "extract_dir": extract_dir_relative,
        }))?;

        // Combine user metadata with bundle metadata
        let combined_metadata = if metadata.is_empty() {
            bundle_metadata
        } else {
            // Prepend bundle metadata to user metadata
            let mut combined = bundle_metadata;
            combined.extend_from_slice(&metadata);
            combined
        };

        let Ok(uri) = Url::from_file_path(path) else {
            bail!("non-absolute path")
        };

        // Install application with bundle blob_id
        self.install_application(
            &bundle_blob_id,
            stored_size,
            &uri.as_str().parse()?,
            combined_metadata,
            package,
            version,
        )
    }

    /// Extract and parse bundle manifest from tar.gz data
    fn extract_bundle_manifest(bundle_data: &[u8]) -> eyre::Result<BundleManifest> {
        let tar = GzDecoder::new(bundle_data);
        let mut archive = Archive::new(tar);

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;

            if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
                let mut manifest_json = String::new();
                entry.read_to_string(&mut manifest_json)?;
                return serde_json::from_str(&manifest_json)
                    .map_err(|e| eyre::eyre!("failed to parse manifest.json: {}", e));
            }
        }

        bail!("manifest.json not found in bundle")
    }

    /// Find duplicate artifact in other versions by hash
    fn find_duplicate_artifact(
        node_root: &Utf8Path,
        package: &str,
        current_version: &str,
        hash: &[u8; 32],
        file_name: &str,
    ) -> Option<Utf8PathBuf> {
        // Check other versions for the same hash
        // For now, we'll check all versions in the package directory
        let package_dir = node_root.join("applications").join(package);

        if let Ok(entries) = fs::read_dir(package_dir.as_std_path()) {
            for entry in entries.flatten() {
                if let Ok(version_name) = entry.file_name().into_string() {
                    if version_name == current_version {
                        continue; // Skip current version
                    }

                    // Check extracted directory in this version
                    let extracted_dir = package_dir.join(&version_name).join("extracted");
                    let candidate_path = extracted_dir.join(file_name);

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
    fn extract_bundle_artifacts(
        bundle_data: &[u8],
        _manifest: &BundleManifest,
        extract_dir: &Utf8Path,
        blobstore_root: &Utf8Path,
        package: &str,
        current_version: &str,
    ) -> eyre::Result<()> {
        // Create extraction directory
        fs::create_dir_all(extract_dir)?;

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

            // Skip manifest.json (already parsed)
            if relative_path == "manifest.json" {
                continue;
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

            // Get file name for deduplication (use just the filename, not full path)
            let file_name = std::path::Path::new(&relative_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&relative_path);

            // Check for duplicates in other versions
            if let Some(duplicate_path) = Self::find_duplicate_artifact(
                blobstore_root,
                package,
                current_version,
                &hash_array,
                file_name,
            ) {
                // Create hardlink to duplicate file
                if let Err(e) = fs::hard_link(duplicate_path.as_std_path(), dest_path.as_std_path())
                {
                    // If hardlink fails (e.g., cross-filesystem), fall back to copying
                    warn!(
                        file = %file_name,
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

        Ok(())
    }

    pub fn uninstall_application(&self, application_id: &ApplicationId) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        handle.delete(&key)?;

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
                let version = app.version.to_string();
                match &latest_version {
                    None => latest_version = Some((version, id.application_id())),
                    Some((current_version, _)) => {
                        if version > *current_version {
                            latest_version = Some((version, id.application_id()));
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
        package: &str,
        version: &str,
        source: &ApplicationSource,
        metadata: Vec<u8>,
    ) -> eyre::Result<ApplicationId> {
        // For now, we'll use the source URL to download the application
        // In a real implementation, you might want to resolve the package/version to a URL
        let url = source.to_string().parse()?;
        self.install_application_from_url(url, metadata, None, package, version)
            .await
    }
}
