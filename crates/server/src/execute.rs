//! Shared execution path for context method calls.
//!
//! Both the JSON-RPC server (`crate::jsonrpc`) and the WebSocket server
//! (`crate::ws`) accept `execute` (query/mutate) requests. The actual work —
//! resolving the executor identity, invoking the runtime, and collecting the
//! result — is identical for both transports, so it lives here and each
//! transport just adapts its own request/response envelope around it.

use std::pin::pin;

use calimero_context_client::client::ContextClient;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{ExecutionError, ExecutionRequest, ExecutionResponse};
use futures_util::StreamExt;
use tracing::{error, info};

/// Who is making an execute call, as determined by the auth layer.
///
/// Using an explicit enum instead of `Option<PublicKey>` makes the bypass path
/// auditable at every call site: `NodeOwner` means the auth layer positively
/// confirmed the caller via a non-key method (e.g. embedded username/password),
/// not simply that no key was provided.
#[derive(Debug)]
pub(crate) enum CallerIdentity<'a> {
    /// A specific public key, extracted from the verified auth token.
    /// The membership check runs against this key.
    Key(&'a PublicKey),
    /// The node owner, authenticated via a non-key method (e.g. embedded
    /// username/password auth). The auth layer already validated the token;
    /// the caller is implicitly authorized for all contexts.
    NodeOwner,
}

/// Execute a context method call against the runtime.
///
/// `caller` identifies who is making the call after the auth layer verified
/// their token. `CallerIdentity::Key` triggers a context-membership check
/// before execution. `CallerIdentity::NodeOwner` skips the check — the auth
/// layer already confirmed the caller is the node owner.
///
/// After the membership check passes, the executor identity is auto-resolved:
/// each node owns exactly one identity per context (the namespace identity),
/// so callers never specify it.
///
/// # Security note — caller vs executor identity
///
/// When `CallerIdentity::Key` is used, the **caller's key** gates access (the
/// membership check). However, the **executor identity** passed to the WASM
/// runtime is the node's owned key for the context, not the caller's key.
/// Applications that inspect `executor` inside WASM will see the node's owned
/// identity, which may have different in-application permissions than the
/// caller's identity. This is an intentional design: the node always executes
/// on behalf of its own namespace identity; the caller's key is used only to
/// authorise the call.
pub(crate) async fn execute_request(
    ctx_client: &ContextClient,
    caller: CallerIdentity<'_>,
    request: ExecutionRequest,
) -> Result<ExecutionResponse, ExecutionError> {
    // Verify the caller is a member of the target context before doing
    // anything else. This prevents a valid token from being used to execute
    // against contexts the caller has no membership in.
    if let CallerIdentity::Key(key) = caller {
        let is_member = ctx_client
            .has_member(&request.context_id, key)
            .map_err(|err| {
                error!(%err, "Membership lookup failed during execute");
                ExecutionError::FunctionCallError(
                    "Internal error during membership verification".to_owned(),
                )
            })?;

        if !is_member {
            return Err(ExecutionError::FunctionCallError(
                "Caller is not a member of this context".to_owned(),
            ));
        }
    }

    let args =
        serde_json::to_vec(&request.args_json).map_err(|err| ExecutionError::SerdeError {
            message: err.to_string(),
        })?;

    // Always auto-resolve the executor identity. Each node has exactly one
    // owned identity per context (the namespace identity). The caller should
    // not need to specify it.
    let executor = {
        let members = ctx_client.get_context_members(&request.context_id, Some(true));
        let mut members = pin!(members);
        match members.next().await {
            Some(Ok((public_key, _))) => public_key,
            // Keep the "no owned identity" and "lookup failed" cases distinct so
            // a store/network error during resolution isn't masked as a missing
            // identity.
            Some(Err(err)) => {
                return Err(ExecutionError::FunctionCallError(format!(
                    "Failed to resolve owned identity for this context: {err}"
                )));
            }
            None => {
                return Err(ExecutionError::FunctionCallError(
                    "No owned identity found for this context".to_string(),
                ));
            }
        }
    };

    let outcome = ctx_client
        .execute(
            &request.context_id,
            &executor,
            request.method,
            args,
            request.substitute,
            None,
        )
        .await
        .map_err(ExecutionError::ExecuteError)?;

    let log_index_width = outcome.logs.len().checked_ilog10().unwrap_or(0) as usize + 1;
    for (i, log) in outcome.logs.iter().enumerate() {
        info!("execution log {i:>log_index_width$}| {}", log);
    }

    let Some(returns) = outcome
        .returns
        .map_err(|e| ExecutionError::FunctionCallError(e.to_string()))?
    else {
        return Ok(ExecutionResponse::new(None));
    };

    let returns = serde_json::from_slice(&returns).map_err(|err| ExecutionError::SerdeError {
        message: err.to_string(),
    })?;

    Ok(ExecutionResponse::new(Some(returns)))
}
