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
//! node may not author is refused with its own error rather than executed and
//! published. Peers would drop the result, and to the member a silently dropped
//! write is indistinguishable from data loss — which then gets diagnosed as a
//! client bug rather than as the missing grant it is. A TEE replica
//! (`ReadOnlyTee`) is refused by its role, whatever capability it holds; a TEE
//! relay (`RelayTee`) may author by its role; any other node needs
//! `CAN_AUTHOR_ON_BEHALF`. A write whose AUTHOR is read-only in the context is
//! refused the same way, up front — never executed and then discarded.
//!
//! **That the warrant covers THIS intent.** Everything else establishes that the
//! member signed *something*. `covers_intent` is what stops a genuinely signed
//! warrant being a blank cheque for whatever the relay chose to run, and no peer
//! can perform it: the intent detail is sealed, so only the party holding the
//! plaintext can compare it to the commitment.

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_account::MAX_PRESENTED_HANDOFFS;
use calimero_context_client::client::ContextClient;
use calimero_governance_store::warrant_gate::WarrantRefusal;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    PerformIntentApiRequest, PerformIntentApiResponse, PerformIntentApiResponseData,
};
use calimero_server_primitives::jsonrpc::ExecutionError;
use eyre::WrapErr as _;
use futures_util::StreamExt;
use tracing::{debug, warn};

use crate::admin::service::{method_error_response, parse_api_error, ApiResponse};
use crate::AdminState;

/// Seconds since the Unix epoch, for the one check that needs a clock.
pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Why an intent was refused, so the status code can say which kind of "no".
///
/// Every variant here is a *caller* precondition, not a server fault, and
/// without this they all fell through `parse_api_error`'s generic arm as `500`.
/// That is the one answer none of them means: a client cannot tell "your
/// warrant is malformed" from "this node is broken", so it cannot know whether
/// retrying is pointless or the only sensible move. DAR-11 asks for a clean
/// refusal at the API, and a `500` is not one.
///
/// The split is by who has to change something:
///
/// * `400` — the request is wrong and re-sending it unchanged cannot help.
/// * `403` — the request is well-formed and genuinely signed, but authority is
///   missing. Someone else (an admin granting the capability) or something else
///   (a fresh warrant) has to change, not the bytes.
#[derive(Debug)]
pub enum IntentRefusal {
    /// The warrant, proof, or arguments could not be made sense of, or the
    /// delegation's signatures do not check out.
    Malformed(String),
    /// Genuinely signed, but it does not authorize *this*.
    NotAuthorized(String),
    /// This node is a TEE replica, which never relays a member's write — the
    /// namespace has to admit its TEEs in relay mode for that.
    ExecutorIsTeeReplica,
    /// This node is a plain `ReadOnly` member, which never relays a member's
    /// write either.
    ExecutorIsReadOnly,
    /// The member the write would be attributed to is read-only in the
    /// context, so no relay may write for them.
    AuthorIsReadOnly,
}

impl core::fmt::Display for IntentRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Malformed(message) | Self::NotAuthorized(message) => f.write_str(message),
            Self::ExecutorIsTeeReplica => f.write_str(
                "this node is a TEE replica (ReadOnlyTee) and does not relay writes; the \
                 namespace must admit relays with mode=relay",
            ),
            Self::ExecutorIsReadOnly => f.write_str(
                "this node's role in this context is read-only (ReadOnly), so it does not relay \
                 writes",
            ),
            Self::AuthorIsReadOnly => f.write_str("the author's role in this context is read-only"),
        }
    }
}

impl core::error::Error for IntentRefusal {}

impl IntentRefusal {
    /// The status this refusal deserves.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Malformed(_) => StatusCode::BAD_REQUEST,
            Self::NotAuthorized(_)
            | Self::ExecutorIsTeeReplica
            | Self::ExecutorIsReadOnly
            | Self::AuthorIsReadOnly => StatusCode::FORBIDDEN,
        }
    }

    /// The refusal `/intents` answers a gate verdict about a ROLE with, or
    /// `None` for a verdict that keeps its own message (a revoked device, a
    /// spent warrant, a missing grant).
    fn from_role_refusal(refusal: &WarrantRefusal) -> Option<Self> {
        match refusal {
            WarrantRefusal::ExecutorIsTeeReplica => Some(Self::ExecutorIsTeeReplica),
            WarrantRefusal::ExecutorIsReadOnly => Some(Self::ExecutorIsReadOnly),
            WarrantRefusal::AuthorIsReadOnly => Some(Self::AuthorIsReadOnly),
            _ => None,
        }
    }
}

/// Decode the hex-borsh warrant a client sent, as a 400 on failure.
///
/// Its own function so the refusal is testable without a `ContextClient`. Both
/// arms are [`IntentRefusal::Malformed`], like the `authorProof` pair beside the
/// call site — they used to be bare `eyre!` strings, which fell through to a 500.
/// That was invisible while no real client sent an undecodable warrant, and it
/// is the single most common refusal the moment one does: warrant v2 (#3933)
/// changed the layout, so a signer still emitting v1 sends 240 bytes where 351+
/// are expected, and "Internal server error" points it at the node rather than
/// at its own encoder.
fn decode_warrant(hex_warrant: &str) -> eyre::Result<calimero_account::Warrant> {
    let bytes = hex::decode(hex_warrant.trim()).map_err(|err| {
        eyre::eyre!(IntentRefusal::Malformed(format!(
            "warrant is not hex: {err}"
        )))
    })?;

    borsh::from_slice(&bytes).map_err(|err| {
        eyre::eyre!(IntentRefusal::Malformed(format!(
            "warrant is not a valid statement ({} bytes): {err}. A warrant signed \
             under the v1 layout no longer decodes — the signing domain is now \
             calimero.warrant.v2 and the encoding carries app_version, a plaintext \
             method and two cited-head lists",
            bytes.len()
        )))
    })
}

/// Decode the member's device credential an intent carries as `authorProof`.
/// The handoff cap is checked here because these routes may be served unauthenticated.
pub(crate) fn decode_author_proof(
    hex_proof: &str,
) -> eyre::Result<calimero_account::AccountProof<calimero_account::DeviceCert>> {
    let bytes = hex::decode(hex_proof.trim()).map_err(|err| {
        eyre::eyre!(IntentRefusal::Malformed(format!(
            "authorProof is not hex: {err}"
        )))
    })?;
    let proof: calimero_account::AccountProof<calimero_account::DeviceCert> =
        borsh::from_slice(&bytes).map_err(|err| {
            eyre::eyre!(IntentRefusal::Malformed(format!(
                "authorProof is not a valid credential: {err}"
            )))
        })?;
    if proof.chain.len() > MAX_PRESENTED_HANDOFFS {
        return Err(eyre::eyre!(IntentRefusal::Malformed(format!(
            "authorProof carries {} root-key handoffs; at most {MAX_PRESENTED_HANDOFFS} are accepted",
            proof.chain.len()
        ))));
    }
    Ok(proof)
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
            // The method ran and returned an error. Not a refusal, and not
            // logged as one: the execute path already warned with the app's
            // message redacted, and this one is not.
            if let Some(method_error) = err.downcast_ref::<ExecutionError>() {
                debug!(%context_id, %err, "intent's method returned an error");
                return method_error_response(method_error);
            }
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
    let warrant = decode_warrant(&req.warrant)?;

    let author_proof = decode_author_proof(&req.author_proof)?;

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
            .wrap_err("this node could not present its own credential")?;

    let delegation = calimero_account::Delegation {
        warrant: Box::new(warrant),
        author_proof: Box::new(author_proof),
        executor_proof,
        executor_key: signer,
    };

    // Authenticity first, so every later message is about a warrant that is
    // genuinely the member's rather than one a caller made up.
    let warrant = delegation.verify().map_err(|err| {
        eyre::eyre!(IntentRefusal::Malformed(format!(
            "delegation does not verify: {err}"
        )))
    })?;

    let args = serde_json::to_vec(&req.args_json)
        .map_err(|err| eyre::eyre!("arguments could not be encoded: {err}"))?;

    // Everything decidable from the warrant, the intent and a clock. Separated
    // out because the clock is the whole difficulty: with `now_secs()` called
    // inline, expiry could not be tested without moving the system clock, so the
    // one time-dependent rule in the feature was the one rule no test covered.
    warrant_authorises_intent(&warrant, context_id, &req.method, &args, now_secs())?;

    // Refuse rather than publish something peers will drop. A TEE replica is
    // told so by name: granting it the capability would not help, only
    // admitting the namespace's TEEs in relay mode does.
    let executor = warrant.executor;
    if let Some(refusal) = calimero_governance_store::warrant_gate::executor_refusal_for_context(
        ctx_client.datastore(),
        &context_id,
        executor,
    )? {
        if let Some(role_refusal) = IntentRefusal::from_role_refusal(&refusal) {
            eyre::bail!(role_refusal);
        }
        eyre::bail!(IntentRefusal::NotAuthorized(format!(
            "this node holds no authorship grant on the group owning this context, so it \
             cannot act for a member here — an admin must grant CAN_AUTHOR_ON_BEHALF to \
             {executor}"
        )));
    }

    // The full gate, before executing. The authoritative call is the one the
    // execute path makes under the context lock — this one cannot be, because a
    // concurrent request could spend the nonce between here and there.
    //
    // It runs anyway for a reason worth stating: everything inside the actor
    // returns through `ExecuteError::InternalError`, which carries no cause, so
    // a warrant refused in there reaches the caller as an opaque `500`. Asking
    // the same question here is what lets a replayed warrant — the common case,
    // and the one a relay is most likely to hit — come back as a typed `403`
    // that says the nonce was spent.
    //
    // A read-only author is refused here too, before anything runs, with the
    // same typed 403 as the executor's role.
    //
    // At no cut: the relay is about to execute at its own current heads, which
    // is also the cut its delta will cite, so this node's live state is the
    // answer every peer will reach for it.
    if let Err(err) = calimero_governance_store::warrant_gate::check_delegated_delta(
        ctx_client.datastore(),
        &context_id,
        &delegation,
        calimero_governance_store::AdmissionCut::live(),
    ) {
        if let Some(role_refusal) = err
            .downcast_ref::<WarrantRefusal>()
            .and_then(IntentRefusal::from_role_refusal)
        {
            eyre::bail!(role_refusal);
        }
        return Err(err);
    }

    debug!(
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
        // `wrap_err`, not `eyre!("{err}")`: the latter flattens the cause to a
        // string, and the gate's `WarrantRefusal` inside it is what tells
        // `parse_api_error` a replayed warrant is the caller's problem rather
        // than this node's. Formatting it away turns a clean 403 into a 500.
        .wrap_err("execution failed")?;

    // A method that returned `Err` ran and committed nothing, so nothing was
    // published and the warrant's nonce is unspent (it is spent only when the
    // delta applies). The error goes back typed, for `handler` to answer.
    Ok(PerformIntentApiResponse {
        data: intent_response_data(outcome).map_err(eyre::Report::new)?,
    })
}

/// The run's answer, with the method's own error kept as JSON-RPC `execute`
/// keeps it (`crate::execute::method_output`) rather than reported as a `null`
/// return.
fn intent_response_data(
    outcome: calimero_context_client::messages::ExecuteResponse,
) -> Result<PerformIntentApiResponseData, ExecutionError> {
    Ok(PerformIntentApiResponseData {
        root_hash: outcome.root_hash.to_string(),
        returns: crate::execute::method_output(outcome.returns)?,
    })
}

/// The checks decidable from the warrant, the intent and a clock — nothing else.
///
/// `now` is a parameter rather than read here so expiry is testable. See the
/// module header for why the clock lives on this side at all: a receiver checking
/// wall-clock expiry would accept a delta on one node and refuse it on another,
/// and authorization would stop converging.
///
/// # Errors
/// [`IntentRefusal::NotAuthorized`] if the warrant is for another context, does
/// not commit to this intent, or has expired.
fn warrant_authorises_intent(
    warrant: &calimero_account::Warrant,
    context_id: ContextId,
    method: &str,
    args: &[u8],
    now: u64,
) -> eyre::Result<()> {
    if warrant.context != context_id {
        eyre::bail!(IntentRefusal::NotAuthorized(
            "this warrant authorises a different context than the one it was presented in"
                .to_owned()
        ));
    }

    if !warrant.covers_intent(method, args) {
        eyre::bail!(IntentRefusal::NotAuthorized(
            "this warrant does not cover the intent presented with it: it commits to a \
             different method or arguments"
                .to_owned()
        ));
    }

    // The one clock check in the system. `<` not `<=`: a warrant is live through
    // the whole second it names, so `not_after == now` still authorises.
    if warrant.not_after < now {
        eyre::bail!(IntentRefusal::NotAuthorized(format!(
            "this warrant expired at {} and it is now {now}; mint a fresh one",
            warrant.not_after
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use calimero_account::Warrant;
    use calimero_primitives::identity::PrivateKey;

    use calimero_governance_store::warrant_gate::WarrantRefusal;

    use super::{
        decode_author_proof, decode_warrant, intent_response_data, warrant_authorises_intent,
        ContextId, IntentRefusal,
    };

    const METHOD: &str = "set";
    const ARGS: &[u8] = br#"{"key":"k","value":"v"}"#;
    const NOW: u64 = 1_700_000_000;

    /// A warrant covering `METHOD`/`ARGS`, expiring at `not_after`.
    ///
    /// Built as a literal rather than signed: authenticity is established by
    /// `delegation.verify()` before this function is ever called, so a signature
    /// here would test `calimero-account`, not this.
    fn warrant(context: ContextId, not_after: u64) -> Warrant {
        Warrant {
            context,
            author_account: calimero_account::AccountGenesis::new(
                PrivateKey::from([7u8; 32]).public_key(),
            )
            .account_id(),
            author_device_key: PrivateKey::from([8u8; 32]).public_key(),
            executor: calimero_account::AccountGenesis::new(
                PrivateKey::from([9u8; 32]).public_key(),
            )
            .account_id(),
            app_version: calimero_primitives::application::ApplicationId::from([0u8; 32]),
            method: METHOD.to_owned(),
            intent_hash: Warrant::intent_hash(METHOD, ARGS),
            // Cited nothing: this fixture exercises the context/expiry/intent
            // gate, which does not read the heads.
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 1,
            not_after,
            signature: [0u8; 64],
        }
    }

    fn refusal(err: &eyre::Report) -> String {
        err.downcast_ref::<IntentRefusal>().map_or_else(
            || format!("not an IntentRefusal: {err}"),
            ToString::to_string,
        )
    }

    fn outcome(
        returns: eyre::Result<Option<Vec<u8>>>,
    ) -> calimero_context_client::messages::ExecuteResponse {
        calimero_context_client::messages::ExecuteResponse {
            returns,
            logs: Vec::new(),
            events: Vec::new(),
            root_hash: calimero_primitives::hash::Hash::default(),
            artifact: Vec::new(),
            atomic: None,
            read_only_write_discarded: false,
        }
    }

    /// A method that returned `Err` is an error, exactly as JSON-RPC `execute`
    /// reports it: `FunctionCallError` carrying the method's message. It used to
    /// be dropped, so a contract refusal (a write outside the writer set, a read
    /// of a missing key) reached the client as `200 { returns: null }`.
    #[test]
    fn a_method_error_is_a_function_call_error_not_a_null_return() {
        let err = intent_response_data(outcome(Err(eyre::eyre!("key not found"))))
            .expect_err("a method that returned Err must not answer as a success");
        let wire = serde_json::to_value(&err).expect("the error serializes");
        assert_eq!(wire["type"], "FunctionCallError");
        assert_eq!(wire["data"], "key not found");
    }

    /// What `/intents` sends for that error: a `400` whose body holds exactly
    /// the object `/jsonrpc` puts under `error` for the same call (`type`,
    /// `data`), plus the admin API's `error` string, so a client reading either
    /// shape gets the method's message rather than a success.
    #[tokio::test]
    async fn a_method_error_answers_400_with_the_json_rpc_error_object() {
        let err = intent_response_data(outcome(Err(eyre::eyre!("key not found"))))
            .expect_err("a method that returned Err is an error");
        let json_rpc_error = serde_json::to_value(&err).expect("the error serializes");

        let response = crate::admin::service::method_error_response(&err);
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let mut body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(
            body["error"], "function call error: key not found",
            "{body}"
        );
        let _message = body.as_object_mut().expect("an object").remove("error");
        assert_eq!(body, json_rpc_error, "the rest is JSON-RPC's error object");
    }

    /// Output that is not JSON is the node path's `SerdeError`, not a `null`.
    #[test]
    fn undecodable_output_is_a_serde_error_not_a_null_return() {
        let err = intent_response_data(outcome(Ok(Some(b"not json".to_vec()))))
            .expect_err("output that is not JSON must not answer as null");
        let wire = serde_json::to_value(&err).expect("the error serializes");
        assert_eq!(wire["type"], "SerdeError");
    }

    /// A method's value, and a method returning nothing, still answer as before.
    #[test]
    fn a_returned_value_and_a_unit_return_are_unchanged() {
        let data = intent_response_data(outcome(Ok(Some(br#"{"v":1}"#.to_vec()))))
            .expect("a value is a success");
        assert_eq!(data.returns, Some(serde_json::json!({ "v": 1 })));
        let data = intent_response_data(outcome(Ok(None))).expect("unit is a success");
        assert_eq!(data.returns, None);
    }

    #[test]
    fn a_live_warrant_for_its_own_intent_is_authorised() {
        let ctx = ContextId::from([1u8; 32]);
        warrant_authorises_intent(&warrant(ctx, NOW + 60), ctx, METHOD, ARGS, NOW)
            .expect("a warrant that has not expired must authorise its own intent");
    }

    #[test]
    fn an_expired_warrant_is_refused() {
        let ctx = ContextId::from([1u8; 32]);
        let err = warrant_authorises_intent(&warrant(ctx, NOW - 1), ctx, METHOD, ARGS, NOW)
            .expect_err("a warrant whose not_after has passed must be refused");
        let msg = refusal(&err);
        assert!(msg.contains("expired"), "{msg}");
        // The message has to carry both clocks, or an operator cannot tell a
        // stale warrant from a skewed relay.
        assert!(msg.contains(&(NOW - 1).to_string()), "{msg}");
        assert!(msg.contains(&NOW.to_string()), "{msg}");
    }

    /// The boundary, pinned deliberately: the check is `<`, not `<=`.
    ///
    /// A warrant is live through the whole second it names. Flipping this to `<=`
    /// would expire warrants one second early — a change no other test would
    /// notice, because every other case is far from the boundary.
    #[test]
    fn a_warrant_expiring_exactly_now_still_authorises() {
        let ctx = ContextId::from([1u8; 32]);
        warrant_authorises_intent(&warrant(ctx, NOW), ctx, METHOD, ARGS, NOW)
            .expect("not_after == now is the last live second, not the first dead one");
    }

    /// Expiry must not mask a wrong-context warrant, or a relay presenting a
    /// warrant for another context would be told to "mint a fresh one".
    #[test]
    fn a_warrant_for_another_context_is_refused_before_the_clock_matters() {
        let err = warrant_authorises_intent(
            &warrant(ContextId::from([1u8; 32]), NOW - 1),
            ContextId::from([2u8; 32]),
            METHOD,
            ARGS,
            NOW,
        )
        .expect_err("a warrant for another context must be refused");
        let msg = refusal(&err);
        assert!(msg.contains("different context"), "{msg}");
        assert!(!msg.contains("expired"), "expiry must not shadow it: {msg}");
    }

    #[test]
    fn a_warrant_committing_to_other_arguments_is_refused() {
        let ctx = ContextId::from([1u8; 32]);
        let err = warrant_authorises_intent(
            &warrant(ctx, NOW + 60),
            ctx,
            METHOD,
            br#"{"key":"k","value":"SOMETHING ELSE"}"#,
            NOW,
        )
        .expect_err("a warrant is not a blank cheque for whatever the relay ran");
        assert!(
            refusal(&err).contains("does not cover"),
            "{}",
            refusal(&err)
        );
    }

    #[test]
    fn a_warrant_committing_to_another_method_is_refused() {
        let ctx = ContextId::from([1u8; 32]);
        let err = warrant_authorises_intent(&warrant(ctx, NOW + 60), ctx, "delete", ARGS, NOW)
            .expect_err("the method is part of the commitment");
        assert!(
            refusal(&err).contains("does not cover"),
            "{}",
            refusal(&err)
        );
    }

    /// A v1 warrant is the refusal every un-updated client now gets, and it has
    /// to read as a client error. It used to be a bare `eyre!` and so a 500,
    /// which sends whoever is debugging their signer to the wrong side of the
    /// wire.
    #[test]
    fn an_undecodable_warrant_is_a_client_error_naming_the_layout() {
        // 240 bytes of zeros: the v1 wire length, which v2 cannot parse.
        let v1_length = hex::encode(vec![0u8; 240]);

        let err = decode_warrant(&v1_length).expect_err("240 bytes cannot be a v2 warrant");
        let refusal = err
            .downcast_ref::<IntentRefusal>()
            .expect("a short warrant is Malformed, not an internal error");
        assert_eq!(refusal.status(), axum::http::StatusCode::BAD_REQUEST);

        let msg = refusal.to_string();
        // The byte count and the domain, so the message localizes the bug in the
        // caller's encoder rather than just saying no.
        assert!(msg.contains("240 bytes"), "{msg}");
        assert!(msg.contains("calimero.warrant.v2"), "{msg}");
    }

    /// The two role refusals are 403s with the messages a relay operator acts
    /// on, and a TEE replica is named as such rather than told to go and get a
    /// grant that would not help it.
    #[test]
    fn a_role_refusal_is_a_named_403() {
        let replica = IntentRefusal::from_role_refusal(&WarrantRefusal::ExecutorIsTeeReplica)
            .expect("a TEE replica is a role refusal");
        assert_eq!(replica.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(
            replica.to_string(),
            "this node is a TEE replica (ReadOnlyTee) and does not relay writes; the namespace \
             must admit relays with mode=relay"
        );

        let read_only_executor =
            IntentRefusal::from_role_refusal(&WarrantRefusal::ExecutorIsReadOnly)
                .expect("a read-only executor is a role refusal");
        assert_eq!(
            read_only_executor.status(),
            axum::http::StatusCode::FORBIDDEN
        );
        assert_eq!(
            read_only_executor.to_string(),
            "this node's role in this context is read-only (ReadOnly), so it does not relay writes"
        );

        let read_only = IntentRefusal::from_role_refusal(&WarrantRefusal::AuthorIsReadOnly)
            .expect("a read-only author is a role refusal");
        assert_eq!(read_only.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(
            read_only.to_string(),
            "the author's role in this context is read-only"
        );

        // A missing grant keeps its own message, and a replay its own.
        assert!(IntentRefusal::from_role_refusal(&WarrantRefusal::ExecutorMayNotAuthor).is_none());
        assert!(IntentRefusal::from_role_refusal(&WarrantRefusal::NonceAlreadySpent).is_none());
    }

    /// Relay clients read a message containing "nonce" as a retryable replay,
    /// so a role refusal must never say it — retrying cannot change a role.
    #[test]
    fn a_role_refusal_does_not_read_as_a_replay() {
        for refusal in [
            IntentRefusal::ExecutorIsTeeReplica,
            IntentRefusal::ExecutorIsReadOnly,
            IntentRefusal::AuthorIsReadOnly,
        ] {
            assert!(!refusal.to_string().contains("nonce"), "{refusal}");
        }
    }

    #[test]
    fn a_non_hex_warrant_is_also_a_client_error() {
        let err = decode_warrant("not hex at all").expect_err("non-hex must be refused");
        let refusal = err
            .downcast_ref::<IntentRefusal>()
            .expect("non-hex is Malformed, not an internal error");
        assert_eq!(refusal.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(refusal.to_string().contains("not hex"), "{refusal}");
    }

    /// Refused while decoding, before the delegation's signatures are verified.
    #[test]
    fn an_author_proof_with_a_long_handoff_chain_is_malformed() {
        use calimero_account::{
            AccountGenesis, AccountProof, DeviceCert, DeviceId, KemPublicKey, RootKeyHandoff,
            MAX_PRESENTED_HANDOFFS,
        };

        let root = PrivateKey::from([1; 32]);
        let genesis = AccountGenesis::new(root.public_key());
        let account = genesis.account_id();
        let statement = DeviceCert::sign(
            &root,
            account,
            DeviceId::mint(account, [0x22; 16]),
            &PrivateKey::from([2; 32]).public_key(),
            &KemPublicKey::from([9; 32]),
            0,
            0,
        )
        .expect("cert");
        let chain = (0..=MAX_PRESENTED_HANDOFFS as u32)
            .map(|from_epoch| RootKeyHandoff {
                account,
                from_epoch,
                new_root_sign_pk: root.public_key(),
                signature: [0; 64],
            })
            .collect();
        let proof = AccountProof {
            genesis,
            chain,
            statement,
        };

        let err = decode_author_proof(&hex::encode(borsh::to_vec(&proof).expect("borsh")))
            .expect_err("a chain this long is refused");
        assert!(
            matches!(
                err.downcast_ref::<IntentRefusal>(),
                Some(IntentRefusal::Malformed(_))
            ),
            "{err}"
        );
    }
}
