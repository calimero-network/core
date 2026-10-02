//! Shared execution path for context method calls.
//!
//! Both the JSON-RPC server (`crate::jsonrpc`) and the WebSocket server
//! (`crate::ws`) accept `execute` (query/mutate) requests. The actual work —
//! resolving the executor identity, invoking the runtime, and collecting the
//! result — is identical for both transports, so it lives here and each
//! transport just adapts its own request/response envelope around it.

use std::pin::pin;

use calimero_context_client::client::ContextClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{ExecutionError, ExecutionRequest, ExecutionResponse};
use futures_util::StreamExt;
use mero_auth::auth::permissions::{
    ContextPermission, Permission, PermissionValidator, ResourceScope, UserScope,
};
use tracing::{debug, error, info, warn};

use crate::auth::GrantedPermissions;

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

/// Whether the caller's token lets it call `method` in `context_id`.
///
/// `granted` is what the token carries: the embedded guard's, or in proxy mode
/// what the proxy named in `X-Auth-Permissions` (`crate::proxy_permissions`).
/// With none, only a node whose guard did not run serves the call.
fn may_execute(
    auth_enabled: bool,
    granted: Option<&GrantedPermissions>,
    context_id: &ContextId,
    method: &str,
) -> bool {
    let Some(granted) = granted else {
        return !auth_enabled;
    };
    let required = [Permission::Context(ContextPermission::Execute(
        ResourceScope::Specific(vec![context_id.to_string()]),
        UserScope::Any,
        Some(method.to_owned()),
    ))];
    PermissionValidator::new().validate_permissions(&granted.0, &required)
}

/// Whether `caller` may act on `context_id`.
///
/// `CallerIdentity::Key` is checked against context membership (resolving the
/// account the key acts as first, since membership rows are account-keyed).
/// `CallerIdentity::NodeOwner` is always authorized — the auth layer already
/// confirmed the caller owns this node.
///
/// Shared by every transport-level handler that gates on context membership
/// (`execute`, `set_ephemeral`) so the gate cannot drift
/// between them. `Err` means the lookup itself failed and the caller must
/// fail closed, not that the caller is a non-member.
pub(crate) fn caller_authorized_for_context(
    ctx_client: &ContextClient,
    context_id: &ContextId,
    caller: &CallerIdentity<'_>,
) -> eyre::Result<bool> {
    match *caller {
        CallerIdentity::Key(key) => {
            let account = crate::caller_account::for_context(ctx_client, context_id, key);
            ctx_client.has_member(context_id, key, account)
        }
        CallerIdentity::NodeOwner => Ok(true),
    }
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
    auth_enabled: bool,
    granted: Option<&GrantedPermissions>,
    request: ExecutionRequest,
) -> Result<ExecutionResponse, ExecutionError> {
    // The token's own scope first, before anything is read on the call's
    // behalf. Checked here, in the one function both transports call: `/ws` is
    // admitted on `context:subscribe` and `/jsonrpc` on any `context:execute`,
    // and neither route can see the context or method a call names. It holds
    // for the node owner too, whose client keys are minted for some purpose.
    if !may_execute(auth_enabled, granted, &request.context_id, &request.method) {
        warn!(context_id=%request.context_id, method=%request.method, "refusing execute: token lacks context:execute");
        return Err(ExecutionError::FunctionCallError(
            "this caller's token does not grant context:execute for this call".to_owned(),
        ));
    }

    // Verify the caller is a member of the target context before doing
    // anything else. This prevents a valid token from being used to execute
    // against contexts the caller has no membership in.
    if matches!(caller, CallerIdentity::Key(_)) {
        let is_member = caller_authorized_for_context(ctx_client, &request.context_id, &caller)
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
    } else {
        debug!(context_id=%request.context_id, method=%request.method, "NodeOwner-privileged execute: membership check skipped");
    }

    let args =
        serde_json::to_vec(&request.args_json).map_err(|err| ExecutionError::SerdeError {
            message: err.to_string(),
        })?;

    // Always auto-resolve the executor identity. Each node has exactly one
    // owned identity per context (the namespace identity). The caller should
    // not need to specify it.
    // TODO: pass the caller's key as the executor identity so that WASM
    // applications that enforce per-member permissions see the actual caller
    // rather than the node's owned key. Until then, a context member whose
    // in-WASM permissions are lower than the node-owner's can execute at
    // the higher privilege level. Tracked as a known limitation.
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
        .execute(&request.context_id, &executor, request.method, args, None)
        .await
        .map_err(ExecutionError::ExecuteError)?;

    refuse_discarded_write(request.context_id, &outcome)?;

    let log_index_width = outcome.logs.len().checked_ilog10().unwrap_or(0) as usize + 1;
    for (i, log) in outcome.logs.iter().enumerate() {
        info!("execution log {i:>log_index_width$}| {}", log);
    }

    Ok(ExecutionResponse::new(method_output(outcome.returns)?))
}

/// What a method's run answers a client with: its JSON output, nothing, or the
/// method's own error as `FunctionCallError`.
///
/// The one mapping every route that runs a method for a client uses — JSON-RPC
/// and WebSocket `execute`, the delegated `/intents`, and the account `/query`
/// — so a method that returned `Err` is reported the same way on all of them.
/// `/intents` used to drop the `Err` and answer `200 { returns: null }`, which a
/// client cannot tell from a method that succeeded and returned nothing.
pub(crate) fn method_output(
    returns: eyre::Result<Option<Vec<u8>>>,
) -> Result<Option<serde_json::Value>, ExecutionError> {
    let Some(returns) = returns.map_err(|e| ExecutionError::FunctionCallError(e.to_string()))?
    else {
        return Ok(None);
    };

    serde_json::from_slice(&returns)
        .map(Some)
        .map_err(|err| ExecutionError::SerdeError {
            message: err.to_string(),
        })
}

/// Refuse a call whose writes the execute path discarded because this node is
/// read-only in the context, rather than answer it as a success.
///
/// The discard itself stays where it is: the node's own event handlers run
/// through the same path, and on a read-only replica their writes are meant to
/// be dropped quietly. A client that asked for the write is owed the refusal.
fn refuse_discarded_write(
    context_id: ContextId,
    outcome: &calimero_context_client::messages::ExecuteResponse,
) -> Result<(), ExecutionError> {
    if outcome.read_only_write_discarded {
        return Err(ExecutionError::ReadOnlyWriteRefused { context_id });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use calimero_context_client::messages::ExecuteResponse;
    use calimero_primitives::context::ContextId;
    use calimero_primitives::hash::Hash;
    use calimero_server_primitives::jsonrpc::ExecutionError;

    use super::refuse_discarded_write;

    fn response(read_only_write_discarded: bool) -> ExecuteResponse {
        ExecuteResponse {
            returns: Ok(None),
            logs: Vec::new(),
            events: Vec::new(),
            root_hash: Hash::default(),
            artifact: Vec::new(),
            atomic: None,
            read_only_write_discarded,
        }
    }

    /// A write a read-only node discarded reaches the client as a refusal it
    /// can tell apart, never as the success it used to be.
    #[test]
    fn a_discarded_read_only_write_is_refused() {
        let context_id = ContextId::from([7; 32]);
        let refusal = refuse_discarded_write(context_id, &response(true))
            .expect_err("a discarded write must not succeed");
        assert!(matches!(
            refusal,
            ExecutionError::ReadOnlyWriteRefused { context_id: refused } if refused == context_id
        ));
        let wire = serde_json::to_value(&refusal).expect("the refusal serializes");
        assert_eq!(wire["type"], "ReadOnlyWriteRefused");
        assert!(
            refusal.to_string().contains("read-only"),
            "the message names the cause; got: {refusal}"
        );
    }

    /// Anything else answers as it did.
    #[test]
    fn a_kept_or_read_only_call_is_not_refused() {
        assert!(refuse_discarded_write(ContextId::from([7; 32]), &response(false)).is_ok());
    }
}

#[cfg(test)]
mod grant_tests {
    use std::sync::Arc;

    use calimero_primitives::context::ContextId;

    use super::may_execute;
    use crate::auth::GrantedPermissions;

    fn granted(permissions: &[&str]) -> GrantedPermissions {
        GrantedPermissions(
            permissions
                .iter()
                .map(|p| (*p).to_owned())
                .collect::<Arc<[_]>>(),
        )
    }

    #[test]
    fn a_grant_scoped_to_one_context_covers_only_that_context() {
        let ctx = ContextId::from([1u8; 32]);
        let other = ContextId::from([2u8; 32]);
        let scoped = granted(&[&format!("context:execute[{ctx}]")]);

        assert!(may_execute(true, Some(&scoped), &ctx, "set"));
        assert!(!may_execute(true, Some(&scoped), &other, "set"));
    }

    #[test]
    fn a_method_scoped_grant_covers_only_that_method() {
        let ctx = ContextId::from([1u8; 32]);
        let scoped = granted(&[&format!("context:execute[{ctx},,set]")]);

        assert!(may_execute(true, Some(&scoped), &ctx, "set"));
        assert!(!may_execute(true, Some(&scoped), &ctx, "delete"));
    }

    #[test]
    fn subscribe_alone_and_admin_and_global_execute() {
        let ctx = ContextId::from([1u8; 32]);

        assert!(!may_execute(
            true,
            Some(&granted(&["context:subscribe"])),
            &ctx,
            "set"
        ));
        assert!(may_execute(
            true,
            Some(&granted(&["context:execute"])),
            &ctx,
            "set"
        ));
        assert!(may_execute(true, Some(&granted(&["admin"])), &ctx, "set"));
    }

    #[test]
    fn no_token_on_the_socket_is_served_only_when_the_node_runs_no_auth() {
        let ctx = ContextId::from([1u8; 32]);

        assert!(!may_execute(true, None, &ctx, "set"));
        assert!(may_execute(false, None, &ctx, "set"));
    }
}
