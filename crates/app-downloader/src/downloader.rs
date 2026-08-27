//! The one entry point: one configured source, verified and installed here.

use std::fmt::Debug;
use std::sync::Arc;

use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use eyre::bail;
use thiserror::Error;
use tracing::{info, warn};

use crate::port::ApplicationStore;
use crate::registry::{stored_coords, PENDING_BLOB_SHARE_SOURCE};
use crate::source::{AppRequest, AppSource};

/// A download that failed for a reason retrying will not fix on its own: the
/// bytes did not verify, the bundle would not install, or storage failed.
/// A source that simply had nothing yet is [`Outcome::Unavailable`].
#[derive(Debug, Error)]
#[error("{0:#}")]
pub struct DownloadError(#[from] eyre::Report);

/// What a download left behind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The row already named these bytes and they were already local.
    AlreadyInstalled,
    /// The bytes are now local and bound to the application row.
    Installed,
    /// The source had nothing yet. Never a fault: the caller keeps the version
    /// it runs and retries on next access.
    Unavailable,
}

/// Resolves a `bytecode_id` a group named into an installed, executable
/// application, from this node's one configured source.
#[derive(Clone, Debug)]
pub struct ApplicationDownloader<A> {
    store: A,
    source: Arc<dyn AppSource>,
}

impl<A: ApplicationStore + Debug + Send + Sync + 'static> ApplicationDownloader<A> {
    pub const fn new(store: A, source: Arc<dyn AppSource>) -> Self {
        Self { store, source }
    }

    /// Acquire the bytecode `req` names from the one source this node is
    /// configured with.
    ///
    /// On [`Outcome::AlreadyInstalled`] and [`Outcome::Installed`] the
    /// application row for `req.application_id` names `req.bytecode_id` and
    /// that blob is local - the application is installed and executable.
    /// `Unavailable` means the source had no bytes yet, which callers retry on
    /// next access rather than treat as a failure.
    pub async fn download(&self, req: &AppRequest<'_>) -> Result<Outcome, DownloadError> {
        Ok(self.walk(req).await?)
    }

    async fn walk(&self, req: &AppRequest<'_>) -> eyre::Result<Outcome> {
        let Some(application_id) = req.application_id else {
            bail!(
                "no application id to acquire {}@{} for",
                req.package,
                req.version
            );
        };
        let Some(bytecode_id) = req.bytecode_id else {
            bail!("no bytecode id to acquire for {application_id}");
        };
        if *bytecode_id == [0_u8; 32] {
            return Ok(Outcome::Unavailable);
        }

        if self.store.has_bytecode(&bytecode_id)? {
            if self.row_names_bytecode(application_id, bytecode_id)? {
                return Ok(Outcome::AlreadyInstalled);
            }
            // Bytes with no row behind them are not executable, so a local
            // blob still has to be installed before it counts as acquired.
            let Some(bytes) = self.store.read_local_bytecode(&bytecode_id).await? else {
                bail!("bytecode blob {bytecode_id} vanished before install");
            };
            // These bytes were already here, so nothing releases them: this
            // download never took a reference on the blob to give back.
            let source = self.recorded_source(application_id)?;
            self.bind(req, application_id, bytecode_id, &bytes, &source)
                .await?;
            return Ok(Outcome::Installed);
        }

        let Some(bytes) = self.source.fetch(req).await? else {
            warn!(%bytecode_id, "the configured source has no bytecode yet");
            return Ok(Outcome::Unavailable);
        };

        let (stored, _size) = self.store.store_bytecode(&bytes).await?;
        if stored != bytecode_id {
            self.release(stored).await;
            bail!("blob id mismatch: expected {bytecode_id}, got {stored}");
        }

        let source = self.recorded_source(application_id)?;
        self.install_or_release(req, application_id, stored, &bytes, &source)
            .await?;
        info!(%bytecode_id, "acquired bytecode");
        Ok(Outcome::Installed)
    }

    /// Install bytes this download stored, releasing them if it fails: nothing
    /// else reclaims a rejected artifact.
    async fn install_or_release(
        &self,
        req: &AppRequest<'_>,
        application_id: ApplicationId,
        stored: BlobId,
        bytes: &[u8],
        source: &ApplicationSource,
    ) -> eyre::Result<()> {
        match self.bind(req, application_id, stored, bytes, source).await {
            Ok(()) => Ok(()),
            Err(err) => {
                self.release(stored).await;
                Err(err)
            }
        }
    }

    /// Keep whatever location governance recorded; downgrading it to the
    /// marker would drop the coordinates this node announces onward.
    fn recorded_source(&self, application_id: ApplicationId) -> eyre::Result<ApplicationSource> {
        let recorded = self
            .store
            .installed_application(&application_id)?
            .map(|row| row.source)
            .filter(|source| !source.is_empty());
        Ok(recorded
            .as_deref()
            .unwrap_or(PENDING_BLOB_SHARE_SOURCE)
            .parse()?)
    }

    async fn bind(
        &self,
        req: &AppRequest<'_>,
        application_id: ApplicationId,
        stored: BlobId,
        bytes: &[u8],
        source: &ApplicationSource,
    ) -> eyre::Result<()> {
        self.store
            .bind_application(
                &application_id,
                stored,
                bytes.len() as u64,
                source,
                stored_coords(req.package, req.version),
                bytes,
            )
            .await
    }

    /// Whether the stored row already names these bytes. A row pointing at an
    /// older blob still has to be rebound to what was just acquired.
    fn row_names_bytecode(
        &self,
        application_id: ApplicationId,
        bytecode_id: BlobId,
    ) -> eyre::Result<bool> {
        Ok(self
            .store
            .installed_application(&application_id)?
            .is_some_and(|row| row.bytecode_id == bytecode_id))
    }

    async fn release(&self, stored: BlobId) {
        if let Err(err) = self.store.release_bytecode(stored).await {
            warn!(%stored, %err, "failed to release a rejected blob");
        }
    }
}
