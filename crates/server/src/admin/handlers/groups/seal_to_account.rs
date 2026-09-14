use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::AccountId;
use calimero_context::error::ContextError;
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::{seal_to_root, SealedEnvelope};
use calimero_governance_store::{
    AccountBindingRepository, MembershipRepository, NamespaceRepository,
};
use calimero_server_primitives::admin::{
    SealToAccountApiRequest, SealToAccountApiResponse, SealedEnvelopeApiData,
};
use calimero_store::Store;
use eyre::Result as EyreResult;
use reqwest::StatusCode;
use tracing::{error, info};

use super::{parse_account, parse_group_id};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// The account this node acts as in `group_id`.
///
/// Same principal `list_member_devices` gates on: the caller of an admin route
/// is the node, so its namespace identity is what resolves to a governance
/// account. An identity bound to no account is refused exactly as a non-member
/// is — it names nobody the membership rows could match.
fn caller_account(store: &Store, group_id: &ContextGroupId) -> EyreResult<AccountId> {
    let account = match NamespaceRepository::new(store).resolve_identity(group_id)? {
        Some((node_key, _)) => {
            calimero_governance_store::member_account_in_namespace(store, group_id, &node_key)?
        }
        None => None,
    };
    account.ok_or_else(|| not_a_group_member(group_id))
}

fn not_a_group_member(group_id: &ContextGroupId) -> eyre::Report {
    // Typed so the admin API surfaces this precondition as a 403 rather than a
    // generic 500 (see `parse_api_error`).
    ContextError::NotAGroupMember {
        group_id: format!("{group_id:?}"),
    }
    .into()
}

/// Seal `plaintext` to `target`'s root key, as seen from `group_id`.
///
/// Two membership questions, and they are not the same one:
///
/// * **the caller** must be a member of the group, so the surface does not widen
///   past "someone who already holds this group's key";
/// * **the target** must be an account this group knows, so the route cannot be
///   used to fish for whether an arbitrary account id exists on this node.
///
/// The root key itself is not a secret — it is hashed into the `AccountId` and
/// travels in every genesis — so neither check is protecting the value. They
/// keep the route's reach equal to the caller's existing reach.
fn seal(
    store: &Store,
    group_id: &ContextGroupId,
    target: AccountId,
    plaintext: Vec<u8>,
) -> EyreResult<Option<(u32, SealedEnvelope)>> {
    let caller = caller_account(store, group_id)?;
    let membership = MembershipRepository::new(store);
    if !membership.is_member(group_id, &caller)? && !membership.is_admin(group_id, &caller)? {
        return Err(not_a_group_member(group_id));
    }

    // Binding rows are keyed by NAMESPACE — a subgroup owns none — which is why
    // the root lookup below resolves upward first, exactly as
    // `list_member_devices` does.
    let namespace = NamespaceRepository::new(store).resolve(group_id)?;

    let known = membership.is_member(group_id, &target)?
        || membership.is_admin(group_id, &target)?
        || membership
            .enumerate_inherited(group_id)?
            .into_iter()
            .any(|(account, _)| account == target);
    if !known {
        return Ok(None);
    }

    // The CURRENT root and its epoch, not epoch 0: an account that has rotated
    // its root is opened by the key it rotated to, and the epoch travels with
    // the envelope so a holder of several knows which root opens which.
    let Some((epoch, root_pk)) =
        AccountBindingRepository::new(store).account_key(&namespace, target)?
    else {
        return Ok(None);
    };

    let envelope = seal_to_root(&mut rand::rng(), &root_pk, plaintext)?;
    Ok(Some((epoch, envelope)))
}

/// `POST /admin-api/groups/:group_id/accounts/:account/seal`
///
/// Produce a blob that only `:account`'s root key can open. The node resolves
/// the root itself — a caller naming a key would sooner or later name a device
/// key, and an envelope sealed to a device is unopenable in exactly the case an
/// envelope is written for.
///
/// Confidentiality only. The sender key is ephemeral and unauthenticated, so an
/// opened envelope proves nothing about who wrote it; the service that accepts
/// a sealed payload is what has to decide it is legitimate.
pub async fn handler(
    Path((group_id_str, account_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<SealToAccountApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let account = match parse_account(&account_str) {
        Ok(account) => account,
        Err(err) => return err.into_response(),
    };

    // Validated as hex by `SealToAccountApiRequest::validate` before reaching
    // here, so a decode failure is a bug rather than bad input.
    let plaintext = match hex::decode(&req.plaintext) {
        Ok(bytes) => bytes,
        Err(err) => {
            error!(error = ?err, "plaintext passed validation but did not decode");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "could not decode the validated plaintext".to_owned(),
            }
            .into_response();
        }
    };

    // The plaintext is the thing being protected, so nothing about it is logged
    // — not its content and not its length, which leaks the size of a namespace
    // set on its own.
    info!(group_id=%group_id_str, account=%account_str, "Sealing a payload to an account root");

    match seal(&state.store, &group_id, account, plaintext) {
        Ok(Some((account_root_epoch, envelope))) => ApiResponse {
            payload: SealToAccountApiResponse {
                data: SealedEnvelopeApiData {
                    account_root_epoch,
                    ephemeral_public_key: hex::encode(AsRef::<[u8; 32]>::as_ref(
                        &envelope.ephemeral_public_key,
                    )),
                    nonce: hex::encode(envelope.nonce),
                    ciphertext: hex::encode(envelope.ciphertext),
                },
            },
        }
        .into_response(),
        // One answer for "no such account here" and "this group does not know
        // it": both mean the same thing to a caller, and separating them would
        // let the route report which account ids exist on this node.
        Ok(None) => ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: format!(
                "group {group_id_str} knows no account {account_str} with a root key on record"
            ),
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
