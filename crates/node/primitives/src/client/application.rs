pub mod acquire;
mod bind;
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
use tracing::debug;

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
        let Some(application) = handle.get(&key::ApplicationMeta::new(*application_id))? else {
            return Ok(None);
        };
        self.application_bytes_from_blob(&application.bytecode.blob_id(), service_name)
            .await
    }

    // Installation and uninstallation are in `install` submodule.
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
            let verified = bundle::VerifiedBundle::open(blob_bytes)?;
            Ok(verified
                .manifest()
                .wasm_artifacts()
                .iter()
                .map(|a| a.name.map(str::to_owned))
                .collect())
        })
        .await
        .map_err(|e| eyre::eyre!("bundle manifest read task failed: {e}"))??;
        Ok(Some(names))
    }

    /// Bundle manifest of the blob at `blob_id`. `None` when the blob is
    /// absent locally or is not a bundle.
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
            bundle::VerifiedBundle::open(blob_bytes).map(|v| v.manifest().clone())
        })
        .await
        .map_err(|e| eyre::eyre!("bundle manifest read task failed: {e}"))??;
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
    /// by a group's `bytecode_id` or a context's activation marker whose bundle
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

        // Candidate blob set: the row + group bytecode_ids + activation markers.
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
                    // bytecode_id would otherwise be fetched + manifest-parsed
                    // just to be discarded by the package filter below.
                    if meta.target_application_id == *application_id {
                        let _ = candidates.insert(meta.bytecode_id);
                    }
                }
            }
            let mut iter = handle.iter::<key::ContextActivatedBytecode>()?;
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
                    bundle::VerifiedBundle::open(blob_bytes).map(|v| v.manifest().clone())
                })
                .await
                {
                    Ok(Ok(manifest)) => manifest,
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
                    version: app
                        .version
                        .as_ref()
                        .map(|v| v.as_str().to_owned())
                        .unwrap_or_default(),
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

    /// Application wasm bytes straight from a bytecode blob - the blobstore is
    /// the only copy, so a context pinned to a blob the application row no
    /// longer references still resolves. `None` when the blob is absent locally.
    pub async fn application_bytes_from_blob(
        &self,
        blob_id: &BlobId,
        service_name: Option<&str>,
    ) -> eyre::Result<Option<Arc<[u8]>>> {
        let Some(blob_bytes) = self.get_blob_bytes(blob_id, None).await? else {
            return Ok(None);
        };
        let service = service_name.map(str::to_owned);
        // Detection gunzips too, so it belongs on the blocking pool with the
        // read it gates rather than on the reactor thread.
        let wasm = tokio::task::spawn_blocking(move || -> eyre::Result<_> {
            if !Self::is_bundle_blob(&blob_bytes) {
                return Ok(blob_bytes);
            }
            bundle::VerifiedBundle::open(blob_bytes)?.wasm(service.as_deref())
        })
        .await??;
        Ok(Some(wasm))
    }
}
