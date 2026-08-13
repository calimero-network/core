//! Resolving an authenticated caller's key to the account it acts as.
//!
//! Membership and capability rows name **accounts**; a request arrives carrying
//! a **key** (the authenticated client key, or this node's namespace identity).
//! `calimero-context-client` cannot bridge the two — the binding rows live in
//! `calimero-governance-store`, which depends on it — so its `has_member` takes
//! the account as a parameter and the server supplies it. This is where.

use calimero_account::AccountId;
use calimero_context_client::client::ContextClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tracing::warn;

/// The account `key` acts as in the group owning `context_id`, if any.
///
/// `None` when the context belongs to no group, or when the key is bound to no
/// account there. Both collapse to the same thing for every caller: the
/// account-keyed membership arm cannot answer, so it abstains rather than
/// guessing — and `has_member`'s key-keyed `ContextIdentity` arm still answers
/// for a context the caller joined directly.
pub(crate) fn for_context(
    ctx_client: &ContextClient,
    context_id: &ContextId,
    key: &PublicKey,
) -> Option<AccountId> {
    let store = ctx_client.datastore();
    // A store fault and an honest absence both abstain, and they should: this
    // arm cannot answer either way, and failing closed is the only safe reading
    // on a path that gates execute and subscribe.
    //
    // They must not look the same in the LOGS, though. Silent, they are
    // indistinguishable from ordinary non-membership, so a store fault degrades
    // every caller to unresolved with nothing to see — an outage that reads as a
    // quiet permissions change. Warn on the fault, say nothing on the absence.
    let group_id = calimero_governance_store::get_group_for_context(store, context_id)
        .unwrap_or_else(|err| {
            warn!(
                %err, %context_id,
                "resolving the caller's account: could not read the context's group; \
                 treating the caller as unresolved"
            );
            None
        })?;
    calimero_governance_store::member_account_in_namespace(store, &group_id, key).unwrap_or_else(
        |err| {
            warn!(
                %err, %context_id, %key,
                "resolving the caller's account: could not read the account binding; \
                 treating the caller as unresolved"
            );
            None
        },
    )
}
