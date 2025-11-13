//! Application installation functionality.
//!
//! This module handles installing applications from various sources:
//! - Local file paths
//! - HTTP/HTTPS URLs
//! - Bundle blobs (already in blobstore)
//! - Bundle archives from paths

use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use camino::Utf8PathBuf;
use eyre::bail;
use futures_util::{io::Cursor, TryStreamExt};
use reqwest::Url;
use std::io::{self, ErrorKind};
use tokio::fs::File;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{debug, trace, warn};

use crate::client::application::bundle;
use crate::client::NodeClient;

impl NodeClient {
    /// Install an application from a blob ID.
    ///
    /// This is the core installation function that creates ApplicationMeta entries.
    /// For bundles, ApplicationId is computed from (package, version).
    /// For single WASM files, ApplicationId is computed from (blob_id, size, source, metadata).
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
            ApplicationId::from(*calimero_primitives::hash::Hash::hash_borsh(&components)?)
        } else {
            // For single WASM: use current logic (blob_id, size, source, metadata)
            // Maintains backward compatibility for non-bundle installations
            let components = (
                application.bytecode,
                application.size,
                &application.source,
                &application.metadata,
            );
            ApplicationId::from(*calimero_primitives::hash::Hash::hash_borsh(&components)?)
        };

        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(application_id);

        handle.put(&key, &application)?;

        Ok(application_id)
    }

    /// Install an application from a local file path.
    ///
    /// Automatically detects bundles (.mpk files) vs single WASM files.
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
        if bundle::is_bundle_archive(&path) {
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

    /// Install an application from an HTTP/HTTPS URL.
    ///
    /// Automatically detects bundles (.mpk files) vs single WASM files.
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
                crate::client::application::bundle::extract_bundle_manifest(&bundle_data_clone)
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
                crate::client::application::bundle::extract_bundle_artifacts(
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
        let manifest = tokio::task::spawn_blocking(move || {
            crate::client::application::bundle::extract_bundle_manifest(&bundle_data_clone)
        })
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
            crate::client::application::bundle::extract_bundle_artifacts(
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

        Ok(application_id)
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
        let manifest = tokio::task::spawn_blocking(move || {
            crate::client::application::bundle::extract_bundle_manifest(&bundle_bytes_clone)
        })
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
            crate::client::application::bundle::extract_bundle_artifacts(
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

    /// Install application by package and version
    ///
    /// Note: Currently uses the source URL to download the application.
    /// In a real implementation, you might want to resolve the package/version to a URL.
    pub async fn install_by_package_version(
        &self,
        _package: &str,
        _version: &str,
        source: &ApplicationSource,
        metadata: Vec<u8>,
    ) -> eyre::Result<ApplicationId> {
        let url = source.to_string().parse()?;
        self.install_application_from_url(url, metadata, None).await
    }
}
