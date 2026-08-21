//! Shared execution path for context method calls.
//!
//! Both the JSON-RPC server (`crate::jsonrpc`) and the WebSocket server
//! (`crate::ws`) accept `execute` (query/mutate) requests. The actual work —
//! resolving the executor identity, invoking the runtime, and collecting the
//! result — is identical for both transports, so it lives here and each
//! transport just adapts its own request/response envelope around it.

use std::pin::pin;

use calimero_account::AccountId;
use calimero_context_client::client::ContextClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{ExecutionError, ExecutionRequest, ExecutionResponse};
use futures_util::StreamExt;
use tracing::{debug, error, info};

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

/// The outcome of the membership gate: whether `caller` may act, and the account
/// they were authorized as.
///
/// The account is carried out rather than dropped because the guest needs it
/// (`env::caller_account()`). It is `None` whenever no account names this caller
/// — a `NodeOwner`, or a key authorized by `has_member`'s key-keyed arm without a
/// binding — and `None` must never be widened into the node's own account.
pub(crate) struct Authorization {
    pub authorized: bool,
    pub account: Option<AccountId>,
}

/// Whether `caller` may act on `context_id`, and as which account.
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
) -> eyre::Result<Authorization> {
    match *caller {
        CallerIdentity::Key(key) => {
            let account = crate::caller_account::for_context(ctx_client, context_id, key);
            Ok(Authorization {
                authorized: ctx_client.has_member(context_id, key, account)?,
                account,
            })
        }
        CallerIdentity::NodeOwner => Ok(Authorization {
            authorized: true,
            account: None,
        }),
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
/// # Security note — three identities, deliberately distinct
///
/// The **executor** passed to the runtime is this node's owned key for the
/// context, never the caller's: the node holds no private key but its own, and
/// the executor doubles as the replica id seeding CRDT slots. The **account**
/// the guest reads as `env::account_id()` is likewise this node's, since a delta
/// is attributed by its signer. Neither may be substituted for the caller.
///
/// The caller is surfaced as a third value, `env::caller_account()`, carried
/// here from the membership gate that already resolved it. It is what an app
/// should test for per-member permissions; the other two answer questions about
/// this node, not about who asked.
pub(crate) async fn execute_request(
    ctx_client: &ContextClient,
    caller: CallerIdentity<'_>,
    request: ExecutionRequest,
) -> Result<ExecutionResponse, ExecutionError> {
    // Verify the caller is a member of the target context before doing
    // anything else. This prevents a valid token from being used to execute
    // against contexts the caller has no membership in.
    let caller_account = if matches!(caller, CallerIdentity::Key(_)) {
        let authorization = caller_authorized_for_context(ctx_client, &request.context_id, &caller)
            .map_err(|err| {
                error!(%err, "Membership lookup failed during execute");
                ExecutionError::FunctionCallError(
                    "Internal error during membership verification".to_owned(),
                )
            })?;

        if !authorization.authorized {
            return Err(ExecutionError::FunctionCallError(
                "Caller is not a member of this context".to_owned(),
            ));
        }
        authorization.account
    } else {
        debug!(context_id=%request.context_id, method=%request.method, "NodeOwner-privileged execute: membership check skipped");
        None
    };

    let args =
        serde_json::to_vec(&request.args_json).map_err(|err| ExecutionError::SerdeError {
            message: err.to_string(),
        })?;

    // Always auto-resolve the executor identity. Each node has exactly one
    // owned identity per context (the namespace identity). The caller should
    // not need to specify it, and could not be substituted for it: executing as
    // the caller's key would need that key's private half, which this node does
    // not hold. Per-member permissions read `env::caller_account()` instead.
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
        .execute_with_origin(
            &request.context_id,
            &executor,
            request.method,
            args,
            None,
            caller_account,
            None,
            0,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::{AccountGenesis, DeviceCert, DeviceId, KemPublicKey};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::events::NodeEvent;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tokio::sync::broadcast;

    use super::*;

    /// Bind `sign_pk` to a fresh account in `namespace`, the way an applied join
    /// op does, and return that account.
    ///
    /// Mirrors governance-store's own enrolment fixture, which is crate-private
    /// there. The KEM half is arbitrary because nothing here opens an envelope —
    /// only the binding lookup matters.
    fn enrol(store: &Store, namespace: &ContextGroupId, sign_pk: &PublicKey) -> AccountId {
        let key_bytes: [u8; 32] = *(*sign_pk);
        let root_sk = PrivateKey::from(key_bytes);
        let genesis = AccountGenesis::new(root_sk.public_key());
        let cert = DeviceCert::sign(
            &root_sk,
            genesis.account_id(),
            DeviceId::from(key_bytes),
            sign_pk,
            &KemPublicKey::from([0; 32]),
            0,
            0,
        )
        .expect("the account root signs its own device cert");
        let account = cert.account;
        let bindings = calimero_governance_store::AccountBindingRepository::new(store);
        let _ = bindings
            .apply_link(namespace, &genesis, &[], &cert)
            .expect("store the binding");
        bindings
            .record_endorser(namespace, account, &account)
            .expect("record the endorser");
        // The membership row the account-keyed arm of `has_member` reads. A join
        // writes both; the binding alone resolves the key but admits nobody.
        calimero_governance_store::MembershipRepository::new(store)
            .add_member(namespace, &account, GroupMemberRole::Member)
            .expect("add the member row");
        account
    }

    /// A `ContextClient` over a fresh in-memory store, plus the store to seed.
    ///
    /// The returned `TempDir` backs the blob store and must outlive the client.
    async fn client() -> (ContextClient, Store, tempfile::TempDir) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (event_sender, _) = broadcast::channel::<NodeEvent>(16);
        let (node_client, blob_dir) =
            crate::test_support::test_node_client(&store, LazyRecipient::new(), event_sender).await;
        let ctx_client = ContextClient::new(store.clone(), node_client, LazyRecipient::new());
        (ctx_client, store, blob_dir)
    }

    /// The gate hands back the account it authorized, instead of dropping it.
    ///
    /// The regression guard for the defect `caller_account` exists to close: this
    /// account was resolved to answer "may this caller act?" and then discarded,
    /// leaving the guest to derive a different one from this node's own device
    /// row. Carrying it out is what makes `env::caller_account()` answerable.
    #[tokio::test]
    async fn the_gate_returns_the_account_it_authorized() {
        let (ctx_client, store, _blob_dir) = client().await;
        let namespace = ContextGroupId::from([0xAA; 32]);
        let context_id = ContextId::from([0x77; 32]);
        calimero_governance_store::register_context_in_group(&store, &namespace, &context_id)
            .expect("register context");

        let caller_key = PublicKey::from([0x42; 32]);
        let caller_account = enrol(&store, &namespace, &caller_key);

        let authorization = caller_authorized_for_context(
            &ctx_client,
            &context_id,
            &CallerIdentity::Key(&caller_key),
        )
        .expect("gate must not error");

        assert!(authorization.authorized, "an enrolled member may act");
        assert_eq!(
            authorization.account,
            Some(caller_account),
            "the gate must surface who it authorized, not drop it"
        );
    }

    /// A node owner names no separate account, and must not borrow one.
    ///
    /// Every call made today takes this branch, so `None` here is what keeps them
    /// reading as "no direct caller to attribute" rather than silently presenting
    /// this node as the requester.
    #[tokio::test]
    async fn a_node_owner_authorizes_with_no_account() {
        let (ctx_client, _store, _blob_dir) = client().await;

        let authorization = caller_authorized_for_context(
            &ctx_client,
            &ContextId::from([0x77; 32]),
            &CallerIdentity::NodeOwner,
        )
        .expect("gate must not error");

        assert!(authorization.authorized);
        assert_eq!(authorization.account, None);
    }
}
