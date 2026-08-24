//! `POST /admin-api/contexts/:context_id/intents` — run one intent a member
//! authorized, and publish the result attributed to them.
//!
//! # Why the checks are here and not on the receive path
//!
//! Peers verify a delegated delta's envelope and authorize it at the cut, and
//! that is the security boundary — nothing this handler does is load-bearing for
//! a peer. Three checks nonetheless belong here and nowhere else:
//!
//! **`not_after`.** This is the only place a clock may decide. A receiver
//! checking wall-clock expiry would accept a delta on one node and refuse it on
//! another depending on when each applied, and authorization would stop
//! converging — the same reason `calimero-account` has no certificate expiry at
//! all. Here there is one clock and nothing has converged yet, so the bound is
//! meaningful and cheap.
//!
//! **Whether this node may author here.** An intent for a context where this
//! node holds no authorship grant is refused with its own error rather than
//! executed and published. Peers would drop the result, and to the member a
//! silently dropped write is indistinguishable from data loss — which then gets
//! diagnosed as a client bug rather than as the missing grant it is.
//!
//! **That the warrant covers THIS intent.** Everything else establishes that the
//! member signed *something*. `covers_intent` is what stops a genuinely signed
//! warrant being a blank cheque for whatever the relay chose to run, and no peer
//! can perform it: the intent detail is sealed, so only the party holding the
//! plaintext can compare it to the commitment.

use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_client::client::ContextClient;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{PerformIntentApiRequest, PerformIntentApiResponse};
use futures_util::StreamExt;
use tracing::{info, warn};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// Seconds since the Unix epoch, for the one check that needs a clock.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub async fn handler(
    Path(context_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<PerformIntentApiRequest>,
) -> impl IntoResponse {
    let context_id: ContextId = match context_id_str.parse() {
        Ok(id) => id,
        Err(err) => {
            return parse_api_error(eyre::eyre!(
                "context id '{context_id_str}' is not valid: {err}"
            ))
            .into_response()
        }
    };

    match perform(&state.ctx_client, context_id, req).await {
        Ok(response) => ApiResponse { payload: response }.into_response(),
        Err(err) => {
            warn!(%context_id, %err, "refusing intent");
            parse_api_error(err).into_response()
        }
    }
}

/// This node's own signing identity in the context.
async fn local_signer(
    ctx_client: &ContextClient,
    context_id: &ContextId,
) -> eyre::Result<calimero_primitives::identity::PublicKey> {
    let members = ctx_client.get_context_members(context_id, Some(true));
    let mut members = std::pin::pin!(members);
    members
        .next()
        .await
        .transpose()?
        .map(|(key, _)| key)
        .ok_or_else(|| eyre::eyre!("this node owns no identity in this context"))
}

async fn perform(
    ctx_client: &ContextClient,
    context_id: ContextId,
    req: PerformIntentApiRequest,
) -> eyre::Result<PerformIntentApiResponse> {
    let warrant_bytes =
        hex::decode(req.warrant.trim()).map_err(|err| eyre::eyre!("warrant is not hex: {err}"))?;
    let warrant: calimero_account::Warrant = borsh::from_slice(&warrant_bytes)
        .map_err(|err| eyre::eyre!("warrant is not a valid statement: {err}"))?;

    let proof_bytes = hex::decode(req.author_proof.trim())
        .map_err(|err| eyre::eyre!("authorProof is not hex: {err}"))?;
    let author_proof: calimero_account::AccountProof<calimero_account::DeviceCert> =
        borsh::from_slice(&proof_bytes)
            .map_err(|err| eyre::eyre!("authorProof is not a valid credential: {err}"))?;

    // The node attaches its OWN half. The author authorized an operator account
    // and never has to learn which of its processes runs the intent — that is
    // what `Warrant::executor` being an account buys, and asking a client for
    // this node's process key would give it back.
    let group_id =
        calimero_governance_store::get_group_for_context(ctx_client.datastore(), &context_id)?
            .ok_or_else(|| eyre::eyre!("this context belongs to no group"))?;
    let signer = local_signer(ctx_client, &context_id).await?;
    let executor_proof =
        calimero_context::join_credential::build(ctx_client.datastore(), &group_id, &signer)
            .map_err(|err| eyre::eyre!("this node could not present its own credential: {err}"))?;

    let delegation = calimero_account::Delegation {
        warrant: Box::new(warrant),
        author_proof: Box::new(author_proof),
        executor_proof,
        executor_key: signer,
    };

    // Authenticity first, so every later message is about a warrant that is
    // genuinely the member's rather than one a caller made up.
    let warrant = delegation
        .verify()
        .map_err(|err| eyre::eyre!("delegation does not verify: {err}"))?;

    if warrant.context != context_id {
        eyre::bail!("this warrant authorises a different context than the one it was presented in");
    }

    let args = serde_json::to_vec(&req.args_json)
        .map_err(|err| eyre::eyre!("arguments could not be encoded: {err}"))?;
    if !warrant.covers_intent(&req.method, &args) {
        eyre::bail!(
            "this warrant does not cover the intent presented with it: it commits to a \
             different method or arguments"
        );
    }

    // The one clock check in the system. See the module header.
    let now = now_secs();
    if warrant.not_after < now {
        eyre::bail!(
            "this warrant expired at {} and it is now {now}; mint a fresh one",
            warrant.not_after
        );
    }

    // Refuse rather than publish something peers will drop.
    let executor = warrant.executor;
    if !calimero_governance_store::warrant_gate::account_may_author(
        ctx_client.datastore(),
        &context_id,
        executor,
    )? {
        eyre::bail!(
            "this node holds no authorship grant on the group owning this context, so it \
             cannot act for a member here — an admin must grant CAN_AUTHOR_ON_BEHALF to \
             {executor}"
        );
    }

    info!(
        %context_id,
        method = %req.method,
        author = %warrant.author_account,
        nonce = warrant.nonce,
        "performing intent on a member's behalf"
    );

    // The signer stays this node's own, as it always is — what changes is that
    // the run's PRINCIPAL comes from the warrant, so the application observes
    // the member and the change is attributed to them.
    let outcome = ctx_client
        .execute_with_origin(
            &context_id,
            &signer,
            req.method,
            args,
            None,
            None,
            0,
            Some(Box::new(delegation)),
        )
        .await
        .map_err(|err| eyre::eyre!("execution failed: {err}"))?;

    Ok(PerformIntentApiResponse {
        delta_id: None,
        returns: outcome
            .returns
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()),
    })
}
