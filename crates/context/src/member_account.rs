//! Resolving a caller's signing key to the account it acts as.
//!
//! Governance rows name **accounts** — membership, roles, capabilities, deny
//! entries, ownership. A request arriving at this crate names a **key**: the
//! node's namespace identity, or whichever identity signed for it. Something has
//! to bridge the two, and this is the only place that does it, so every handler
//! refuses an unresolvable key the same way.
//!
//! Resolution runs at the NAMESPACE, not at the group the request targets.
//! Bindings are written where the credential arrived, which is the namespace a
//! member joined; a subgroup holds none of its own, so asking it would refuse
//! every legitimate subgroup operation. See
//! [`member_account_in_namespace`](calimero_governance_store::member_account_in_namespace).

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_store::Store;
use eyre::Result as EyreResult;

/// The account `identity` acts as in `group_id`'s namespace.
///
/// # Errors
///
/// Fails when `identity` is bound to no account here. That is deliberate and it
/// is the whole point of returning a `Result` rather than an `Option`: the
/// tempting fallback is a key-derived stand-in, which would let the request
/// succeed against a principal that holds no grant and that the caller's own
/// later writes will not present. The grant would look recorded and match
/// nothing.
///
/// Since every member is bound by the op that admits it — a join for a joiner,
/// the genesis for a founder — an unresolvable key is a genuine anomaly here,
/// not an ordinary state a caller should be written to tolerate.
pub fn require(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &calimero_primitives::identity::PublicKey,
) -> EyreResult<AccountId> {
    calimero_governance_store::member_account_in_namespace(store, group_id, identity)?.ok_or_else(
        || {
            eyre::eyre!(
                "identity {identity} is bound to no account in the namespace owning \
                 group {group_id:?}; it holds no membership or capability grant here"
            )
        },
    )
}
