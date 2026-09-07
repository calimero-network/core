//! Context configuration synchronization: the join bootstrap that writes a
//! context's store entries and acquires the bytecode its group named.

use calimero_app_downloader::registry::{stored_coords, PENDING_BLOB_SHARE_SOURCE};
use calimero_app_downloader::{AppRequest, Outcome};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use eyre::WrapErr;
use tokio::sync::oneshot;
use tracing::{debug, warn};
use url::Url;

use super::ContextClient;
use crate::messages::{ContextMessage, SyncRequest};

impl ContextClient {
    /// Put `application_id`'s bytecode on this node, the way a joiner gets it.
    ///
    /// A joiner reaches the chain below through a context bootstrap. A device
    /// that holds an account but follows no context never does, so this drives
    /// the same chain off the application row the `ContextRegistered` apply
    /// writes: registry source first, then the blob layer's on-demand fetch.
    ///
    /// `context_id` names the scope the bytecode is announced under, not a
    /// membership claim - a context's own application bundle is served to any
    /// requester, which is the allowance that lets a node bootstrap at all.
    ///
    /// `Ok(false)` is "not yet", never a failure: nothing is known to fetch, no
    /// peer served it, or the row names bytecode whose id this node cannot
    /// reproduce. Callers retry.
    ///
    /// # Errors
    /// Propagates store reads and a failed bundle install.
    pub async fn ensure_application_bytecode(
        &self,
        application_id: ApplicationId,
        context_id: ContextId,
    ) -> eyre::Result<bool> {
        let Some(application) = self.node_client.get_application(&application_id)? else {
            return Ok(false);
        };
        // `has_application` alone is not "installed". The `ContextRegistered` stub
        // names the creator's real blob, so a node holding those bytes for any
        // other reason already satisfies it while the row still carries no
        // package, version or size. `size == 0` is the same still-a-stub sentinel
        // the joiner's post-blob-share install keys on.
        if application.size != 0 && self.node_client.has_application(&application_id)? {
            return Ok(true);
        }
        // A zero blob is the context bootstrap's stub, which names no bytecode to
        // fetch; the peer rung below declines it on its own, and the registry rung
        // can still resolve such a row from its coordinates.
        let blob_id = application.blob.bytecode;
        let source: Url = application.source.into();

        // First rung of the joiner's chain, and the one that needs no peer. A
        // stub row records no coordinates, so there is nothing to address yet.
        let version = application.version.as_ref().map(ToString::to_string);
        if let (package, Some(version)) = (application.package.as_str(), version.as_deref()) {
            if let Some(coords) = stored_coords(package, version) {
                if let Some(installed) = self
                    .node_client
                    .install_by_coords(coords.package, coords.version)
                    .await?
                {
                    return Self::installed_as_expected(application_id, installed);
                }
            }
        }

        // Fetches from peers when absent and persists what it gets, so this both
        // acquires the blob and hands back the bytes the bundle check needs.
        let Some(blob_bytes) = self
            .node_client
            .get_blob_bytes(&blob_id, Some(&context_id))
            .await?
        else {
            debug!(%application_id, %blob_id, %context_id, "no peer served the application bytecode yet");
            return Ok(false);
        };

        let probe = blob_bytes.to_vec();
        if !tokio::task::spawn_blocking(move || NodeClient::is_bundle_blob(&probe)).await? {
            // A raw-wasm id is derived from the bytes, size, source and metadata
            // it was installed with, and a stub row carries none of them
            // faithfully - installing would write a row under a different id.
            warn!(%application_id, %blob_id, "application bytecode is not a bundle; it can only be installed through a context sync");
            return Ok(false);
        }

        let installed = self
            .node_client
            .install_application_from_bundle_blob(&blob_id, &source.into())
            .await?;
        Self::installed_as_expected(application_id, installed)
    }

    fn installed_as_expected(
        expected: ApplicationId,
        installed: ApplicationId,
    ) -> eyre::Result<bool> {
        if installed != expected {
            eyre::bail!("application mismatch: expected {expected}, got {installed}");
        }
        Ok(true)
    }

    /// Synchronize context configuration and ensure context metadata is present.
    ///
    /// Two modes:
    ///
    /// * **Bootstrap** (`config: Some(...)`): The context does not exist locally
    ///   yet. The caller supplies initial revision hints. The function installs
    ///   the application (if not already present), writes `ContextMeta` and
    ///   `ContextConfig`, and sends a `Sync` message to the context manager.
    ///
    /// * **Refresh** (`config: None`): The context already exists locally.
    ///   Returns the stored context. Membership and application state are kept
    ///   up-to-date through the governance DAG, so no revision polling is needed.
    pub async fn sync_context_config(
        &self,
        context_id: ContextId,
        config: Option<ContextConfigParams>,
    ) -> eyre::Result<Context> {
        let mut handle = self.registry.datastore.handle();

        let context = handle.get(&key::ContextMeta::new(context_id))?;

        // Refresh path: context already exists, return stored metadata.
        // Membership and application updates propagate through the governance
        // DAG, so there is no external source to poll for revision changes.
        let Some(config) = config else {
            let meta = context.ok_or_else(|| {
                eyre::eyre!("sync_context_config called with config: None but context {context_id} not found")
            })?;

            debug!(
                %context_id,
                application_id = %meta.application.application_id(),
                dag_heads_count = meta.dag_heads.len(),
                "context already exists, returning stored metadata"
            );

            return Ok(Context::with_service(
                context_id,
                meta.application.application_id(),
                meta.root_hash.into(),
                meta.dag_heads.clone(),
                meta.service_name.as_deref().map(String::from),
            ));
        };

        // The caller supplies application_id (from the group store) because
        // `ContextMeta` has not been written yet — it is created below.
        let application_id = if let Some(ctx) = &context {
            ctx.application.application_id()
        } else if let Some(id) = config.application_id {
            id
        } else {
            debug!(
                %context_id,
                "bootstrap: no application_id available yet; \
                 writing placeholder — sync will populate it"
            );
            ApplicationId::zero()
        };

        // One resolver for both planes, and the row stays under the id
        // governance named: a re-derived raw-wasm id varies per node.
        if application_id != ApplicationId::zero() {
            let bytecode_id = key::ApplicationMeta::new(application_id);
            if let Some(row) = handle.get(&bytecode_id)? {
                // The coordinates governance seeded onto the row: this is the
                // only registry signal a joiner has before its first sync.
                let coords = stored_coords(&row.package, &row.version);

                let outcome = self
                    .node_client
                    .acquire_bytecode(&AppRequest {
                        bytecode_id: Some(row.bytecode.blob_id()),
                        application_id: Some(application_id),
                        package: coords.map_or("", |coords| coords.package),
                        version: coords.map_or("", |coords| coords.version),
                        context_id: Some(&context_id),
                    })
                    .await;

                if outcome == Outcome::Unavailable {
                    // Not fatal: the configured source had nothing yet, and
                    // every later access re-runs this same acquisition.
                    warn!(
                        %context_id,
                        %application_id,
                        "bootstrap could not acquire bytecode yet; will retry on next access"
                    );
                }
            } else {
                debug!(
                    %context_id,
                    %application_id,
                    "application not available locally during bootstrap; writing stub \
                     — the configured source delivers it once governance names a blob"
                );
                let zero_blob = key::BlobMeta::new(BlobId::from([0_u8; 32]));
                handle.put(
                    &bytecode_id,
                    &types::ApplicationMeta::new(
                        zero_blob,
                        0,
                        PENDING_BLOB_SHARE_SOURCE.to_owned().into_boxed_str(),
                        Box::default(),
                        zero_blob,
                        types::PackageInfo {
                            package: String::new().into_boxed_str(),
                            version: String::new().into_boxed_str(),
                            signer_id: String::new().into_boxed_str(),
                            state_version: 0,
                        },
                    ),
                )?;
            }
        }

        handle.put(
            &key::ContextConfig::new(context_id),
            &types::ContextConfig::new(config.application_revision, config.members_revision),
        )?;

        // Re-read ContextMeta immediately before the write. The `context`
        // captured at the top of this function is STALE: we `.await`ed through
        // application / blob installation above, and a concurrent snapshot
        // finalize may have set this context's `root_hash` + `dag_heads` in the
        // meantime. Writing back the pre-`await` capture (`root=0`/`heads=[]` on
        // a fresh bootstrap) would clobber that finalize while its applied
        // `ContextState` entries persist — the `has_state_keys && root==0`
        // contradiction that permanently trips the snapshot safety gate (#3252).
        // Preserve whatever root/heads are on disk NOW; this write only
        // (re)asserts the resolved `application_id` + `service_name`.
        let current_meta = handle.get(&key::ContextMeta::new(context_id))?;
        let (root_hash, dag_heads) = bootstrap_meta_root_heads(current_meta.as_ref());

        handle.put(
            &key::ContextMeta::new(context_id),
            &types::ContextMeta::new(
                key::ApplicationMeta::new(application_id),
                *root_hash,
                dag_heads.clone(),
                config.service_name.as_deref().map(Box::from),
            ),
        )?;

        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::Sync {
                request: SyncRequest {
                    context_id,
                    application_id,
                },
                outcome: sender,
            })
            .await
            .wrap_err("context manager mailbox closed")?;

        receiver
            .await
            .wrap_err("context manager dropped the response channel")?;

        Ok(Context::with_service(
            context_id,
            application_id,
            root_hash,
            dag_heads,
            config.service_name,
        ))
    }
}

/// The `(root_hash, dag_heads)` a bootstrap `ContextMeta` write must persist,
/// given the context's CURRENT on-disk metadata (re-read immediately before the
/// write — NOT the stale pre-`await` capture from the top of
/// [`ContextClient::sync_context_config`]).
///
/// Preserves any root/heads a concurrent snapshot finalize wrote; defaults to
/// the uninitialized `(0, [])` only when no record exists yet (a genuine fresh
/// bootstrap). Extracted as a pure function so the "never clobber a finalized
/// context back to `root=0`" invariant (#3252) is unit-testable without the full
/// async bootstrap path.
fn bootstrap_meta_root_heads(current: Option<&types::ContextMeta>) -> (Hash, Vec<[u8; 32]>) {
    current.map_or_else(
        || (Hash::default(), vec![]),
        |meta| (meta.root_hash.into(), meta.dag_heads.clone()),
    )
}

#[cfg(test)]
mod bootstrap_meta_tests {
    use calimero_primitives::application::ApplicationId;
    use calimero_store::{key, types};

    use super::bootstrap_meta_root_heads;

    // Genuine fresh bootstrap: no ContextMeta on disk yet ⇒ uninitialized.
    #[test]
    fn no_meta_yields_uninitialized() {
        let (root, heads) = bootstrap_meta_root_heads(None);
        assert_eq!(*root, [0u8; 32]);
        assert!(heads.is_empty());
    }

    // #3252 regression: a concurrent snapshot finalize has already published a
    // non-zero root + heads by the time the bootstrap write runs. The bootstrap
    // write must PRESERVE them, never clobber back to root=0/heads=[].
    #[test]
    fn existing_finalized_meta_is_preserved() {
        let finalized = types::ContextMeta::new(
            key::ApplicationMeta::new(ApplicationId::from([0xAA; 32])),
            [0x11; 32],
            vec![[0x22; 32]],
            None,
        );
        let (root, heads) = bootstrap_meta_root_heads(Some(&finalized));
        assert_eq!(*root, [0x11; 32], "snapshot-finalized root preserved");
        assert_eq!(
            heads,
            vec![[0x22; 32]],
            "snapshot-finalized heads preserved"
        );
    }
}
