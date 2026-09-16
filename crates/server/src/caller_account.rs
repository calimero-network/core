//! Resolving an authenticated caller's key to the account it acts as.
//!
//! Membership and capability rows name **accounts**; a request arrives carrying
//! a **key** (the authenticated client key, or this node's namespace identity).
//! `calimero-context-client` cannot bridge the two — the binding rows live in
//! `calimero-governance-store`, which depends on it — so its `has_member` takes
//! the account as a parameter and the server supplies it. This is where.

use calimero_account::AccountId;
use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::MembershipRepository;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tracing::warn;

/// Who a connection acts as, for the gates that decide what it may observe.
///
/// Two shapes, because two login paths anchor a session differently and the
/// difference is not cosmetic:
///
/// * [`Self::Key`] — a verified client key. Membership rows are account-keyed,
///   so the key is *resolved* to an account through the namespace binding rows
///   (that is what the rest of this module is for), and a key bound to no
///   account there resolves to nothing.
/// * [`Self::Account`] — an `account_proof` session (#3930). It **is** an
///   account: its identity is its own cryptographic anchor, it deliberately
///   persists no key row on this node, and there is nothing to resolve.
///
/// Before this existed the gates took a bare `PublicKey`, so an
/// account-anchored caller had nothing to present — every subscription it
/// asked for was refused by a resolution that could not have succeeded. That
/// is #3942: a delegated device could read and write but never learn that
/// anything had changed.
#[derive(Clone, Copy, Debug)]
pub(crate) enum EventCaller {
    Key(PublicKey),
    Account(AccountId),
}

/// Whether `account` is a member of the group owning `context_id`.
///
/// The account-keyed sibling of the key-keyed `ContextClient::has_member`,
/// for a caller that never had a key to be keyed by.
///
/// **This is deliberately the same rule the delegated read runs** (#3931, the
/// `read_as` arm in `crates/context/src/handlers/execute/mod.rs`): resolve the
/// context's group, then ask `is_member`. Two implementations of "may this
/// account see this context" would be free to drift, and the drift would show
/// up as a stream delivering what a read refuses, or the reverse — which is
/// exactly the shape of bug nobody notices until it is a disclosure.
///
/// Fails **closed** on both of its "cannot answer" cases, and they are
/// different cases:
///
/// * a store fault is warned and denied, because an outage must not read as a
///   quiet permissions change;
/// * a context owned by **no group** is denied silently, because there is no
///   membership to check against and "no rule, therefore allowed" is how a
///   stranger gets in.
pub(crate) fn account_is_context_member(
    ctx_client: &ContextClient,
    context_id: &ContextId,
    account: &AccountId,
) -> bool {
    let store = ctx_client.datastore();
    let group_id = match calimero_governance_store::get_group_for_context(store, context_id) {
        Ok(Some(group_id)) => group_id,
        Ok(None) => return false,
        Err(err) => {
            warn!(
                %err, %context_id, %account,
                "account membership: could not read the context's group; denying observation"
            );
            return false;
        }
    };
    MembershipRepository::new(store)
        .is_member(&group_id, account)
        .unwrap_or_else(|err| {
            warn!(
                %err, %context_id, %account,
                "account membership: could not read the membership row; denying observation"
            );
            false
        })
}

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

/// The account `key` acts as in `group_id`, if any.
///
/// The group-keyed sibling of [`for_context`], for the gates that subscribe a
/// connection to a group. Both the capability and the admin authority those
/// gates test are held by the account, so the key resolves first and a key
/// bound to none is denied by both.
pub(crate) fn for_group(
    ctx_client: &ContextClient,
    group_id: &ContextGroupId,
    key: &PublicKey,
) -> Option<AccountId> {
    calimero_governance_store::member_account_in_namespace(ctx_client.datastore(), group_id, key)
        .unwrap_or_else(|err| {
            warn!(
                %err, ?group_id, %key,
                "resolving the caller's account: could not read the account binding; \
                 treating the caller as unresolved"
            );
            None
        })
}
