//! Signing and persistence of authorized storage actions for context
//! execution: signing local User/Shared actions that are unsigned, and
//! persisting their signatures into the store. Extracted from the execute
//! handler; shared with `handlers::create_context`.

use calimero_account::AccountId;
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
///
/// `on_behalf` is the account a delegated run writes for, `None` for every other
/// run. Storage stamped each placeholder with the run's DEVICE as its signer,
/// which in a delegated run is the author's device, whose key this node does not
/// hold. So for a delegated run each placeholder is restamped before it is
/// signed: `signer` becomes this node's key (the one that signs) and `on_behalf`
/// the author's account, both inside the signed payload. Peers then verify the
/// signature under the relay's key and judge ownership and writer sets against
/// the author's account, provided the relay may write for it
/// (`calimero_governance_store::on_behalf_standing`).
///
/// # Errors
/// Refuses, rather than signs, a placeholder whose `signer` names any key but
/// the signing one: that entry would verify under no key its signer names, and
/// every peer would refuse it. After the restamp this cannot happen; the check
/// is what keeps a future path that stamps another device from publishing
/// entries that are poisoned on arrival.
pub(crate) fn sign_authorized_actions(
    actions: &mut [Action],
    identity_private_key: &PrivateKey,
    on_behalf: Option<AccountId>,
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
                signature_data: Some(sig_data),
                ..
            } => {
                // Placeholder-only, exactly like the two arms below, and for the
                // same reason: the authorization decision was already made at
                // write time (storage stamps a placeholder only for the owner's
                // own entries). `owner` is an account now, so it cannot be
                // compared against the executor's key here — and the key it CAN
                // be compared against is `sig_data.signer`, which storage
                // stamped with this same device.
                let placeholder = sig_data.signature == [0; 64];
                if placeholder {
                    stamp_placeholder(sig_data, nonce, executor_pk, on_behalf)?;
                }
                placeholder
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
                    stamp_placeholder(sig_data, nonce, executor_pk, on_behalf)?;
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

/// Fill a placeholder's signed fields: the nonce, and for a delegated run the
/// signer and the account written for. Refuses a signer the signing key is not.
fn stamp_placeholder(
    sig_data: &mut calimero_storage::entities::SignatureData,
    nonce: u64,
    signing_key: calimero_primitives::identity::PublicKey,
    on_behalf: Option<AccountId>,
) -> eyre::Result<()> {
    sig_data.nonce = nonce;
    if let Some(account) = on_behalf {
        sig_data.signer = Some(signing_key);
        sig_data.on_behalf = Some(account);
    }
    if sig_data.signer != Some(signing_key) {
        eyre::bail!(
            "refusing to sign an entry whose signer is {:?} with the key {signing_key}: \
             no peer could verify it",
            sig_data.signer
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
/// `account` MUST be the account the execution that produced `actions` ran as —
/// passed in rather than re-resolved here, and the difference is not cosmetic.
/// Re-resolving via `account_for_context` looks equivalent and is not during
/// context creation: the context→group row lands after `init`, so that lookup
/// falls back to a context-scoped account while `init` itself ran under the
/// namespace-scoped one. Nothing on this path reads the account today
/// (`update_signature_in_place` patches a signature and refuses any structural
/// change), so the mismatch would be inert until the first gate that does read
/// it — at which point context creation breaks in the confusing way documented on
/// `account_for_group`. Threading it keeps the signing pass gating as the
/// execution did, which is what the caller already knows.
pub(crate) fn persist_signed_signatures(
    store: &Store,
    context: &Context,
    account: AccountId,
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
        *account.as_bytes(),
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
            let signed_with_real_sig = matches!(
                &storage_type,
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
                } if sig.signature != [0u8; 64]
            );
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

#[cfg(test)]
mod tests {
    use calimero_account::AccountId;
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_storage::action::Action;
    use calimero_storage::address::Id;
    use calimero_storage::entities::{EntryRules, Metadata, SignatureData, StorageType};

    use super::sign_authorized_actions;

    const NONCE: u64 = 42;

    fn owner() -> AccountId {
        AccountId::from([0xA1; 32])
    }

    /// A `User` add carrying the placeholder storage stamps, with `signer` the
    /// device the run executed as.
    fn placeholder(signer: PublicKey) -> Action {
        let mut metadata = Metadata::new(1, NONCE);
        metadata.storage_type = StorageType::User {
            rules: EntryRules::OWNED,
            owner: owner(),
            signature_data: Some(SignatureData {
                signature: [0; 64],
                nonce: 0,
                signer: Some(signer),
                on_behalf: None,
            }),
        };
        Action::Add {
            id: Id::new([0x01; 32]),
            data: vec![7],
            ancestors: vec![],
            metadata,
        }
    }

    fn sig_data(action: &Action) -> SignatureData {
        let Action::Add { metadata, .. } = action else {
            panic!("an add");
        };
        let StorageType::User {
            signature_data: Some(sd),
            ..
        } = &metadata.storage_type
        else {
            panic!("a signed User entry");
        };
        *sd
    }

    /// A run on this node's own behalf is signed as before: by the key that
    /// storage named, with no account written for.
    #[test]
    fn a_direct_run_is_signed_by_the_key_it_names() {
        let node = PrivateKey::from([0x22; 32]);
        let mut actions = [placeholder(node.public_key())];
        sign_authorized_actions(&mut actions, &node, None).expect("sign");

        let sd = sig_data(&actions[0]);
        assert_eq!(sd.signer, Some(node.public_key()));
        assert_eq!(sd.on_behalf, None);
        assert_eq!(sd.nonce, NONCE);
        node.public_key()
            .verify_raw_signature(&actions[0].payload_for_signing(), &sd.signature)
            .expect("verifies under the key it names");
    }

    /// A delegated run's placeholders name the author's device, whose key this
    /// node does not hold. They are restamped to this node's key and the
    /// author's account before signing, so the signature verifies under the key
    /// the entry names and commits to the account it was written for.
    #[test]
    fn a_delegated_run_signs_for_the_author_under_the_relays_key() {
        let relay = PrivateKey::from([0x22; 32]);
        let author_device = PrivateKey::from([0x44; 32]).public_key();
        let mut actions = [placeholder(author_device)];
        sign_authorized_actions(&mut actions, &relay, Some(owner())).expect("sign");

        let sd = sig_data(&actions[0]);
        assert_eq!(sd.signer, Some(relay.public_key()), "signed by the relay");
        assert_eq!(sd.on_behalf, Some(owner()), "written for the author");
        relay
            .public_key()
            .verify_raw_signature(&actions[0].payload_for_signing(), &sd.signature)
            .expect("verifies under the relay's key");

        // The account is inside the signed payload: naming another breaks it.
        let mut retargeted = actions[0].clone();
        if let Action::Add { metadata, .. } = &mut retargeted {
            if let StorageType::User {
                signature_data: Some(sd),
                ..
            } = &mut metadata.storage_type
            {
                sd.on_behalf = Some(AccountId::from([0x51; 32]));
            }
        }
        assert!(relay
            .public_key()
            .verify_raw_signature(&retargeted.payload_for_signing(), &sd.signature)
            .is_err());
    }

    /// The bug a delegated run had: an entry naming one key, signed by
    /// another, which no peer can verify. It is refused, not signed.
    #[test]
    fn an_entry_naming_another_key_is_refused_not_signed() {
        let node = PrivateKey::from([0x22; 32]);
        let other = PrivateKey::from([0x44; 32]).public_key();
        let mut actions = [placeholder(other)];
        let err = sign_authorized_actions(&mut actions, &node, None)
            .expect_err("must refuse to sign under a key the entry does not name");
        assert!(err.to_string().contains("refusing to sign"), "{err}");
        assert_eq!(sig_data(&actions[0]).signature, [0; 64], "left unsigned");
    }
}
