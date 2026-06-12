//! Signing and persistence of authorized storage actions for context
//! execution: signing local User/Shared actions that are unsigned, and
//! persisting their signatures into the store. Extracted from the execute
//! handler; shared with `handlers::create_context`.

use calimero_primitives::context::Context;
use calimero_primitives::identity::PrivateKey;
use calimero_storage::action::Action;
use calimero_storage::entities::StorageType;
use calimero_storage::env::{with_runtime_env, RuntimeEnv};
use calimero_storage::interface::Interface;
use calimero_storage::store::MainStorage;
use calimero_store::Store;
use tracing::{debug, error, info, warn};

use crate::handlers::update_application::create_storage_callbacks;

/// Helper function to sign authorized actions (User and Shared storage).
/// Iterates over actions and signs any that are local and unsigned.
pub(crate) fn sign_authorized_actions(
    actions: &mut [Action],
    identity_private_key: &PrivateKey,
) -> eyre::Result<()> {
    info!(
        actions_count = actions.len(),
        "Signing authorized actions..."
    );
    let executor_pk = identity_private_key.public_key();
    for action in actions.iter_mut() {
        let action_id = action.id();

        // The nonce was already set by `calimero-storage`:
        // * For Add/Update, it's `metadata.updated_at`.
        // * For DeleteRef, it's `deleted_at`.
        let (metadata, nonce) = match action {
            Action::Add { metadata, .. } | Action::Update { metadata, .. } => {
                let nonce = *metadata.updated_at;
                (metadata, nonce)
            }
            Action::DeleteRef {
                metadata,
                deleted_at,
                ..
            } => {
                let nonce = *deleted_at;
                (metadata, nonce)
            }
            Action::Compare { .. } => continue,
        };

        // STAMP THE NONCE BEFORE COMPUTING THE PAYLOAD.
        //
        // `payload_for_signing` commits to `sig_data.nonce` (for User/Shared/
        // SharedMember). The nonce carried at outcome-build time can differ from
        // the final `metadata.updated_at` we stamp here, so computing the payload
        // before the stamp signed a STALE-nonce payload while the action shipped
        // the new nonce — every receiver (and the author itself, re-checking a
        // pushed-back leaf via HashComparison) then reconstructed a different
        // payload and rejected the signature as "Invalid signature for user-owned
        // data" (the concurrent-rotation SharedMember value split-brain). Stamp
        // first, then hash, so the signature commits to exactly the action that
        // ships. `should_sign` gates on "ours + still a placeholder", matching the
        // prior per-arm conditions; the borrow of `metadata` ends with this match
        // (NLL), freeing `action` for the immutable `payload_for_signing` below.
        let should_sign = match &mut metadata.storage_type {
            StorageType::User {
                owner,
                signature_data: Some(sig_data),
            } => {
                let ours = *owner == executor_pk && sig_data.signature == [0; 64];
                if ours {
                    sig_data.nonce = nonce;
                }
                ours
            }
            StorageType::Shared {
                signature_data: Some(sig_data),
                ..
            }
            | StorageType::SharedMember {
                signature_data: Some(sig_data),
                ..
            } => {
                // Sign whenever the placeholder is present. The authorization
                // decision (executor ∈ stored ∪ claimed / anchor writers) was
                // already made in `save_raw` / `remove_child_from`, which handles
                // the rotate-self-out case.
                let placeholder = sig_data.signature == [0; 64];
                if placeholder {
                    sig_data.nonce = nonce;
                }
                placeholder
            }
            _ => false,
        };

        if !should_sign {
            continue;
        }

        // Payload now reflects the stamped nonce — sign exactly what ships.
        let payload_for_signing = action.payload_for_signing();
        let signature = identity_private_key.sign(&payload_for_signing)?;

        let metadata = match action {
            Action::Add { metadata, .. } | Action::Update { metadata, .. } => metadata,
            Action::DeleteRef { metadata, .. } => metadata,
            Action::Compare { .. } => continue,
        };
        let sig_data = match &mut metadata.storage_type {
            StorageType::User {
                signature_data: Some(sd),
                ..
            }
            | StorageType::Shared {
                signature_data: Some(sd),
                ..
            }
            | StorageType::SharedMember {
                signature_data: Some(sd),
                ..
            } => sd,
            // `should_sign` was true, so one of the above matched; unreachable.
            _ => continue,
        };
        sig_data.signature = signature.to_bytes();

        debug!(
            action_id = %action_id,
            executor = %executor_pk,
            nonce = %nonce,
            payload_for_signing = ?payload_for_signing,
            "Signed authorized action (nonce stamped before payload)"
        );
    }
    Ok(())
}

/// Persist the signed `signature_data` from `sign_authorized_actions`
/// back to the local index entry for each upsert action.
///
/// Best-effort: structural mismatches and missing entities are logged
/// and skipped rather than failing the whole execute call. The Action
/// in the broadcast artifact carries the real signature; this function
/// keeps the locally stored entity's metadata in sync so HashComparison
/// (and any other receiver-verifying sync path) ships verifiable state.
///
/// Runs inside a `with_runtime_env` scope built over the post-commit
/// `Store` handle — `Interface::<MainStorage>::update_signature_in_place`
/// reads + writes the entity's `EntityIndex` blob through this runtime
/// env, which routes via `create_storage_callbacks` to the same
/// RocksDB keys that `storage.commit()` just wrote.
pub(crate) fn persist_signed_signatures(
    store: &Store,
    context: &Context,
    identity_private_key: &PrivateKey,
    actions: &[Action],
) -> eyre::Result<()> {
    let callbacks = create_storage_callbacks(store, context.id);
    let context_id_bytes: [u8; 32] = *context.id.as_ref();
    let executor_id_bytes: [u8; 32] = *identity_private_key.public_key().as_ref();
    let env = RuntimeEnv::new(
        callbacks.read,
        callbacks.write,
        callbacks.remove,
        context_id_bytes,
        executor_id_bytes,
    );

    // Collect failures inside the env scope and propagate after.
    // Returning Result lets the caller (`execute_method` or
    // `create_context`) decide whether to abort the transaction:
    // a failed persist leaves the locally stored entity with the
    // `[0; 64]` placeholder signature, so subsequent HashComparison
    // sync would ship the placeholder to peers and trip the
    // receiver's signature verifier. The signed broadcast artifact
    // still carries the real signature for delta-replay receivers,
    // but the local node would be permanently stuck shipping
    // unverifiable HashComparison responses until the next signed
    // write to that entity. Aborting and surfacing the error gives
    // the user a chance to retry.
    let result: eyre::Result<()> = with_runtime_env(env, || {
        for action in actions {
            let (id, storage_type, is_delete) = match action {
                Action::Add { id, metadata, .. } | Action::Update { id, metadata, .. } => {
                    (*id, metadata.storage_type.clone(), false)
                }
                // DeleteRef carries a real signature too (signed by
                // `sign_authorized_actions`). Persist it onto the now-tombstoned
                // index entry — `update_signature_in_place` RMWs the index, which
                // survives the delete — so HashComparison can later ship a
                // *verifiable* signed DeleteRef for the cleared entity (otherwise
                // a User/Shared clear can't converge via HC, only via the delta
                // stream). The tombstone's owner/writer set is unchanged by the
                // delete, so the in-place patch's identity guard still matches.
                // Marked `is_delete` so a persist failure is BEST-EFFORT (see
                // the `Err` arm): unlike Add/Update, a missed tombstone
                // signature only degrades HC clear-convergence — the deletion
                // still propagates via the delta stream — so it must NOT abort
                // the transaction.
                Action::DeleteRef { id, metadata, .. } => {
                    (*id, metadata.storage_type.clone(), true)
                }
                Action::Compare { .. } => continue,
            };
            // Only Shared/User with a REAL signature need the
            // re-persist. Public/Frozen don't carry signatures.
            //
            // Three skip conditions:
            // 1. `signature_data: None` — unsigned bootstrap action;
            //    `sign_authorized_actions` doesn't touch these.
            // 2. `signature_data: Some(SignatureData { signature: [0;
            //    64], .. })` — placeholder that
            //    `sign_authorized_actions` declined to sign (e.g. a
            //    `User` action whose owner ≠ executor, or a `Shared`
            //    action where the executor isn't in the writer set).
            //    Persisting the placeholder here would overwrite the
            //    real signature already stored for that entity.
            // 3. Anything else falls through to
            //    `update_signature_in_place`.
            let signed_with_real_sig = match &storage_type {
                StorageType::Shared {
                    signature_data: Some(sig),
                    ..
                }
                | StorageType::User {
                    signature_data: Some(sig),
                    ..
                }
                | StorageType::SharedMember {
                    signature_data: Some(sig),
                    ..
                } if sig.signature != [0u8; 64] => true,
                _ => false,
            };
            if !signed_with_real_sig {
                continue;
            }
            match Interface::<MainStorage>::update_signature_in_place(id, storage_type) {
                Ok(true) => {
                    debug!(%id, "persisted signed signature_data to local index");
                }
                Ok(false) => {
                    debug!(
                        %id,
                        "skipped signature persist — entity missing from local index \
                         (raced a delete?)"
                    );
                }
                Err(e) if is_delete => {
                    // BEST-EFFORT for deletes: a failed tombstone
                    // signature-persist only means this DeleteRef can't
                    // ship verifiably via HashComparison — the deletion
                    // still converges via the delta stream. Never abort
                    // the transaction over it (the strict path below is
                    // for Add/Update, where a placeholder would make a
                    // *live* entity unverifiable on peers).
                    warn!(
                        %id,
                        error = ?e,
                        "skipped persisting signed DeleteRef signature; HC clear-convergence \
                         degraded for this entity (delta-stream propagation unaffected)"
                    );
                }
                Err(e) => {
                    // Fail loud + propagate. The alternatives
                    // (silent log, metric, ignore) leave the local
                    // entity with a placeholder forever — see the
                    // function-level comment.
                    error!(
                        %id,
                        error = ?e,
                        "failed to persist signed signature_data; local entity would \
                         retain placeholder signature and fail HashComparison \
                         verification on peers — aborting transaction so the user \
                         can retry"
                    );
                    return Err(eyre::eyre!(
                        "persist_signed_signatures: update_signature_in_place failed \
                         for entity {id}: {e:?}"
                    ));
                }
            }
        }
        Ok(())
    });
    result
}
