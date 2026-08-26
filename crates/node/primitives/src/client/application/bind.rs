//! Binding acquired bytes to an application row, and releasing them when that
//! fails - there is no content-addressed GC to reclaim a rejected artifact.

use calimero_app_downloader::registry::RegistryCoords;
use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_store::key;
use calimero_store::types;
use eyre::bail;
use tracing::warn;

use crate::client::NodeClient;

impl NodeClient {
    /// Verify `stored` against `expected`, deleting it on mismatch so a
    /// rejected download doesn't linger forever - there is no blob GC.
    pub async fn verify_stored_blob(
        &self,
        stored: BlobId,
        expected: Option<BlobId>,
    ) -> eyre::Result<()> {
        let Some(expected) = expected else {
            return Ok(());
        };
        if stored == expected {
            return Ok(());
        }
        if let Err(err) = self.delete_blob(stored).await {
            warn!(%stored, %err, "failed to delete mismatched blob");
        }
        bail!("blob id mismatch: expected {expected}, got {stored}");
    }

    /// Release `stored` when the install that followed it failed: no
    /// content-addressed GC exists, so its bytes would otherwise never go.
    pub(super) async fn release_blob_on_error<T>(
        &self,
        stored: BlobId,
        outcome: eyre::Result<T>,
    ) -> eyre::Result<T> {
        if outcome.is_err() {
            if let Err(err) = self.delete_blob(stored).await {
                warn!(%stored, %err, "failed to delete blob after a failed install");
            }
        }
        outcome
    }

    /// Write a row under a caller-named id, for ids that would vary per node.
    /// Absent `coords` records empty coordinates, never a guessed placeholder.
    pub fn write_application_row(
        &self,
        application_id: &ApplicationId,
        blob_id: &BlobId,
        size: u64,
        source: &ApplicationSource,
        coords: Option<RegistryCoords<'_>>,
    ) -> eyre::Result<()> {
        let (package, version) = coords.map_or(("", ""), |c| (c.package, c.version));
        let blob_meta = key::BlobMeta::new(*blob_id);
        let mut handle = self.datastore.handle();
        handle.put(
            &key::ApplicationMeta::new(*application_id),
            &types::ApplicationMeta::new(
                blob_meta,
                size,
                source.to_string().into_boxed_str(),
                Box::default(),
                key::BlobMeta::new(BlobId::from([0_u8; 32])),
                types::PackageInfo {
                    package: package.into(),
                    version: version.into(),
                    signer_id: String::new().into_boxed_str(),
                    state_version: 0,
                },
            ),
        )?;
        Ok(())
    }
}
