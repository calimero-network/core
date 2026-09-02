//! Application installation and uninstallation.

use std::io::ErrorKind;
use std::sync::Arc;

use super::bundle;
use calimero_app_downloader::app_source;
use calimero_app_downloader::registry::RegistryCoords;
use calimero_app_downloader::registry::PENDING_BLOB_SHARE_SOURCE;
use calimero_app_downloader::source::AppRequest;
use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_store::{key, types};
use camino::Utf8PathBuf;
use eyre::bail;
use futures_util::io::Cursor;
use reqwest::Url;
use tracing::{debug, trace};

use crate::client::NodeClient;

// A payload that is no bundle at all, as opposed to one that fails to verify.
const NOT_A_BUNDLE: &str = "not a signed application bundle";

impl NodeClient {
    fn install_bundle_application(
        &self,
        blob_id: &BlobId,
        size: u64,
        source: &ApplicationSource,
        metadata: Vec<u8>,
        info: types::PackageInfo,
        services: Vec<types::ServiceMeta>,
    ) -> eyre::Result<ApplicationId> {
        let mut application = types::ApplicationMeta::new(
            key::BlobMeta::new(*blob_id),
            size,
            source.to_string().into_boxed_str(),
            metadata.into_boxed_slice(),
            key::BlobMeta::new(BlobId::from([0; 32])),
            info,
        );
        application.services = services;

        let application_id =
            ApplicationId::for_bundle(&application.package, &application.signer_id)?;

        // Bundle ids are version-stable (hash(package, signer)), so a new
        // version overwrites the row in place. The row is a download-cache
        // pointer ("latest fetched"); what a context executes is decided by
        // its per-context binding (activation marker / group bytecode_id), so no
        // displaced-blob breadcrumb is needed.
        let mut handle = self.datastore.handle();
        let key = key::ApplicationMeta::new(application_id);
        handle.put(&key, &application)?;
        Ok(application_id)
    }

    /// Install a `.mpk`. The signature is mandatory and every wasm artifact is
    /// digest-checked before any bytes are stored.
    ///
    /// `expected` is what the caller asked for, on coordinate-addressed paths.
    pub(super) async fn install_bundle(
        &self,
        bundle_data: Arc<[u8]>,
        blob_id: &BlobId,
        stored_size: u64,
        source: &ApplicationSource,
        expected: Option<RegistryCoords<'_>>,
    ) -> eyre::Result<ApplicationId> {
        // Every artifact, including the unnamed single-service one that gets no
        // blob of its own, so substituted bytes are refused before first execution.
        let (verified, wasm) = tokio::task::spawn_blocking(move || -> eyre::Result<_> {
            let verified = bundle::VerifiedBundle::open(bundle_data)?;
            let wasm = verified.all_wasm()?;
            Ok((verified, wasm))
        })
        .await??;

        // Otherwise the application row is written with no bytecode and the
        // bundle only reads as malformed at its first execution.
        if wasm.is_empty() {
            bail!("bundle manifest declares no wasm artifact: expected a top-level 'wasm' or a non-empty 'services' list");
        }

        let manifest = verified.manifest();
        let package = &manifest.package;
        let version = &manifest.app_version;

        // The coordinates are the only promise a bare install makes: another
        // package signs just as validly, and an id is stable across versions.
        if let Some(expected) = expected {
            if package != expected.package || version != expected.version {
                bail!(
                    "registry artifact is {package}@{version}, not the {}@{} that was requested",
                    expected.package,
                    expected.version
                );
            }
        }

        let mut services = Vec::new();
        let mut max_state_version = 0;
        for artifact in &wasm {
            // Before the name guard: an unnamed single-service artifact still
            // carries a state version.
            if let Some(schema) =
                calimero_wasm_abi::embed::read_embedded_state_schema(&artifact.bytes)
            {
                max_state_version = max_state_version.max(schema.state_version_or_default());
            }
            let Some(name) = &artifact.name else { continue };
            let (svc_blob_id, _svc_size) = self
                .add_blob(
                    Cursor::new(&artifact.bytes[..]),
                    Some(artifact.bytes.len() as u64),
                    None,
                )
                .await?;
            services.push(types::ServiceMeta {
                name: name.as_str().into(),
                bytecode: key::BlobMeta::new(svc_blob_id),
                compiled: key::BlobMeta::new(BlobId::from([0; 32])),
            });
        }

        self.install_bundle_application(
            blob_id,
            stored_size,
            source,
            manifest.to_metadata_json()?,
            types::PackageInfo {
                package: package.as_str().into(),
                version: version.as_str().into(),
                signer_id: verified.signer_id().into(),
                state_version: max_state_version,
            },
            services,
        )
    }

    /// Install a signed `.mpk` from disk. The id derives from the manifest's
    /// (package, signer), so a payload without one has no re-derivable id.
    pub async fn install_application_from_path(
        &self,
        path: Utf8PathBuf,
    ) -> eyre::Result<ApplicationId> {
        debug!(path = %path, "install_application_from_path started");

        let path = match path.canonicalize_utf8() {
            Ok(canonicalized) => canonicalized,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                bail!("application file not found at {}", path);
            }
            Err(err) => return Err(err.into()),
        };
        trace!(path = %path, "application path canonicalized");

        let bundle_data: Arc<[u8]> = tokio::fs::read(&path).await?.into();
        if !bundle::is_bundle_blob(&bundle_data) {
            bail!("{NOT_A_BUNDLE}: {path}");
        }

        let Ok(uri) = Url::from_file_path(&path) else {
            bail!("non-absolute path")
        };

        self.store_and_install_bundle(bundle_data, &uri.as_str().parse()?, None)
            .await
    }

    /// Install `package@version` from this node's one source; `Ok(None)` means
    /// nothing published there. The manifest decides the id, so it is checked.
    pub async fn install_by_coords(
        &self,
        package: &str,
        version: &str,
    ) -> eyre::Result<Option<ApplicationId>> {
        let source = app_source(&self.registry_config(), self.clone())?;
        let req = AppRequest {
            application_id: None,
            package,
            version,
            bytecode_id: None,
            context_id: None,
        };
        let Some(bundle_data) = source.fetch(&req).await? else {
            return Ok(None);
        };
        if !bundle::is_bundle_blob(&bundle_data) {
            bail!("{NOT_A_BUNDLE}: {package}@{version}");
        }

        // The marker, not this node's own base_url: a joiner resolves the app
        // from its OWN registry, addressed by the coordinates the row records.
        self.store_and_install_bundle(
            bundle_data,
            &PENDING_BLOB_SHARE_SOURCE.parse()?,
            Some(RegistryCoords::new(package, version)),
        )
        .await
        .map(Some)
    }

    /// Store the archive as a blob, then install it. The blob is written before
    /// the manifest is opened, so an install that fails must take it back out.
    async fn store_and_install_bundle(
        &self,
        bundle_data: Arc<[u8]>,
        source: &ApplicationSource,
        expected: Option<RegistryCoords<'_>>,
    ) -> eyre::Result<ApplicationId> {
        let cursor = Cursor::new(&bundle_data[..]);
        let (bundle_blob_id, stored_size) = self
            .add_blob(cursor, Some(bundle_data.len() as u64), None)
            .await?;

        debug!(
            %bundle_blob_id,
            bundle_size = bundle_data.len(),
            stored_size,
            "bundle stored as blob"
        );

        let installed = self
            .install_bundle(bundle_data, &bundle_blob_id, stored_size, source, expected)
            .await;
        self.release_blob_on_error(bundle_blob_id, installed).await
    }

    // Bundle verification, manifest extraction, and path validation functions
    // are in the `bundle` submodule. The methods below delegate for backward
    // compatibility with external callers that use `NodeClient::method()`.

    /// Check if a blob contains a bundle archive.
    /// Delegates to [`bundle::is_bundle_blob`].
    pub fn is_bundle_blob(blob_bytes: &[u8]) -> bool {
        bundle::is_bundle_blob(blob_bytes)
    }

    /// Install an application from a bundle blob that's already in the blobstore.
    /// This is used when a bundle blob is received via blob sharing or discovery.
    pub async fn install_application_from_bundle_blob(
        &self,
        blob_id: &BlobId,
        source: &ApplicationSource,
    ) -> eyre::Result<ApplicationId> {
        debug!(%blob_id, "install_application_from_bundle_blob started");

        let Some(bundle_bytes) = self.get_blob_bytes(blob_id, None).await? else {
            bail!("bundle blob not found");
        };

        let stored_size = bundle_bytes.len() as u64;

        self.install_bundle(bundle_bytes, blob_id, stored_size, source, None)
            .await
    }

    pub fn uninstall_application(&self, application_id: &ApplicationId) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();
        handle.delete(&key::ApplicationMeta::new(*application_id))?;
        Ok(())
    }

    // Query and management functions (list_applications, list_packages,
    // list_versions, get_latest_version, update_compiled_app) are in the
    // `query` submodule.
}
