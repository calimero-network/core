//! The one admission path every delegated statement goes through at the cut:
//! the standing of the author and the executor, and the replay ledger.
//!
//! # Why it is shaped this way
//!
//! **Three gates asked the same five questions.** A delegated delta
//! ([`crate::warrant_gate`]), a delegated governance op
//! ([`crate::delegation_gate`]) and a delegated context registration
//! ([`crate::creation_gate`]) each checked, in the same order: is the author's
//! device revoked here, is the executor's, is the author a member who may write,
//! may the executor act for members here, and is the nonce unspent. Each wrote
//! the answer out again, with its own copy of the nonce-window read, so the
//! checks agreed by being kept in step by hand. That is the shape the envelope
//! verifier was already built to avoid — a rule written three times is three
//! chances for one copy to accept what the others refuse, and at the cut that
//! is divergence rather than a rejection.
//!
//! So the questions live here once, and each gate keeps only what is genuinely
//! its own: what the statement must commit to (an intent hash, an op's
//! delegable form, a creation's seed and labels), and the op-specific authority
//! its inner handler or capability check decides.
//!
//! **Each gate keeps its own refusal type.** Operators, the relay's HTTP
//! mapping and the tests all match on [`WarrantRefusal`], `DelegationRefusal`
//! and `CreationRefusal` by name, and the messages differ on purpose (a
//! delegated write talks about "this context", a governance op about "this
//! group"). [`AdmissionRefusal`] names the cases this module can produce, and
//! each gate says which of its variants each one is — the same associated-const
//! pattern `calimero_account::RootSigned` uses for the same reason.
//!
//! **The ledger is chosen by the warrant's scope, not by the caller.** A
//! delegated write and a creation spend in a context's ledger, a governance op
//! in a ledger derived from its group. Deriving it from
//! [`WarrantStatement::scope`] rather than taking it as an argument means a
//! call site cannot spend a warrant in the wrong ledger — the persisted keys are
//! exactly the ones the three gates wrote before, which the tests below pin.
//!
//! # What is deliberately NOT checked here
//!
//! **`not_after`.** Wall-clock expiry must not gate an apply: peers apply at
//! different times, so a warrant that expired between two receivers would be
//! accepted by one and refused by the other, and authorization would stop
//! converging. The relay checks it at its API boundary, where one clock decides
//! and nothing has converged yet. See [`crate::warrant_gate`].

use calimero_account::{AccountId, Delegated, WarrantScope, WarrantStatement};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{domain_hash, PublicKey};
use calimero_store::{key, types, Store};
use eyre::Result as EyreResult;

use crate::account_bindings::AccountBindingRepository;
use crate::capabilities::CapabilitiesRepository;
use crate::membership::MembershipPath;
use crate::warrant_gate::WarrantRefusal;
use crate::{MembershipRepository, NamespaceRepository, PermissionChecker};

/// Domain for the ledger scope a group's delegated-governance nonces live
/// under. Hashing the group under its own domain keeps the scope out of the
/// context-id space the same ledger column is otherwise keyed by.
const GOVERNANCE_LEDGER_DOMAIN: &[u8] = b"calimero.governance-warrant.ledger.v1";

/// The refusals admission can produce, as each gate names them.
///
/// Every constant is a case the shared path decides; [`Self::executor`] wraps
/// the executor's standing, which is already a typed [`WarrantRefusal`].
pub(crate) trait AdmissionRefusal: std::error::Error + Send + Sync + 'static {
    /// The author's device has been revoked in the group.
    const AUTHOR_DEVICE_REVOKED: Self;
    /// The executor's device has been revoked in the group.
    const EXECUTOR_DEVICE_REVOKED: Self;
    /// The author's account is not a member of the group.
    const AUTHOR_NOT_A_MEMBER: Self;
    /// The author's role in the group is read-only.
    const AUTHOR_IS_READ_ONLY: Self;
    /// The warrant's nonce is already spent, or too old to judge.
    const NONCE_SPENT: Self;
    /// The executor may not act for members in the group.
    fn executor(refusal: WarrantRefusal) -> Self;
}

/// What the author must be, beyond "not revoked", for the statement to be
/// admitted.
pub(crate) enum AuthorRule<'a, R> {
    /// An effective member (deny-list aware, inheritance included) whose role
    /// may write. The rule for a delegated write, which has no op cut to ask
    /// an admin question at.
    Member,
    /// As [`Self::Member`], except that an account with no member row passes
    /// if it is an admin of the group at the op's cut — the genesis admin has
    /// no row. Optionally the author must also hold `capability` (or be an
    /// admin), refused as the given value.
    MemberOrAdmin {
        /// The checker for the op being applied, carrying its cut.
        permissions: &'a PermissionChecker<'a>,
        /// A capability the author must hold, and how its absence is refused.
        capability: Option<(MemberCapabilities, R)>,
    },
}

/// Admit a delegated statement in `group_id`: both devices are live, the
/// author may write, the executor may act for members, and the nonce is
/// unspent.
///
/// **Read-only.** It writes nothing; the nonce is spent by [`spend_nonce`]
/// after the change applied. Why the two are separate, and why both must run
/// under the apply's lock, is set out on
/// [`crate::warrant_gate::check_delegated_delta`].
///
/// The bundle is assumed authentic — its `verify` has already run (for a
/// delta, inside the envelope verifier). The order of the checks is part of
/// the contract: when several fail, the first is the one reported, and every
/// peer must report the same one.
///
/// # Errors
/// `R` for a statement that must not be admitted, or a store failure.
pub(crate) fn admit<W: WarrantStatement, R: AdmissionRefusal>(
    store: &Store,
    group_id: &ContextGroupId,
    delegation: &Delegated<W>,
    author: AuthorRule<'_, R>,
) -> EyreResult<()> {
    let warrant = &*delegation.warrant;

    let bindings = AccountBindingRepository::new(store);
    if bindings.is_revoked(group_id, delegation.author_proof.statement.device)? {
        return Err(R::AUTHOR_DEVICE_REVOKED.into());
    }
    if bindings.is_revoked(group_id, delegation.executor_proof.statement.device)? {
        return Err(R::EXECUTOR_DEVICE_REVOKED.into());
    }

    // The author's ACCOUNT, not the device key: bindings are per group, and a
    // thin client's device never joins one. The certificate is what ties the key
    // to the account; this asks whether that account may write here.
    //
    // `effective_role` rather than a bare `check_path`: it is deny-list aware,
    // so an author kicked from an Open subgroup (where the deny entry IS the
    // removal) is not a member there, and it carries the role the read-only
    // check needs — including a role inherited from an ancestor, which a
    // direct-row read would miss.
    let author_account = warrant.author_account();
    match MembershipRepository::new(store).effective_role(group_id, &author_account)? {
        // The read-only rule belongs to the AUTHOR: the change is theirs, and a
        // relay is not a way round a role that may not write.
        Some((role, _)) if role.is_read_only() => {
            return Err(R::AUTHOR_IS_READ_ONLY.into());
        }
        Some(_) => {}
        None => match &author {
            AuthorRule::MemberOrAdmin { permissions, .. }
                if permissions.is_admin_account(&author_account)? => {}
            AuthorRule::Member | AuthorRule::MemberOrAdmin { .. } => {
                return Err(R::AUTHOR_NOT_A_MEMBER.into());
            }
        },
    }

    // Membership and role first, then the capability. The capability read
    // alone is not enough: at a cut it falls back to the namespace's default
    // mask for an account with no row, so a stranger would inherit whatever
    // the default grants. An admin by genesis passes the admin half.
    if let AuthorRule::MemberOrAdmin {
        permissions,
        capability: Some((capability, refusal)),
    } = author
    {
        if !permissions.is_account_authorized_with_capability(&author_account, capability.bits())? {
            return Err(refusal.into());
        }
    }

    if let Err(refusal) = executor_standing(store, group_id, warrant.executor())? {
        return Err(R::executor(refusal).into());
    }

    let _admitted = next_nonce_state::<R>(store, warrant)?;
    Ok(())
}

/// Record the warrant's nonce as spent, in the ledger its scope names.
///
/// Call only after [`admit`] passed AND the change applied, under the same
/// lock — see [`crate::warrant_gate::check_delegated_delta`].
///
/// # Errors
/// `R::NONCE_SPENT` if the nonce was spent between the check and here (which
/// the shared lock is what prevents), or a store failure.
pub(crate) fn spend_nonce<W: WarrantStatement, R: AdmissionRefusal>(
    store: &Store,
    warrant: &W,
) -> EyreResult<()> {
    let next = next_nonce_state::<R>(store, warrant)?;
    store.handle().put(&ledger_key(warrant), &next)?;
    Ok(())
}

/// Refuse a statement whose nonce may not be accepted, without spending it.
///
/// For the one admission that has no standing to check — a delegated genesis,
/// whose namespace has no rows yet — and still must not be replayable.
///
/// # Errors
/// `R::NONCE_SPENT`, or a store failure.
pub(crate) fn check_nonce<W: WarrantStatement, R: AdmissionRefusal>(
    store: &Store,
    warrant: &W,
) -> EyreResult<()> {
    let _admitted = next_nonce_state::<R>(store, warrant)?;
    Ok(())
}

/// The ledger row a warrant's nonce is spent in: per author device, under the
/// scope the warrant authorizes.
///
/// * a delegated write — the context it writes to;
/// * a delegated creation — the context its seed derives, so a creation
///   warrant and a later write warrant from the same device share one window
///   and can never share a nonce;
/// * a delegated governance op — a scope hashed from its group under
///   [`GOVERNANCE_LEDGER_DOMAIN`], so it never shares a row with a context.
fn ledger_key<W: WarrantStatement>(warrant: &W) -> key::ContextWarrantNonce {
    let scope = match warrant.scope() {
        WarrantScope::Context(context) => context,
        WarrantScope::Creation { seed, .. } => ContextId::from_seed(seed),
        WarrantScope::Governance { group, .. } => {
            ContextId::from(domain_hash(GOVERNANCE_LEDGER_DOMAIN, &[&group]))
        }
    };
    key::ContextWarrantNonce::new(scope, warrant.author_device_key())
}

/// The ledger state that would result from accepting this warrant's nonce, or
/// `R::NONCE_SPENT` if it may not be accepted.
///
/// A window rather than a high-water mark, because gossip gives no ordering
/// between two warrants from one device — see [`types::ContextWarrantNonce`].
fn next_nonce_state<R: AdmissionRefusal>(
    store: &Store,
    warrant: &impl WarrantStatement,
) -> EyreResult<types::ContextWarrantNonce> {
    let nonce = warrant.nonce();
    match store.handle().get(&ledger_key(warrant))? {
        Some(seen) => {
            let seen: types::ContextWarrantNonce = seen;
            Ok(seen.accept(nonce).ok_or(R::NONCE_SPENT)?)
        }
        None => Ok(types::ContextWarrantNonce::first(nonce)),
    }
}

/// Whether `account` may relay a member's change in `group_id`, and on whose
/// authority: `Ok(group)` names the group carrying it, `Err` says why not.
///
/// # Relaying comes from the role for a TEE, from a grant for anyone else
///
/// The executor's EFFECTIVE role decides first — the direct row, or the anchor
/// row a member inherits through, which is the shape a fleet node admitted once
/// at the namespace root has in every subgroup context:
///
/// * **`RelayTee`** relays by its role. Attestation under a namespace policy
///   whose mode is `relay` is the grant, so no `CAN_AUTHOR_ON_BEHALF` bit is
///   needed — a namespace whose default mask omits the bit still has working
///   relays.
/// * **`ReadOnlyTee`** never relays, even holding `CAN_AUTHOR_ON_BEHALF` from a
///   default mask or an explicit grant. It is the TEE replica: new namespaces
///   put the bit in their default mask, so a bit-based rule made every replica
///   admitted after that a relay by accident.
/// * **`ReadOnly`** never relays either: relaying writes a delta, whatever its
///   capability row says.
/// * **`Admin`, `Member`** — self-hosted nodes run with `--delegated-access` —
///   relay by the capability rule ([`capability_grant_source`]).
///
/// A member of no group reaches no role and is refused.
///
/// # A TEE's replica/relay role is the namespace's, read at the root
///
/// For a TEE the role that decides is the one on its **namespace root** row,
/// not the copy a subgroup holds (see [`namespace_tee_role`]). The mode is a
/// namespace policy, and a mode switch converts the root row — the one row the
/// namespace admin can always sign for — but not the copy that
/// `tee_subgroup_admit` carried into a `Restricted` subgroup that admin does not
/// administer. Reading that copy let a TEE the admin had turned back into a
/// replica keep relaying there, and a TEE turned into a relay stay unable to.
///
/// # Peers must agree
///
/// Authorization evaluated **at the cut**, so a node running this and a node
/// running an older rule would disagree about whether the same delegated delta
/// is authorized — and then hold different state. This landed as one
/// coordinated upgrade (`SIGNED_NAMESPACE_OP_SCHEMA_VERSION` 12), not a rolling
/// one; reading a TEE's role at the root is a change of the same kind (13).
pub(crate) fn executor_standing(
    store: &Store,
    group_id: &ContextGroupId,
    account: AccountId,
) -> EyreResult<Result<ContextGroupId, WarrantRefusal>> {
    let Some((role, role_group)) =
        MembershipRepository::new(store).effective_role(group_id, &account)?
    else {
        return Ok(Err(WarrantRefusal::ExecutorMayNotAuthor));
    };
    let (role, role_group) = namespace_tee_role(store, group_id, &account, role, role_group)?;
    Ok(match role {
        GroupMemberRole::RelayTee => Ok(role_group),
        GroupMemberRole::ReadOnlyTee => Err(WarrantRefusal::ExecutorIsTeeReplica),
        GroupMemberRole::ReadOnly => Err(WarrantRefusal::ExecutorIsReadOnly),
        GroupMemberRole::Admin | GroupMemberRole::Member => {
            capability_grant_source(store, group_id, account)?
                .ok_or(WarrantRefusal::ExecutorMayNotAuthor)
        }
    })
}

/// The TEE role that decides whether `account` relays in `group_id`, and the
/// group whose row carries it.
///
/// A non-TEE role, or one already read at the root, is returned unchanged. A
/// TEE role read from a subgroup row is replaced by the role on the account's
/// namespace root row when that row is a TEE role too: the root row is what an
/// admission-mode switch converts, and the subgroup copy may lag behind it
/// indefinitely. Only a TEE role replaces a TEE role, so this never turns a
/// TEE into anything else, nor anything else into a TEE. A TEE with no TEE row
/// at the root — admitted into the subgroup alone — keeps its subgroup row's
/// role, as before.
fn namespace_tee_role(
    store: &Store,
    group_id: &ContextGroupId,
    account: &AccountId,
    role: GroupMemberRole,
    role_group: ContextGroupId,
) -> EyreResult<(GroupMemberRole, ContextGroupId)> {
    if !role.is_tee() {
        return Ok((role, role_group));
    }
    let root = NamespaceRepository::new(store).resolve(group_id)?;
    if root == role_group {
        return Ok((role, role_group));
    }
    Ok(
        match MembershipRepository::new(store).role_of(&root, account)? {
            Some(root_role) if root_role.is_tee() => (root_role, root),
            _ => (role, role_group),
        },
    )
}

/// Which group's capability row carries `account`'s `CAN_AUTHOR_ON_BEHALF`
/// grant, if any — the rule for a non-TEE executor.
///
/// Resolution order: the grant on `group_id` itself if the account is an
/// effective member holding it there, otherwise the row at the ancestor the
/// account inherits its membership through.
///
/// # Why it looks at this group and its anchor, and nothing else
///
/// It mirrors [`MembershipRepository::check_path`] exactly, which is the whole
/// design rule: **a grant reaches wherever membership reaches, and no further.**
/// `check_path` already stops at a non-Open boundary and already returns the
/// closest ancestor holding a direct row, so deferring to it means this cannot
/// report a grant across a privacy boundary the membership walk itself refuses
/// to cross — a subgroup that required its own admission also requires its own
/// grant. Re-deriving the traversal here would be a second implementation of
/// that rule, free to drift from the first.
///
/// An intermediate ancestor between `group` and `anchor` cannot hold a
/// meaningful row: capability rows are written alongside membership rows, and by
/// `check_path`'s definition the anchor is the closest ancestor that has one.
///
/// One conservative edge follows from that deferral. `check_path` short-circuits
/// on an inherited *admin*, returning the first Open ancestor the account
/// administers as the anchor without requiring a member row there — so an admin
/// of a mid-tree group whose authorship grant sits further up resolves to that
/// mid-tree anchor, finds no row, and is refused. Refusing is the safe
/// direction, and `CAN_AUTHOR_ON_BEHALF` is deliberately not implied by admin,
/// so an admin is not a special case that ought to pass regardless. Widening it
/// would mean climbing past the anchor, which is exactly the second
/// implementation of the traversal this defers in order to avoid.
///
/// **An ancestor grant counts, and membership is required.** A namespace-wide
/// grant reaches a subgroup context the account inherits into, and a bare
/// capability row with no membership behind it grants nothing. Both halves
/// read the same way — a group that required its own admission requires its
/// own grant — and the deny-list property is inherited from
/// `effective_capabilities`: a node deny-listed off an Open subgroup is refused
/// there.
fn capability_grant_source(
    store: &Store,
    group_id: &ContextGroupId,
    account: AccountId,
) -> EyreResult<Option<ContextGroupId>> {
    let membership = MembershipRepository::new(store);

    // `effective_capabilities` rather than `check_path` + a raw row read, because
    // it is the deny-list-aware pair of the two. `check_path` deliberately does
    // NOT consult the deny-list, so building on it directly would report a grant
    // for a node kicked from an Open subgroup — where the deny entry *is* the
    // removal, there being no direct row to delete. Reusing the audited read
    // keeps that rule in one place instead of restating it here.
    //
    // `None` means not an effective member of this group by any path, so nothing
    // reachable from here can carry a grant.
    let Some(here) = membership.effective_capabilities(group_id, &account)? else {
        return Ok(None);
    };
    if MemberCapabilities::from_bits_truncate(here)
        .contains(MemberCapabilities::CAN_AUTHOR_ON_BEHALF)
    {
        return Ok(Some(*group_id));
    }

    // Not granted on this group. The anchor is the only other place it can live:
    // membership is inherited from there, and `check_path` has already refused to
    // cross any non-Open boundary on the way. A `Direct` member has no anchor, so
    // its own row above was the whole answer.
    let MembershipPath::Inherited { anchor, .. } = membership.check_path(group_id, &account)?
    else {
        return Ok(None);
    };
    let Some(bits) = CapabilitiesRepository::new(store).member_capability(&anchor, &account)?
    else {
        return Ok(None);
    };
    Ok(MemberCapabilities::from_bits_truncate(bits)
        .contains(MemberCapabilities::CAN_AUTHOR_ON_BEHALF)
        .then_some(anchor))
}

/// Whether `signer` is the executor key the bundle certifies.
///
/// The bundle's executor key is certified for the executor account by
/// `verify`; requiring it to be the op's signer is what makes that certificate
/// about the key that actually published the op. A delegated delta needs no
/// such check here: its envelope verifier already binds the signature to the
/// bundle's key.
#[must_use]
pub(crate) fn signed_by_executor<W>(delegation: &Delegated<W>, signer: &PublicKey) -> bool {
    delegation.executor_key == *signer
}

#[cfg(test)]
mod tests {
    //! The persisted ledger keys are part of every node's on-disk state: a node
    //! upgraded to this code must find the nonces it spent before the upgrade
    //! under exactly the keys it wrote them. So these pin the derivation to the
    //! three gates' original formulas rather than to this module's.

    use super::*;
    use calimero_account::{ContextCreationWarrant, GovernanceOpKind, GovernanceWarrant, Warrant};
    use calimero_primitives::application::ApplicationId;

    // The ledger key is a function of the bytes, not of a real key, so any
    // 32 bytes stand in for a device.
    fn device() -> PublicKey {
        PublicKey::from([0xD1; 32])
    }

    fn account(byte: u8) -> AccountId {
        AccountId::from([byte; 32])
    }

    #[test]
    fn a_write_spends_in_its_context_ledger() {
        let key = device();
        let context = ContextId::from([7; 32]);
        let warrant = Warrant {
            context,
            author_account: account(1),
            author_device_key: key,
            executor: account(2),
            app_version: ApplicationId::from([3; 32]),
            method: "m".to_owned(),
            intent_hash: [0; 32],
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 1,
            not_after: 0,
            signature: [0; 64],
        };
        assert_eq!(
            ledger_key(&warrant),
            key::ContextWarrantNonce::new(context, key)
        );
    }

    #[test]
    fn a_creation_spends_in_the_ledger_of_the_context_its_seed_derives() {
        let key = device();
        let seed = [9; 32];
        let warrant = ContextCreationWarrant {
            group: [4; 32],
            seed,
            author_account: account(1),
            author_device_key: key,
            executor: account(2),
            application_id: ApplicationId::from([3; 32]),
            service_name: None,
            name: None,
            init_hash: [0; 32],
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 1,
            not_after: 0,
            signature: [0; 64],
        };
        assert_eq!(
            ledger_key(&warrant),
            key::ContextWarrantNonce::new(ContextId::from_seed(seed), key)
        );
    }

    #[test]
    fn a_governance_op_spends_in_the_domain_hashed_group_ledger() {
        let key = device();
        let group = [5; 32];
        for kind in [GovernanceOpKind::Group, GovernanceOpKind::Root] {
            let warrant = GovernanceWarrant {
                scope: group,
                kind,
                author_account: account(1),
                author_device_key: key,
                executor: account(2),
                op_hash: [0; 32],
                account_heads: vec![],
                governance_floor: vec![],
                nonce: 1,
                not_after: 0,
                signature: [0; 64],
            };
            // The literal is the delegation gate's original domain, restated
            // rather than referenced so a rename of the constant fails here.
            let scope = ContextId::from(domain_hash(
                b"calimero.governance-warrant.ledger.v1",
                &[&group],
            ));
            assert_eq!(
                ledger_key(&warrant),
                key::ContextWarrantNonce::new(scope, key),
                "{kind:?} ops share the group's one ledger, as before"
            );
        }
    }
}
