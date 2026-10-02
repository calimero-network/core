//! Binding acquired bytes to an application row, and releasing them when that
//! fails - there is no content-addressed GC to reclaim a rejected artifact.

use std::cmp::Ordering;
use std::sync::PoisonError;

use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_store::key;
use calimero_store::types;
use eyre::bail;
use semver::Version;
use tracing::warn;

use crate::client::NodeClient;

/// Who asked for an install. Any group can name any application id, so only an
/// operator may move the row to an older release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallOrigin {
    Operator, // an admin-authenticated request on this node
    Remote,   // a group, peer or relayed member named the release
}

/// Whether a remote party's signed `version` may replace `row`: only an equal
/// or provably newer release may; anything else keeps the row's release.
fn remote_bundle_may_replace(row: Option<&types::ApplicationMeta>, version: &str) -> bool {
    let Some(row) = row.filter(|row| !row.signer_id.is_empty()) else {
        return true;
    };
    if *row.version == *version {
        return true;
    }
    match (Version::parse(&row.version), Version::parse(version)) {
        (Ok(installed), Ok(incoming)) => installed.cmp_precedence(&incoming) != Ordering::Greater,
        _ => false,
    }
}

impl NodeClient {
    fn application_row(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<types::ApplicationMeta>> {
        Ok(self
            .datastore
            .handle()
            .get(&key::ApplicationMeta::new(*application_id))?)
    }

    /// Whether a signed bundle at `version` may replace the row under `application_id`.
    pub(super) fn bundle_may_replace(
        &self,
        application_id: &ApplicationId,
        version: &str,
        origin: InstallOrigin,
    ) -> eyre::Result<bool> {
        match origin {
            InstallOrigin::Operator => Ok(true),
            InstallOrigin::Remote => Ok(remote_bundle_may_replace(
                self.application_row(application_id)?.as_ref(),
                version,
            )),
        }
    }

    /// Write a signed bundle's row unless the rule refuses it, re-checked under
    /// the row lock. Returns whether the row was written.
    pub(super) fn put_bundle_row(
        &self,
        application_id: &ApplicationId,
        row: &types::ApplicationMeta,
        origin: InstallOrigin,
    ) -> eyre::Result<bool> {
        let _rows = self
            .row_writes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !self.bundle_may_replace(application_id, &row.version, origin)? {
            return Ok(false);
        }
        self.datastore
            .handle()
            .put(&key::ApplicationMeta::new(*application_id), row)?;
        Ok(true)
    }

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

    /// A row under `application_id` naming `blob_id`, for tests. Nothing in a
    /// node writes a raw-wasm row, and the execution read refuses one.
    #[cfg(any(test, feature = "testing"))]
    pub fn write_application_row(
        &self,
        application_id: &ApplicationId,
        blob_id: &BlobId,
        size: u64,
        source: &calimero_primitives::application::ApplicationSource,
    ) -> eyre::Result<()> {
        self.datastore.handle().put(
            &key::ApplicationMeta::new(*application_id),
            &types::ApplicationMeta::new(
                key::BlobMeta::new(*blob_id),
                size,
                source.to_string().into_boxed_str(),
                Box::default(),
                key::BlobMeta::new(BlobId::from([0_u8; 32])),
                types::PackageInfo {
                    package: Box::default(),
                    version: Box::default(),
                    signer_id: Box::default(),
                    state_version: 0,
                },
            ),
        )?;
        Ok(())
    }
}
