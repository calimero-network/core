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

/// Resolve a [`MemberIdentity`] — 32 bytes that may name either an account or a
/// signing key — into the account it means, plus the key if that is what it was.
///
/// The wire cannot say which it is. Both are 32 bytes and both render as the same
/// 64 hex characters, so the string carries no evidence and the only honest place
/// to decide is here, against the bindings this namespace actually holds.
///
/// A bound signing key resolves to its account, exactly as [`require`] does.
/// Anything else is taken as an account **as given** — no existence check, which
/// is the long-standing rule for accounts and the reason naming one works for
/// somebody this node has not converged on yet.
///
/// # What the ambiguity costs
///
/// A key that is *not yet bound here* reads as an account, and adds a member that
/// nothing will ever match. Under the older tagged form that same input was an
/// error, because the caller had declared it a key and [`require`] could refuse.
///
/// The trade is deliberate: the tag bought that one error by making every caller
/// declare, up front, something the node can look up — and it misfires precisely
/// when the caller is confused about what they are holding, which is when a
/// declaration is least trustworthy. Naming an account that does not exist yet is
/// already ordinary and already permitted here; this widens that door rather than
/// opening a new one.
///
/// # Errors
///
/// Propagates a datastore failure. An unresolvable identity is not an error — it
/// is read as an account.
pub fn resolve(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &calimero_primitives::identity::MemberIdentity,
) -> EyreResult<(AccountId, Option<calimero_primitives::identity::PublicKey>)> {
    let key = identity.as_key();

    if let Some(account) =
        calimero_governance_store::member_account_in_namespace(store, group_id, &key)?
    {
        return Ok((account, Some(key)));
    }

    Ok((identity.as_account(), None))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_primitives::identity::{MemberIdentity, PrivateKey};
    use calimero_store::db::InMemoryDB;
    use rand::rand_core::UnwrapErr;
    use rand::rngs::SysRng;

    use super::*;
    use crate::test_support::{account_for, enrol};

    /// A bound signing key resolves to its account, and says it was a key.
    #[test]
    fn a_bound_key_resolves_to_its_account() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x01; 32]);
        let key = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
        let account = enrol(&store, &ns, &key);

        let identity = MemberIdentity::from(key);
        let (resolved, as_key) = resolve(&store, &ns, &identity).expect("resolve");

        assert_eq!(resolved, account);
        assert_eq!(
            as_key,
            Some(key),
            "the caller needs to know it was a key, to address the delivery",
        );
    }

    /// Anything not bound here is taken as an account, as given.
    #[test]
    fn an_unbound_identity_is_taken_as_an_account() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x01; 32]);
        let account = account_for(&PrivateKey::random(&mut UnwrapErr(SysRng)).public_key());

        let identity = MemberIdentity::from(account);
        let (resolved, as_key) = resolve(&store, &ns, &identity).expect("resolve");

        assert_eq!(
            resolved, account,
            "an account this node has not converged on must still be addable",
        );
        assert_eq!(as_key, None);
    }

    /// The ambiguity, asserted rather than left implicit.
    ///
    /// An unbound KEY is indistinguishable from an account and reads as one. The
    /// tagged form could refuse this; plain hex cannot, and pretending otherwise
    /// would mean guessing from the string. What the node can check, it checks —
    /// see the test above.
    #[test]
    fn an_unbound_key_reads_as_an_account() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x01; 32]);
        // Never enrolled anywhere, so no binding names it.
        let key = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();

        let identity = MemberIdentity::from(key);
        let (resolved, as_key) = resolve(&store, &ns, &identity).expect("resolve");

        assert_eq!(as_key, None, "nothing here can tell it was a key");
        assert_eq!(
            resolved.as_bytes(),
            AsRef::<[u8; 32]>::as_ref(&key),
            "so its bytes are read as an account id verbatim",
        );
    }
}
