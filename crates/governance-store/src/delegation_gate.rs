//! The at-cut half of admitting a governance op a relay published on a member's
//! behalf ([`GroupOp::OnBehalf`], [`RootOp::OnBehalf`]).
//!
//! # Why it is shaped this way
//!
//! The wrapper is signed by the relay; the inner op is the member's. This gate
//! settles everything about the wrapper — the bundle is authentic, it commits
//! to exactly this inner op, it is spent in the group it was signed for, the
//! relay may act for members here, the member is a member who may write — and
//! then the inner op is applied through its ordinary handler **as the member**
//! (an [`ActingPrincipal`]). So the member's authority for the particular op
//! (admin to add an admin, `MANAGE_MEMBERS` to add anyone, the owner or
//! `CAN_DELETE_SUBGROUP` to delete a subgroup, …) is decided by the exact gate a
//! self-signed op meets, at the same cut, and nothing here restates it.
//!
//! **Only delegable ops.** [`GroupOp::delegable_form`] and
//! [`RootOp::delegable_form`] name them. An op outside that set — credentials,
//! key rotation, TEE policy, ownership, upgrades, namespace creation, and every
//! wrapper (so no nesting) — is refused before anything is checked.
//!
//! **Replay** is refused by a per-(group, author device) nonce window, the same
//! window type delegated writes use, kept under a ledger scope derived from the
//! group so it can never share a row with a context's own ledger.
//!
//! **`not_after`** is not checked here, for the reason given in
//! [`crate::warrant_gate`]; the relay checks it at its API.
//!
//! [`GroupOp::OnBehalf`]: calimero_context_client::local_governance::GroupOp::OnBehalf
//! [`RootOp::OnBehalf`]: calimero_context_client::local_governance::RootOp::OnBehalf
//! [`GroupOp::delegable_form`]: calimero_context_client::local_governance::GroupOp::delegable_form
//! [`RootOp::delegable_form`]: calimero_context_client::local_governance::RootOp::delegable_form

use calimero_account::{GovernanceDelegation, GovernanceOpKind, VerifiedGovernanceWarrant};
use calimero_context_client::local_governance::{GroupOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{domain_hash, PublicKey};
use calimero_store::{key, types, Store};
use eyre::Result as EyreResult;

use crate::account_bindings::AccountBindingRepository;
use crate::warrant_gate::{executor_refusal_for_group, WarrantRefusal};
use crate::{ActingPrincipal, MembershipRepository, PermissionChecker};

/// Domain for the ledger scope a group's delegated-governance nonces live
/// under. Hashing the group under its own domain keeps the scope out of the
/// context-id space the same ledger column is otherwise keyed by.
const LEDGER_DOMAIN: &[u8] = b"calimero.governance-warrant.ledger.v1";

/// Why a delegated governance op was refused before its inner op ran.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum DelegationRefusal {
    /// The warrant or one of its certificates does not verify.
    #[error("the governance warrant does not verify: {0}")]
    InvalidDelegation(String),
    /// The inner op is not one a relay may publish on a member's behalf.
    #[error("the {0} op cannot be published on a member's behalf")]
    NotDelegable(&'static str),
    /// The warrant was signed for another group, or another plane.
    #[error(
        "the governance warrant is for a different group or plane than the one it is published on"
    )]
    ScopeMismatch,
    /// The inner op is not the one the warrant commits to.
    #[error("the governance warrant does not cover this op: it commits to a different one")]
    OpMismatch,
    /// The wrapper is signed by a key other than the executor key the bundle names.
    #[error("the op is not signed by the executor the governance warrant authorises")]
    SignerIsNotExecutor,
    /// The author's device has been revoked in this group.
    #[error("the author's device has been revoked in this group")]
    AuthorDeviceRevoked,
    /// The executor's device has been revoked in this group.
    #[error("the executor's device has been revoked in this group")]
    ExecutorDeviceRevoked,
    /// The author is not a member of this group.
    #[error("the author is not a member of this group")]
    AuthorNotAMember,
    /// The author's role in this group is read-only.
    #[error("the author's role in this group is read-only")]
    AuthorIsReadOnly,
    /// The executor may not act for members in this group.
    #[error("the executor may not act for members in this group: {0}")]
    Executor(WarrantRefusal),
    /// The op would grant or withdraw `CAN_AUTHOR_ON_BEHALF` — which relays may
    /// act for members is not something a relay carries a change to.
    #[error("CAN_AUTHOR_ON_BEHALF cannot be granted or withdrawn on a member's behalf")]
    AuthorshipGrantNotDelegable,
    /// A delegated subgroup creation named a group that already exists.
    ///
    /// The creation apply tolerates existing meta (the local handler writes it
    /// before applying its own op) and still seats the creator as an admin —
    /// so a delegated creation naming an existing id would hand the author the
    /// admin seat of someone else's subgroup. A relay's creation never
    /// pre-populates meta, so for it an existing group is only ever a collision.
    #[error("group {0} already exists; pick a fresh random id for the new subgroup")]
    GroupAlreadyExists(String),
    /// This warrant has already been spent.
    #[error("this governance warrant has already been spent")]
    AlreadySpent,
}

/// Admit a delegated group op: check the wrapper, and return the principal the
/// inner op is to be applied as.
///
/// # Errors
/// A [`DelegationRefusal`], the checker's `AuthorityUndecidable` (retried, not
/// a refusal), or a store failure.
pub fn check_group_delegation(
    store: &Store,
    permissions: &PermissionChecker<'_>,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    inner: &GroupOp,
    delegation: &GovernanceDelegation,
) -> EyreResult<(VerifiedGovernanceWarrant, ActingPrincipal)> {
    let form = inner
        .delegable_form()
        .ok_or(DelegationRefusal::NotDelegable(inner.op_kind_label()))?;
    refuse_authorship_grant_change(store, group_id, inner)?;
    let bytes = borsh::to_vec(&form)?;
    check_common(
        store,
        permissions,
        group_id,
        signer,
        GovernanceOpKind::Group,
        &bytes,
        delegation,
    )
}

/// Admit a delegated root op. `namespace_group` is the namespace root the op is
/// published on.
///
/// # Errors
/// As [`check_group_delegation`].
pub fn check_root_delegation(
    store: &Store,
    permissions: &PermissionChecker<'_>,
    namespace_group: &ContextGroupId,
    signer: &PublicKey,
    inner: &RootOp,
    delegation: &GovernanceDelegation,
) -> EyreResult<(VerifiedGovernanceWarrant, ActingPrincipal)> {
    let form = inner
        .delegable_form()
        .ok_or(DelegationRefusal::NotDelegable(root_op_label(inner)))?;
    let bytes = borsh::to_vec(&form)?;
    if matches!(inner, RootOp::NamespaceCreatedV2 { .. }) {
        return check_genesis(store, namespace_group, signer, &bytes, delegation);
    }
    check_common(
        store,
        permissions,
        namespace_group,
        signer,
        GovernanceOpKind::Root,
        &bytes,
        delegation,
    )
}

/// Admit a delegated genesis: the member founds a namespace through a relay.
///
/// Nothing exists yet — no member rows, no bindings, no grants — so the checks
/// that read the namespace's state do not apply, and the ones that remain are
/// the ones that need none: the bundle is authentic, it commits to exactly this
/// genesis, it is spent in the namespace it names, the op is signed by the
/// executor the author named, and the namespace is not already founded. The
/// genesis apply then checks the id derives from the author and the credential
/// is theirs, exactly as for a genesis the founder signs.
fn check_genesis(
    store: &Store,
    namespace_group: &ContextGroupId,
    signer: &PublicKey,
    form_bytes: &[u8],
    delegation: &GovernanceDelegation,
) -> EyreResult<(VerifiedGovernanceWarrant, ActingPrincipal)> {
    let warrant = delegation
        .verify()
        .map_err(|err| DelegationRefusal::InvalidDelegation(err.to_string()))?;
    if warrant.scope != namespace_group.to_bytes() || warrant.kind != GovernanceOpKind::Root {
        return Err(DelegationRefusal::ScopeMismatch.into());
    }
    if delegation.executor_key != *signer {
        return Err(DelegationRefusal::SignerIsNotExecutor.into());
    }
    if !warrant.covers_op(GovernanceOpKind::Root, form_bytes) {
        return Err(DelegationRefusal::OpMismatch.into());
    }
    // An established namespace is never re-founded: the genesis apply would
    // no-op it, but a delegated one is refused outright so a replayed or
    // colliding founding is an error the relay reports, not a silent success.
    if let Some(meta) = crate::MetaRepository::new(store).load(namespace_group)? {
        if meta.admin_identity != crate::placeholder_admin_identity() {
            return Err(DelegationRefusal::GroupAlreadyExists(namespace_group.to_string()).into());
        }
    }
    let _admitted = next_nonce_state(store, namespace_group, &warrant)?;
    let principal = ActingPrincipal {
        key: warrant.author_device_key,
        account: warrant.author_account,
    };
    Ok((warrant, principal))
}

fn check_common(
    store: &Store,
    permissions: &PermissionChecker<'_>,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    kind: GovernanceOpKind,
    form_bytes: &[u8],
    delegation: &GovernanceDelegation,
) -> EyreResult<(VerifiedGovernanceWarrant, ActingPrincipal)> {
    let warrant = delegation
        .verify()
        .map_err(|err| DelegationRefusal::InvalidDelegation(err.to_string()))?;
    if warrant.scope != group_id.to_bytes() || warrant.kind != kind {
        return Err(DelegationRefusal::ScopeMismatch.into());
    }
    if delegation.executor_key != *signer {
        return Err(DelegationRefusal::SignerIsNotExecutor.into());
    }
    if !warrant.covers_op(kind, form_bytes) {
        return Err(DelegationRefusal::OpMismatch.into());
    }

    let bindings = AccountBindingRepository::new(store);
    if bindings.is_revoked(group_id, delegation.author_proof.statement.device)? {
        return Err(DelegationRefusal::AuthorDeviceRevoked.into());
    }
    if bindings.is_revoked(group_id, delegation.executor_proof.statement.device)? {
        return Err(DelegationRefusal::ExecutorDeviceRevoked.into());
    }

    // The author must be someone who may write here at all; what they may do
    // with THIS op is the inner handler's gate. A genesis admin has no member
    // row and is let through by the admin check.
    match MembershipRepository::new(store).effective_role(group_id, &warrant.author_account)? {
        Some((role, _)) if role.is_read_only() => {
            return Err(DelegationRefusal::AuthorIsReadOnly.into());
        }
        Some(_) => {}
        None => {
            if !permissions.is_admin_account(&warrant.author_account)? {
                return Err(DelegationRefusal::AuthorNotAMember.into());
            }
        }
    }

    if let Some(refusal) = executor_refusal_for_group(store, group_id, warrant.executor)? {
        return Err(DelegationRefusal::Executor(refusal).into());
    }

    let _admitted = next_nonce_state(store, group_id, &warrant)?;
    let principal = ActingPrincipal {
        key: warrant.author_device_key,
        account: warrant.author_account,
    };
    Ok((warrant, principal))
}

/// Refuse a capability op that would change `CAN_AUTHOR_ON_BEHALF`.
///
/// That bit decides which relays may write for members — the same concern as
/// the TEE policies, which are not delegable at all. A relay carrying a change
/// to it would be deciding its own trust (or a rival's), so a delegated
/// capability op must leave it exactly as it is: neither grant it where it is
/// absent nor drop it where it is held.
fn refuse_authorship_grant_change(
    store: &Store,
    group_id: &ContextGroupId,
    op: &GroupOp,
) -> EyreResult<()> {
    let bit = calimero_context_config::MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits();
    let caps = crate::CapabilitiesRepository::new(store);
    let (current, next) = match op {
        GroupOp::MemberCapabilitySet {
            member,
            capabilities,
        } => (
            caps.member_capability(group_id, member)?.unwrap_or(0),
            capabilities.bits(),
        ),
        GroupOp::DefaultCapabilitiesSet { capabilities } => (
            caps.default_capabilities(group_id)?.unwrap_or(0),
            capabilities.bits(),
        ),
        _ => return Ok(()),
    };
    if (current ^ next) & bit != 0 {
        return Err(DelegationRefusal::AuthorshipGrantNotDelegable.into());
    }
    Ok(())
}

/// Record the warrant's nonce as spent. Call only after the inner op applied,
/// under the same group lock.
///
/// # Errors
/// [`DelegationRefusal::AlreadySpent`], or a store failure.
pub fn spend_delegation_nonce(
    store: &Store,
    group_id: &ContextGroupId,
    warrant: &VerifiedGovernanceWarrant,
) -> EyreResult<()> {
    let next = next_nonce_state(store, group_id, warrant)?;
    store.handle().put(&ledger_key(group_id, warrant), &next)?;
    Ok(())
}

fn ledger_key(
    group_id: &ContextGroupId,
    warrant: &VerifiedGovernanceWarrant,
) -> key::ContextWarrantNonce {
    let scope = ContextId::from(domain_hash(LEDGER_DOMAIN, &[&group_id.to_bytes()]));
    key::ContextWarrantNonce::new(scope, warrant.author_device_key)
}

fn next_nonce_state(
    store: &Store,
    group_id: &ContextGroupId,
    warrant: &VerifiedGovernanceWarrant,
) -> EyreResult<types::ContextWarrantNonce> {
    match store.handle().get(&ledger_key(group_id, warrant))? {
        Some(seen) => {
            let seen: types::ContextWarrantNonce = seen;
            Ok(seen
                .accept(warrant.nonce)
                .ok_or(DelegationRefusal::AlreadySpent)?)
        }
        None => Ok(types::ContextWarrantNonce::first(warrant.nonce)),
    }
}

/// A stable label for a root op, for the refusal message.
fn root_op_label(op: &RootOp) -> &'static str {
    match op {
        RootOp::GroupCreated { .. } => "GroupCreated",
        RootOp::GroupReparented { .. } => "GroupReparented",
        RootOp::GroupDeleted { .. } => "GroupDeleted",
        RootOp::AdminChanged { .. } => "AdminChanged",
        RootOp::PolicyUpdated { .. } => "PolicyUpdated",
        RootOp::MemberJoined { .. } => "MemberJoined",
        RootOp::MemberJoinedAt { .. } => "MemberJoinedAt",
        RootOp::MemberJoinedOpen { .. } => "MemberJoinedOpen",
        RootOp::MemberJoinedViaTeeAttestation { .. } => "MemberJoinedViaTeeAttestation",
        RootOp::KeyDelivery { .. } => "KeyDelivery",
        RootOp::NamespaceCreatedV2 { .. } => "NamespaceCreated",
        RootOp::OnBehalf { .. } => "OnBehalf",
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::{
        AccountId, GovernanceDelegation, GovernanceOpKind, GovernanceTerms, GovernanceWarrant,
    };
    use calimero_context_client::local_governance::{
        GroupOp, RootOp, SignedGroupOp, SignedNamespaceOp,
    };
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::MemberCapabilities;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::Store;

    use super::DelegationRefusal;
    use crate::namespace::NamespaceGovernance;
    use crate::test_fixtures::{
        account_for, enrol_member, real_join_account, sample_meta_with_admin, seal_for_test,
        test_store,
    };
    use crate::warrant_gate::WarrantRefusal;
    use crate::{
        apply_local_signed_group_op, AccountBindingRepository, CapabilitiesRepository,
        MembershipRepository, MetaRepository, MetadataRepository, NamespaceRepository,
    };

    const NS: [u8; 32] = [0xA0; 32];
    const GENESIS_ADMIN: [u8; 32] = [0xEE; 32];

    struct World {
        store: Store,
        ns: ContextGroupId,
        author_sk: PrivateKey,
        author: AccountId,
        relay_sk: PrivateKey,
        relay: AccountId,
        other: AccountId,
        nonce: std::cell::Cell<u64>,
    }

    /// A namespace in which the author is a `Member` with `author_caps`, the
    /// relay a `Member` holding only `CAN_AUTHOR_ON_BEHALF`, and `other` a plain
    /// member the author may act upon.
    fn world(author_caps: MemberCapabilities) -> World {
        let store = test_store();
        let ns = ContextGroupId::from(NS);
        MetaRepository::new(&store)
            .save(&ns, &sample_meta_with_admin(AccountId::from(GENESIS_ADMIN)))
            .expect("meta");
        let author_sk = PrivateKey::from([0x0A; 32]);
        let relay_sk = PrivateKey::from([0x0B; 32]);
        let other_pk = PrivateKey::from([0x0C; 32]).public_key();
        let author = enrol_member(&store, &ns, &author_sk.public_key());
        let relay = enrol_member(&store, &ns, &relay_sk.public_key());
        let other = enrol_member(&store, &ns, &other_pk);
        let membership = MembershipRepository::new(&store);
        for (who, role) in [
            (author, GroupMemberRole::Member),
            (relay, GroupMemberRole::Member),
            (other, GroupMemberRole::Member),
        ] {
            membership.add_member(&ns, &who, role).expect("member");
        }
        let caps = CapabilitiesRepository::new(&store);
        caps.set_member_capability(&ns, &author, author_caps.bits())
            .expect("author caps");
        caps.set_member_capability(&ns, &relay, MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits())
            .expect("relay caps");
        World {
            store,
            ns,
            author_sk,
            author,
            relay_sk,
            relay,
            other,
            nonce: std::cell::Cell::new(0),
        }
    }

    impl World {
        fn next_nonce(&self) -> u64 {
            self.nonce.set(self.nonce.get() + 1);
            self.nonce.get()
        }

        fn bundle(
            &self,
            scope: ContextGroupId,
            kind: GovernanceOpKind,
            form: &[u8],
        ) -> GovernanceDelegation {
            self.bundle_with(scope, kind, form, self.next_nonce())
        }

        fn bundle_with(
            &self,
            scope: ContextGroupId,
            kind: GovernanceOpKind,
            form: &[u8],
            nonce: u64,
        ) -> GovernanceDelegation {
            GovernanceDelegation {
                warrant: Box::new(
                    GovernanceWarrant::sign(
                        &self.author_sk,
                        GovernanceTerms {
                            scope: scope.to_bytes(),
                            kind,
                            author_account: self.author,
                            executor: self.relay,
                            op_hash: GovernanceWarrant::op_hash(kind, form),
                            account_heads: vec![],
                            governance_floor: vec![],
                            nonce,
                            not_after: u64::MAX,
                        },
                    )
                    .expect("sign"),
                ),
                author_proof: real_join_account(&self.author_sk.public_key()),
                executor_proof: real_join_account(&self.relay_sk.public_key()),
                executor_key: self.relay_sk.public_key(),
            }
        }

        /// A group op wrapped for `group`, with a warrant for its delegable form.
        fn on_behalf(&self, group: ContextGroupId, inner: GroupOp) -> GroupOp {
            let form = borsh::to_vec(&inner.delegable_form().expect("delegable")).expect("encode");
            GroupOp::OnBehalf {
                delegation: Box::new(self.bundle(group, GovernanceOpKind::Group, &form)),
                op: Box::new(inner),
            }
        }

        /// Publish a group op signed by `signer`, the way a peer receives it.
        fn publish(
            &self,
            signer: &PrivateKey,
            group: ContextGroupId,
            op: GroupOp,
        ) -> eyre::Result<()> {
            let signed = SignedGroupOp::sign(
                signer,
                group.to_bytes().into(),
                vec![],
                self.next_nonce(),
                op,
            )?;
            apply_local_signed_group_op(&self.store, &signed).map(|_| ())
        }

        fn relay_publishes(&self, group: ContextGroupId, op: GroupOp) -> eyre::Result<()> {
            self.publish(&self.relay_sk, group, op)
        }

        /// Publish a sealed root op on the namespace, signed by the relay.
        fn relay_publishes_root(&self, op: RootOp) -> eyre::Result<()> {
            let signed = SignedNamespaceOp::sign(
                &self.relay_sk,
                NS.into(),
                vec![],
                self.next_nonce(),
                seal_for_test(&self.store, self.ns, op),
            )?;
            NamespaceGovernance::new(&self.store, NS.into())
                .apply_signed_op(&signed)
                .map(|_| ())
        }

        fn root_on_behalf(&self, inner: RootOp) -> RootOp {
            let form = borsh::to_vec(&inner.delegable_form().expect("delegable")).expect("encode");
            RootOp::OnBehalf {
                delegation: Box::new(self.bundle(self.ns, GovernanceOpKind::Root, &form)),
                op: Box::new(inner),
            }
        }

        fn is_member(&self, group: &ContextGroupId, who: &AccountId) -> bool {
            MembershipRepository::new(&self.store)
                .is_member(group, who)
                .expect("read")
        }

        fn stranger(&self) -> AccountId {
            account_for(&PrivateKey::from([0x5A; 32]).public_key())
        }
    }

    fn refusal(result: eyre::Result<()>) -> DelegationRefusal {
        let err = result.expect_err("must be refused");
        err.chain()
            .find_map(|cause| cause.downcast_ref::<DelegationRefusal>())
            .cloned()
            .unwrap_or_else(|| panic!("not a DelegationRefusal: {err:?}"))
    }

    fn add(member: AccountId) -> GroupOp {
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Member,
        }
    }

    // ── group ops: the author's own authority decides ─────────────────────

    #[test]
    fn a_member_with_manage_members_adds_someone_through_a_relay_that_has_no_such_right() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let newcomer = w.stranger();
        w.relay_publishes(w.ns, w.on_behalf(w.ns, add(newcomer)))
            .expect("the author may add members");
        assert!(w.is_member(&w.ns, &newcomer));

        // Pin the point: the relay could not have done this as itself.
        let second = account_for(&PrivateKey::from([0x5B; 32]).public_key());
        let _refused = w
            .relay_publishes(w.ns, add(second))
            .expect_err("the relay holds no MANAGE_MEMBERS of its own");
        assert!(!w.is_member(&w.ns, &second));
    }

    #[test]
    fn an_author_without_the_right_is_refused_by_the_inner_ops_own_gate() {
        let w = world(MemberCapabilities::empty());
        let newcomer = w.stranger();
        let err = w
            .relay_publishes(w.ns, w.on_behalf(w.ns, add(newcomer)))
            .expect_err("the author may not add members");
        assert!(
            err.chain()
                .any(|c| c.downcast_ref::<crate::CapabilitiesError>().is_some()),
            "refused by the ordinary MANAGE_MEMBERS gate: {err:?}"
        );
        assert!(!w.is_member(&w.ns, &newcomer), "nothing written");
    }

    /// `MANAGE_MEMBERS` does not reach admin-making: the same rule a
    /// self-signed op meets, applied to the author.
    #[test]
    fn a_non_admin_author_cannot_make_an_admin_through_a_relay() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let newcomer = w.stranger();
        let _refused = w
            .relay_publishes(
                w.ns,
                w.on_behalf(
                    w.ns,
                    GroupOp::MemberAdded {
                        member: newcomer,
                        role: GroupMemberRole::Admin,
                    },
                ),
            )
            .expect_err("only an admin adds an admin");
        assert!(!w.is_member(&w.ns, &newcomer));
    }

    #[test]
    fn an_admin_author_changes_roles_and_capabilities() {
        let w = world(MemberCapabilities::empty());
        MembershipRepository::new(&w.store)
            .add_member(&w.ns, &w.author, GroupMemberRole::Admin)
            .expect("promote");
        w.relay_publishes(
            w.ns,
            w.on_behalf(
                w.ns,
                GroupOp::MemberCapabilitySet {
                    member: w.other,
                    capabilities: MemberCapabilities::CAN_CREATE_CONTEXT,
                },
            ),
        )
        .expect("an admin sets capabilities");
        let caps = CapabilitiesRepository::new(&w.store)
            .member_capability(&w.ns, &w.other)
            .expect("read")
            .unwrap_or(0);
        assert_eq!(caps, MemberCapabilities::CAN_CREATE_CONTEXT.bits());

        w.relay_publishes(
            w.ns,
            w.on_behalf(
                w.ns,
                GroupOp::MemberRoleSet {
                    member: w.other,
                    role: GroupMemberRole::ReadOnly,
                },
            ),
        )
        .expect("an admin sets roles");
        assert_eq!(
            MembershipRepository::new(&w.store)
                .role_of(&w.ns, &w.other)
                .expect("read"),
            Some(GroupMemberRole::ReadOnly)
        );
    }

    #[test]
    fn metadata_is_attributed_to_the_author_not_the_relay() {
        let w = world(MemberCapabilities::CAN_MANAGE_METADATA);
        w.relay_publishes(
            w.ns,
            w.on_behalf(
                w.ns,
                GroupOp::GroupMetadataSet {
                    name: Some("general".to_owned()),
                    data: Default::default(),
                },
            ),
        )
        .expect("the author may manage metadata");
        let record = MetadataRepository::new(&w.store)
            .group_metadata(&w.ns)
            .expect("read")
            .expect("recorded");
        assert_eq!(record.name.as_deref(), Some("general"));
    }

    /// A removal's post-state hashes are the relay's to compute: the warrant
    /// signs the op with them cleared, and the filled-in op still matches.
    #[test]
    fn a_removal_is_signed_without_the_hashes_the_relay_computes() {
        let w = world(MemberCapabilities::empty());
        MembershipRepository::new(&w.store)
            .add_member(&w.ns, &w.author, GroupMemberRole::Admin)
            .expect("promote");
        let signed_form = GroupOp::MemberRemoved {
            member: w.other,
            expected_group_state_hash: [0; 32],
            expected_context_state_hashes: vec![],
        };
        let form = borsh::to_vec(&signed_form).expect("encode");
        let filled = GroupOp::MemberRemoved {
            member: w.other,
            expected_group_state_hash: [0x42; 32],
            expected_context_state_hashes: vec![],
        };
        w.relay_publishes(
            w.ns,
            GroupOp::OnBehalf {
                delegation: Box::new(w.bundle(w.ns, GovernanceOpKind::Group, &form)),
                op: Box::new(filled),
            },
        )
        .expect("the removal applies");
        assert!(!w.is_member(&w.ns, &w.other));
    }

    #[test]
    fn an_author_leaves_through_a_relay_but_cannot_leave_someone_else() {
        let w = world(MemberCapabilities::empty());
        let leave = |member| GroupOp::MemberLeft {
            member,
            expected_group_state_hash: [0; 32],
            expected_context_state_hashes: vec![],
        };
        let err = w
            .relay_publishes(w.ns, w.on_behalf(w.ns, leave(w.other)))
            .expect_err("a leave is self-only");
        assert!(
            err.chain().any(|c| matches!(
                c.downcast_ref::<crate::MembershipError>(),
                Some(crate::MembershipError::SelfLeaveOnly)
            )),
            "{err:?}"
        );
        assert!(w.is_member(&w.ns, &w.other));

        w.relay_publishes(w.ns, w.on_behalf(w.ns, leave(w.author)))
            .expect("the author leaves");
        assert!(!w.is_member(&w.ns, &w.author));
    }

    // ── what the wrapper must match ───────────────────────────────────────

    #[test]
    fn only_delegable_ops_are_carried() {
        let w = world(MemberCapabilities::from_bits_truncate(u32::MAX));
        for inner in [
            GroupOp::TransferOwnership {
                new_owner: w.author,
            },
            GroupOp::GroupKeyRotated { departed: w.other },
            GroupOp::TeeAuthoringPolicySet {
                allowed_mrtd: vec![],
            },
            GroupOp::GroupMigrationSet { migration: None },
            GroupOp::Noop,
        ] {
            let bytes = borsh::to_vec(&inner).expect("encode");
            let op = GroupOp::OnBehalf {
                delegation: Box::new(w.bundle(w.ns, GovernanceOpKind::Group, &bytes)),
                op: Box::new(inner),
            };
            assert!(matches!(
                refusal(w.relay_publishes(w.ns, op)),
                DelegationRefusal::NotDelegable(_)
            ));
        }
    }

    #[test]
    fn a_wrapper_cannot_nest_another() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let inner = w.on_behalf(w.ns, add(w.stranger()));
        let bytes = borsh::to_vec(&inner).expect("encode");
        let nested = GroupOp::OnBehalf {
            delegation: Box::new(w.bundle(w.ns, GovernanceOpKind::Group, &bytes)),
            op: Box::new(inner),
        };
        assert!(matches!(
            refusal(w.relay_publishes(w.ns, nested)),
            DelegationRefusal::NotDelegable(_)
        ));
    }

    /// The relay swaps the member being added after the author signed.
    #[test]
    fn an_op_other_than_the_signed_one_is_refused() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let signed = borsh::to_vec(&add(w.stranger())).expect("encode");
        let swapped = account_for(&PrivateKey::from([0x5C; 32]).public_key());
        let op = GroupOp::OnBehalf {
            delegation: Box::new(w.bundle(w.ns, GovernanceOpKind::Group, &signed)),
            op: Box::new(add(swapped)),
        };
        assert_eq!(
            refusal(w.relay_publishes(w.ns, op)),
            DelegationRefusal::OpMismatch
        );
        assert!(!w.is_member(&w.ns, &swapped));
    }

    #[test]
    fn a_warrant_for_another_group_or_plane_is_refused() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let inner = add(w.stranger());
        let bytes = borsh::to_vec(&inner).expect("encode");
        for (scope, kind) in [
            (ContextGroupId::from([0x99; 32]), GovernanceOpKind::Group),
            (w.ns, GovernanceOpKind::Root),
        ] {
            let op = GroupOp::OnBehalf {
                delegation: Box::new(w.bundle(scope, kind, &bytes)),
                op: Box::new(inner.clone()),
            };
            assert_eq!(
                refusal(w.relay_publishes(w.ns, op)),
                DelegationRefusal::ScopeMismatch
            );
        }
    }

    #[test]
    fn the_wrapper_must_be_published_by_the_named_executor_key() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let op = w.on_behalf(w.ns, add(w.stranger()));
        assert_eq!(
            refusal(w.publish(&w.author_sk, w.ns, op)),
            DelegationRefusal::SignerIsNotExecutor
        );
    }

    #[test]
    fn a_tampered_warrant_is_refused() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let mut op = w.on_behalf(w.ns, add(w.stranger()));
        if let GroupOp::OnBehalf { delegation, .. } = &mut op {
            delegation.warrant.not_after -= 1;
        }
        assert!(matches!(
            refusal(w.relay_publishes(w.ns, op)),
            DelegationRefusal::InvalidDelegation(_)
        ));
    }

    // ── who may carry it, and who may ask ─────────────────────────────────

    #[test]
    fn a_relay_without_standing_is_refused_and_a_relay_tee_is_not() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        CapabilitiesRepository::new(&w.store)
            .set_member_capability(&w.ns, &w.relay, 0)
            .expect("drop the grant");
        assert_eq!(
            refusal(w.relay_publishes(w.ns, w.on_behalf(w.ns, add(w.stranger())))),
            DelegationRefusal::Executor(WarrantRefusal::ExecutorMayNotAuthor)
        );

        MembershipRepository::new(&w.store)
            .set_role(&w.ns, &w.relay, GroupMemberRole::RelayTee)
            .expect("relay TEE");
        w.relay_publishes(w.ns, w.on_behalf(w.ns, add(w.stranger())))
            .expect("a RelayTee carries a member's op by its role");

        MembershipRepository::new(&w.store)
            .set_role(&w.ns, &w.relay, GroupMemberRole::ReadOnlyTee)
            .expect("replica");
        let second = account_for(&PrivateKey::from([0x5D; 32]).public_key());
        assert_eq!(
            refusal(w.relay_publishes(w.ns, w.on_behalf(w.ns, add(second)))),
            DelegationRefusal::Executor(WarrantRefusal::ExecutorIsTeeReplica)
        );
    }

    #[test]
    fn a_read_only_or_removed_author_is_refused() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        MembershipRepository::new(&w.store)
            .set_role(&w.ns, &w.author, GroupMemberRole::ReadOnly)
            .expect("demote");
        assert_eq!(
            refusal(w.relay_publishes(w.ns, w.on_behalf(w.ns, add(w.stranger())))),
            DelegationRefusal::AuthorIsReadOnly
        );
        MembershipRepository::new(&w.store)
            .remove_member(&w.ns, &w.author)
            .expect("remove");
        assert_eq!(
            refusal(w.relay_publishes(w.ns, w.on_behalf(w.ns, add(w.stranger())))),
            DelegationRefusal::AuthorNotAMember
        );
    }

    #[test]
    fn a_revoked_author_device_is_refused() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let device = real_join_account(&w.author_sk.public_key())
            .statement
            .device;
        AccountBindingRepository::new(&w.store)
            .apply_revocation(&w.ns, device)
            .expect("revoke");
        assert_eq!(
            refusal(w.relay_publishes(w.ns, w.on_behalf(w.ns, add(w.stranger())))),
            DelegationRefusal::AuthorDeviceRevoked
        );
    }

    /// Adding someone who was later removed must not be undone by replaying
    /// the warrant: the nonce is spent.
    #[test]
    fn a_replayed_warrant_is_refused() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let newcomer = w.stranger();
        let op = w.on_behalf(w.ns, add(newcomer));
        w.relay_publishes(w.ns, op.clone()).expect("first");
        MembershipRepository::new(&w.store)
            .remove_member(&w.ns, &newcomer)
            .expect("removed since");
        assert_eq!(
            refusal(w.relay_publishes(w.ns, op)),
            DelegationRefusal::AlreadySpent
        );
        assert!(!w.is_member(&w.ns, &newcomer));
    }

    // ── root ops: subgroups ───────────────────────────────────────────────

    fn create_subgroup(_w: &World, group: [u8; 32], admin: AccountId) -> RootOp {
        RootOp::GroupCreated {
            group_id: group.into(),
            parent_id: NS.into(),
            restricted: true,
            admin,
        }
    }

    #[test]
    fn a_member_with_create_subgroup_creates_one_through_a_relay_and_owns_it() {
        let w = world(MemberCapabilities::CAN_CREATE_SUBGROUP);
        NamespaceRepository::new(&w.store)
            .store_identity(&w.ns, &w.relay_sk.public_key(), w.relay_sk.as_bytes())
            .expect("identity");
        let channel = [0xB1; 32];
        w.relay_publishes_root(w.root_on_behalf(create_subgroup(&w, channel, w.author)))
            .expect("the author may create a subgroup");
        let gid = ContextGroupId::from(channel);
        let meta = MetaRepository::new(&w.store)
            .load(&gid)
            .expect("read")
            .expect("created");
        assert_eq!(meta.owner_identity, w.author, "the AUTHOR owns it");
        assert!(MembershipRepository::new(&w.store)
            .is_admin(&gid, &w.author)
            .expect("read"));
        assert!(
            !MembershipRepository::new(&w.store)
                .is_admin(&gid, &w.relay)
                .expect("read"),
            "the relay gains nothing"
        );
    }

    #[test]
    fn a_subgroup_naming_another_admin_is_refused() {
        let w = world(MemberCapabilities::CAN_CREATE_SUBGROUP);
        let channel = [0xB2; 32];
        let _refused = w
            .relay_publishes_root(w.root_on_behalf(create_subgroup(&w, channel, w.relay)))
            .expect_err("the declared admin must be the author");
        assert!(MetaRepository::new(&w.store)
            .load(&ContextGroupId::from(channel))
            .expect("read")
            .is_none());
    }

    #[test]
    fn an_author_without_create_subgroup_is_refused() {
        let w = world(MemberCapabilities::empty());
        let channel = [0xB3; 32];
        let _refused = w
            .relay_publishes_root(w.root_on_behalf(create_subgroup(&w, channel, w.author)))
            .expect_err("the author may not create subgroups");
        assert!(MetaRepository::new(&w.store)
            .load(&ContextGroupId::from(channel))
            .expect("read")
            .is_none());
    }

    /// The whole DM flow, delegated: create a restricted subgroup, then add the
    /// other person to it — both published by the relay, both authorized as the
    /// author.
    #[test]
    fn a_dm_is_created_and_populated_entirely_through_a_relay() {
        let w = world(MemberCapabilities::CAN_CREATE_SUBGROUP);
        let dm = [0xD1; 32];
        let gid = ContextGroupId::from(dm);
        w.relay_publishes_root(w.root_on_behalf(create_subgroup(&w, dm, w.author)))
            .expect("create the DM subgroup");
        // No "set capabilities" for the relay in between: creating the subgroup
        // through it seated it there.
        w.relay_publishes(gid, w.on_behalf(gid, add(w.other)))
            .expect("the DM's admin adds the other member");
        assert!(w.is_member(&gid, &w.other));
    }

    #[test]
    fn root_ops_outside_the_delegable_set_are_refused() {
        let w = world(MemberCapabilities::from_bits_truncate(u32::MAX));
        for inner in [
            RootOp::AdminChanged {
                new_admin: w.author,
            },
            RootOp::PolicyUpdated {
                policy_bytes: vec![],
            },
        ] {
            let bytes = borsh::to_vec(&inner).expect("encode");
            let op = RootOp::OnBehalf {
                delegation: Box::new(w.bundle(w.ns, GovernanceOpKind::Root, &bytes)),
                op: Box::new(inner),
            };
            assert!(matches!(
                refusal(w.relay_publishes_root(op)),
                DelegationRefusal::NotDelegable(_)
            ));
        }
    }

    /// The projection folds a delegated op as the op it carries, attributed to
    /// the AUTHOR — or later at-cut checks would not see the member it added,
    /// and would credit the relay with the change.
    #[test]
    fn the_projection_folds_the_inner_op_as_the_authors() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let newcomer = w.stranger();
        let wrapped = w.on_behalf(w.ns, add(newcomer));
        // The envelope's ciphertext is irrelevant here: the projection is
        // handed the decrypted op, as the apply path hands it.
        let signed = SignedNamespaceOp::sign(
            &w.relay_sk,
            NS.into(),
            vec![],
            1,
            calimero_context_client::local_governance::NamespaceOp::Group {
                group_id: w.ns.to_bytes().into(),
                key_id: [0u8; 32].into(),
                encrypted: calimero_governance_types::EncryptedGroupOp {
                    nonce: [0u8; 12],
                    ciphertext: Vec::new(),
                },
                key_rotation: None,
            },
        )
        .expect("sign");
        let op = crate::unified_op_decode::op_from_namespace_op(
            &signed,
            Some(&wrapped),
            [0x01; 32],
            calimero_storage::logical_clock::HybridTimestamp::default(),
            &[],
        );
        assert_eq!(
            op.author(),
            w.author,
            "attributed to the member, not the relay"
        );
        assert_ne!(op.author(), w.relay);
        assert!(
            matches!(
                op.payload,
                calimero_op::OpPayload::MemberAdded { member, .. } if member == newcomer
            ),
            "folds as the inner MemberAdded: {:?}",
            op.payload
        );
    }

    /// A wrapper around an op the apply refuses to carry folds as nothing, so
    /// no fold can seat a root admin through one, whatever it wraps.
    #[test]
    fn a_wrapped_op_that_may_not_be_delegated_folds_as_nothing() {
        use calimero_context_client::local_governance::NamespaceOp;

        let w = world(MemberCapabilities::from_bits_truncate(u32::MAX));
        let subgroup = ContextGroupId::from([0xA7; 32]);
        let wrap_group = |inner: GroupOp| {
            let bytes = borsh::to_vec(&inner).expect("encode");
            GroupOp::OnBehalf {
                delegation: Box::new(w.bundle(subgroup, GovernanceOpKind::Group, &bytes)),
                op: Box::new(inner),
            }
        };
        let transfer = wrap_group(GroupOp::TransferOwnership {
            new_owner: w.author,
        });
        for group in [subgroup, w.ns] {
            let envelope = SignedNamespaceOp::sign(
                &w.relay_sk,
                NS.into(),
                vec![],
                1,
                NamespaceOp::Group {
                    group_id: group.to_bytes().into(),
                    key_id: [0u8; 32].into(),
                    encrypted: calimero_governance_types::EncryptedGroupOp {
                        nonce: [0u8; 12],
                        ciphertext: Vec::new(),
                    },
                    key_rotation: None,
                },
            )
            .expect("sign");
            let op = crate::unified_op_decode::op_from_namespace_op(
                &envelope,
                Some(&transfer),
                [0x01; 32],
                calimero_storage::logical_clock::HybridTimestamp::default(),
                &[],
            );
            assert_eq!(
                op.payload,
                calimero_op::OpPayload::Noop,
                "a wrapped TransferOwnership in {group:?}"
            );
        }

        let admin_change = RootOp::AdminChanged {
            new_admin: w.author,
        };
        let bytes = borsh::to_vec(&admin_change).expect("encode");
        let root = SignedNamespaceOp::sign(
            &w.relay_sk,
            NS.into(),
            vec![],
            2,
            NamespaceOp::Root(RootOp::OnBehalf {
                delegation: Box::new(w.bundle(w.ns, GovernanceOpKind::Root, &bytes)),
                op: Box::new(admin_change),
            }),
        )
        .expect("sign");
        let op = crate::unified_op_decode::op_from_namespace_op(
            &root,
            None,
            [0x02; 32],
            calimero_storage::logical_clock::HybridTimestamp::default(),
            &[],
        );
        assert_eq!(
            op.payload,
            calimero_op::OpPayload::Noop,
            "a wrapped AdminChanged"
        );
    }

    /// An at-cut authorizer shaped like the real projection for a thin client:
    /// it cannot resolve the author's device key (bound in no group), so every
    /// KEY-typed question answers "no", while ACCOUNT-typed ones answer from the
    /// rows. A delegated op must therefore be decided by account.
    struct KeyBlindAuthorizer<'a>(&'a Store);

    impl crate::authorizer::AtCutAuthorizer for KeyBlindAuthorizer<'_> {
        fn is_admin_at_cut(
            &self,
            _: &ContextGroupId,
            _: &PublicKey,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            Some(false)
        }
        fn is_admin_or_capability_at_cut(
            &self,
            _: &ContextGroupId,
            _: &PublicKey,
            _: u32,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            Some(false)
        }
        fn is_admin_or_capability_account_at_cut(
            &self,
            group: &ContextGroupId,
            member: &AccountId,
            capability: u32,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            MembershipRepository::new(self.0)
                .is_admin_or_has_capability(group, member, capability)
                .ok()
        }
        fn is_admin_account_at_cut(
            &self,
            group: &ContextGroupId,
            member: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            MembershipRepository::new(self.0)
                .is_admin(group, member)
                .ok()
        }
        fn is_last_admin_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            Some(false)
        }
        fn membership_path_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<crate::authorizer::AtCutMembershipPath> {
            None
        }
    }

    /// At a real cut, the author's authority is read by ACCOUNT. A gate that
    /// asked about the author's key would get "no" from a projection that has
    /// never seen that key, and refuse an op every live check admits.
    #[test]
    fn at_a_cut_the_authors_authority_is_read_by_account() {
        let w = world(MemberCapabilities::MANAGE_MEMBERS);
        let newcomer = w.stranger();
        let op = w.on_behalf(w.ns, add(newcomer));
        let authorizer = KeyBlindAuthorizer(&w.store);
        let cut: &[[u8; 32]] = &[[0x01; 32]];
        let relay_pk = w.relay_sk.public_key();
        let mut ctx = crate::ops::group::GroupApplyCtx::new_with_apply_auth(
            &w.store,
            &w.ns,
            &relay_pk,
            cut,
            &authorizer,
        );
        let handled = crate::ops::group::dispatch(&mut ctx, &op).expect("admitted at the cut");
        assert!(handled);
        assert!(w.is_member(&w.ns, &newcomer));
    }

    // ── the relay that creates a subgroup can serve it ────────────────────

    #[test]
    fn a_relay_that_creates_a_subgroup_is_seated_in_it_with_authorship() {
        let w = world(MemberCapabilities::CAN_CREATE_SUBGROUP);
        let channel = [0xC1; 32];
        let gid = ContextGroupId::from(channel);
        w.relay_publishes_root(w.root_on_behalf(create_subgroup(&w, channel, w.author)))
            .expect("create");
        assert_eq!(
            MembershipRepository::new(&w.store)
                .role_of(&gid, &w.relay)
                .expect("read"),
            Some(GroupMemberRole::Member),
            "seated as a plain member, never an admin"
        );
        let caps = CapabilitiesRepository::new(&w.store)
            .member_capability(&gid, &w.relay)
            .expect("read")
            .unwrap_or(0);
        assert_eq!(
            caps,
            MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
            "with the standing to act for members, and nothing else"
        );
        assert!(
            crate::warrant_gate::executor_refusal_for_group(&w.store, &gid, w.relay)
                .expect("read")
                .is_none(),
            "so it may serve the subgroup's contexts immediately"
        );
    }

    /// The projection folds the relay's seat along with the creation it rode
    /// on, so the relay the rows seat is a member at every cut too. Folding the
    /// creation alone left it a stranger to every read the projection gates.
    #[test]
    fn the_projection_seats_the_relay_that_created_a_subgroup() {
        let w = world(MemberCapabilities::CAN_CREATE_SUBGROUP);
        let channel = [0xC3; 32];
        let gid = ContextGroupId::from(channel);
        let root = w.root_on_behalf(create_subgroup(&w, channel, w.author));
        w.relay_publishes_root(root.clone()).expect("create");

        let envelope = SignedNamespaceOp::sign(
            &w.relay_sk,
            NS.into(),
            vec![],
            1,
            calimero_context_client::local_governance::NamespaceOp::Root(root.clone()),
        )
        .expect("sign");
        let op = crate::unified_op_decode::op_from_namespace_op_with_binding(
            &envelope,
            None,
            Some(&root),
            None,
            [0x01; 32],
            calimero_storage::logical_clock::HybridTimestamp::default(),
            &[],
        );
        assert_eq!(op.author(), w.author, "still attributed to the member");
        let view = calimero_projection::ScopeState::from_ops([&op]).acl_view();
        assert_eq!(
            view.groups
                .get(&gid)
                .and_then(|members| members.get(&w.relay)),
            MembershipRepository::new(&w.store)
                .role_of(&gid, &w.relay)
                .expect("read")
                .as_ref(),
            "the fold seats the relay exactly as the rows do"
        );
        assert_eq!(
            view.member_caps.get(&(gid, w.relay)).copied(),
            CapabilitiesRepository::new(&w.store)
                .member_capability(&gid, &w.relay)
                .expect("read"),
        );
        assert_eq!(
            view.group_admin.get(&gid),
            Some(&w.author),
            "and the creation itself still folds"
        );
    }

    /// A TEE relay gets its subgroup role from attestation admission, never a
    /// plain row written here.
    #[test]
    fn a_tee_relay_is_left_to_attestation_admission() {
        let w = world(MemberCapabilities::CAN_CREATE_SUBGROUP);
        MembershipRepository::new(&w.store)
            .set_role(&w.ns, &w.relay, GroupMemberRole::RelayTee)
            .expect("relay TEE");
        let channel = [0xC2; 32];
        let gid = ContextGroupId::from(channel);
        w.relay_publishes_root(w.root_on_behalf(create_subgroup(&w, channel, w.author)))
            .expect("create");
        assert_eq!(
            MembershipRepository::new(&w.store)
                .role_of(&gid, &w.relay)
                .expect("read"),
            None,
            "no Member row minted for a TEE"
        );
    }

    // ── CAN_AUTHOR_ON_BEHALF stays off the relay path ─────────────────────

    #[test]
    fn a_relay_cannot_carry_a_grant_or_withdrawal_of_authorship() {
        let w = world(MemberCapabilities::empty());
        MembershipRepository::new(&w.store)
            .add_member(&w.ns, &w.author, GroupMemberRole::Admin)
            .expect("promote");
        let set = |capabilities| GroupOp::MemberCapabilitySet {
            member: w.other,
            capabilities,
        };

        assert_eq!(
            refusal(w.relay_publishes(
                w.ns,
                w.on_behalf(w.ns, set(MemberCapabilities::CAN_AUTHOR_ON_BEHALF))
            )),
            DelegationRefusal::AuthorshipGrantNotDelegable,
            "granting it"
        );

        CapabilitiesRepository::new(&w.store)
            .set_member_capability(
                &w.ns,
                &w.other,
                MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
            )
            .expect("held already");
        assert_eq!(
            refusal(w.relay_publishes(
                w.ns,
                w.on_behalf(w.ns, set(MemberCapabilities::CAN_CREATE_CONTEXT))
            )),
            DelegationRefusal::AuthorshipGrantNotDelegable,
            "withdrawing it by omission"
        );

        // Other bits change freely while this one is left as it is.
        w.relay_publishes(
            w.ns,
            w.on_behalf(
                w.ns,
                set(MemberCapabilities::CAN_AUTHOR_ON_BEHALF
                    | MemberCapabilities::CAN_CREATE_CONTEXT),
            ),
        )
        .expect("the grant untouched, another bit added");
    }

    #[test]
    fn a_relay_cannot_put_authorship_into_the_default_mask() {
        let w = world(MemberCapabilities::empty());
        MembershipRepository::new(&w.store)
            .add_member(&w.ns, &w.author, GroupMemberRole::Admin)
            .expect("promote");
        assert_eq!(
            refusal(w.relay_publishes(
                w.ns,
                w.on_behalf(
                    w.ns,
                    GroupOp::DefaultCapabilitiesSet {
                        capabilities: MemberCapabilities::CAN_AUTHOR_ON_BEHALF
                    }
                )
            )),
            DelegationRefusal::AuthorshipGrantNotDelegable
        );
        w.relay_publishes(
            w.ns,
            w.on_behalf(
                w.ns,
                GroupOp::DefaultCapabilitiesSet {
                    capabilities: MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
                },
            ),
        )
        .expect("a default mask without it is fine");
    }

    // ── joining an Open subgroup yourself ─────────────────────────────────

    fn open_channel(w: &World, id: [u8; 32]) -> ContextGroupId {
        let gid = ContextGroupId::from(id);
        MetaRepository::new(&w.store)
            .save(
                &gid,
                &sample_meta_with_admin(AccountId::from(GENESIS_ADMIN)),
            )
            .expect("meta");
        crate::test_fixtures::nest_for_test(&w.store, &w.ns, &gid);
        CapabilitiesRepository::new(&w.store)
            .set_subgroup_visibility(&gid, calimero_context_config::VisibilityMode::Open)
            .expect("open");
        gid
    }

    #[test]
    fn a_member_joins_an_open_channel_through_a_relay() {
        let w = world(MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS);
        let channel = open_channel(&w, [0xE1; 32]);
        w.relay_publishes_root(w.root_on_behalf(RootOp::MemberJoinedOpen {
            member: w.author,
            group_id: channel,
            account: real_join_account(&w.author_sk.public_key()),
        }))
        .expect("the author joins the open channel");
    }

    /// The join is self-only: a relay holding the author's warrant cannot use it
    /// to join somebody else.
    #[test]
    fn a_relay_cannot_join_someone_else_to_an_open_channel() {
        let w = world(MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS);
        let channel = open_channel(&w, [0xE2; 32]);
        let other_pk = PrivateKey::from([0x0C; 32]).public_key();
        let err = w
            .relay_publishes_root(w.root_on_behalf(RootOp::MemberJoinedOpen {
                member: w.other,
                group_id: channel,
                account: real_join_account(&other_pk),
            }))
            .expect_err("only the author may be joined");
        assert!(
            err.chain().any(|c| matches!(
                c.downcast_ref::<crate::ApplyError>(),
                Some(crate::ApplyError::MemberJoinedOpenRejected(_))
            )),
            "{err:?}"
        );
    }

    /// A creation naming an existing subgroup must not seat the author as its
    /// admin: the relay path never pre-populates meta, so an existing id is a
    /// collision, refused before anything is written.
    #[test]
    fn a_creation_naming_an_existing_subgroup_is_refused() {
        let w = world(MemberCapabilities::CAN_CREATE_SUBGROUP);
        let theirs = open_channel(&w, [0xF1; 32]);
        let err = w
            .relay_publishes_root(w.root_on_behalf(create_subgroup(
                &w,
                theirs.to_bytes(),
                w.author,
            )))
            .expect_err("collision");
        assert!(
            matches!(
                err.chain()
                    .find_map(|c| c.downcast_ref::<DelegationRefusal>()),
                Some(DelegationRefusal::GroupAlreadyExists(_))
            ),
            "{err:?}"
        );
        assert!(
            !MembershipRepository::new(&w.store)
                .is_admin(&theirs, &w.author)
                .expect("read"),
            "the author gains no seat in somebody else's subgroup"
        );
    }
}

/// A namespace founded through a relay: the author is its founder, owner and
/// admin; the relay is seated to serve it and may admit itself, once, as its
/// first TEE, which sets the default relay-mode policy.
#[cfg(test)]
mod founding_tests {
    use calimero_account::{
        AccountId, GovernanceDelegation, GovernanceOpKind, GovernanceTerms, GovernanceWarrant,
    };
    use calimero_context_client::local_governance::{
        GroupOp, NamespaceOp, RootOp, SignedGroupOp, SignedNamespaceOp, TeeAdmissionMode,
    };
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::MemberCapabilities;

    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::Store;

    use super::DelegationRefusal;
    use crate::namespace::NamespaceGovernance;
    use crate::tee::{read_tee_admission_policy, TeeAdmissionPolicyRead};
    use crate::test_fixtures::{account_for, real_join_account, test_store};
    use crate::{
        apply_local_signed_group_op, CapabilitiesRepository, MembershipRepository, MetaRepository,
        NamespaceFoundingRepository,
    };

    const SALT: [u8; 32] = [0x5C; 32];

    struct Founding {
        store: Store,
        ns: ContextGroupId,
        author_sk: PrivateKey,
        author: AccountId,
        relay_sk: PrivateKey,
        relay: AccountId,
        seq: std::cell::Cell<u64>,
    }

    fn founding() -> Founding {
        let author_sk = PrivateKey::from([0x1A; 32]);
        let relay_sk = PrivateKey::from([0x2B; 32]);
        let author = account_for(&author_sk.public_key());
        let relay = account_for(&relay_sk.public_key());
        let ns = ContextGroupId::from(calimero_account::founded_namespace_id(&author, &SALT));
        Founding {
            store: test_store(),
            ns,
            author_sk,
            author,
            relay_sk,
            relay,
            seq: std::cell::Cell::new(0),
        }
    }

    impl Founding {
        fn genesis(&self) -> RootOp {
            RootOp::NamespaceCreatedV2 {
                founder: self.author,
                account: real_join_account(&self.author_sk.public_key()),
                salt: SALT,
            }
        }

        fn wrapped(&self, inner: RootOp, nonce: u64) -> RootOp {
            let form = borsh::to_vec(&inner).expect("encode");
            RootOp::OnBehalf {
                delegation: Box::new(GovernanceDelegation {
                    warrant: Box::new(
                        GovernanceWarrant::sign(
                            &self.author_sk,
                            GovernanceTerms {
                                scope: self.ns.to_bytes(),
                                kind: GovernanceOpKind::Root,
                                author_account: self.author,
                                executor: self.relay,
                                op_hash: GovernanceWarrant::op_hash(GovernanceOpKind::Root, &form),
                                account_heads: vec![],
                                governance_floor: vec![],
                                nonce,
                                not_after: u64::MAX,
                            },
                        )
                        .expect("sign"),
                    ),
                    author_proof: real_join_account(&self.author_sk.public_key()),
                    executor_proof: real_join_account(&self.relay_sk.public_key()),
                    executor_key: self.relay_sk.public_key(),
                }),
                op: Box::new(inner),
            }
        }

        /// Publish a cleartext root op, parentless, as a genesis is.
        fn publish_root(&self, signer: &PrivateKey, op: RootOp) -> eyre::Result<()> {
            let signed = SignedNamespaceOp::sign(
                signer,
                self.ns.to_bytes().into(),
                vec![],
                1,
                NamespaceOp::Root(op),
            )?;
            NamespaceGovernance::new(&self.store, self.ns.to_bytes().into())
                .apply_signed_op(&signed)
                .map(|_| ())
        }

        fn found(&self) -> eyre::Result<()> {
            self.publish_root(&self.relay_sk, self.wrapped(self.genesis(), 1))
        }

        fn attestation(&self, sk: &PrivateKey, mock: bool) -> GroupOp {
            GroupOp::FoundingRelayAttested {
                account: real_join_account(&sk.public_key()),
                quote: crate::tee::tests::mock_quote_for(&sk.public_key()),
                collateral: None,
                attested_at: 1_700_000_000,
                release_version: "3.1.0".to_owned(),
                profile: "locked-read-only".to_owned(),
                mock,
            }
        }

        /// Apply a group op on the namespace root and append it to the root's
        /// log, as the live pipeline does.
        fn publish_group(&self, signer: &PrivateKey, op: GroupOp) -> eyre::Result<()> {
            self.seq.set(self.seq.get() + 1);
            let signed = SignedGroupOp::sign(signer, self.ns, vec![], self.seq.get(), op)?;
            apply_local_signed_group_op(&self.store, &signed)?;
            crate::local_state::append_op_log_entry(
                &self.store,
                &self.ns,
                self.seq.get(),
                &borsh::to_vec(&signed)?,
            )?;
            Ok(())
        }

        fn role(&self, who: &AccountId) -> Option<GroupMemberRole> {
            MembershipRepository::new(&self.store)
                .role_of(&self.ns, who)
                .expect("read")
        }
    }

    fn refusal(result: eyre::Result<()>) -> Option<DelegationRefusal> {
        result
            .expect_err("refused")
            .chain()
            .find_map(|c| c.downcast_ref::<DelegationRefusal>())
            .cloned()
    }

    #[test]
    fn a_member_founds_a_namespace_through_a_relay_and_owns_it() {
        let f = founding();
        f.found().expect("founded");
        let meta = MetaRepository::new(&f.store)
            .load(&f.ns)
            .expect("read")
            .expect("established");
        assert_eq!(meta.owner_identity, f.author, "the author owns it");
        assert_eq!(meta.admin_identity, f.author, "and administers it");
        assert_eq!(f.role(&f.author), Some(GroupMemberRole::Admin));

        assert_eq!(
            f.role(&f.relay),
            Some(GroupMemberRole::Member),
            "the relay is seated, never as an admin"
        );
        assert_eq!(
            CapabilitiesRepository::new(&f.store)
                .member_capability(&f.ns, &f.relay)
                .expect("read"),
            Some(MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits()),
        );
        assert!(
            crate::warrant_gate::executor_refusal_for_group(&f.store, &f.ns, f.relay)
                .expect("read")
                .is_none(),
            "so it can serve the new namespace at once"
        );
        assert_eq!(
            NamespaceFoundingRepository::new(&f.store)
                .founding_relay(&f.ns)
                .expect("read"),
            Some((f.relay, false))
        );
    }

    #[test]
    fn the_founding_relay_attests_and_becomes_the_first_relay_tee() {
        let f = founding();
        f.found().expect("founded");
        assert!(matches!(
            read_tee_admission_policy(&f.store, &f.ns).expect("read"),
            TeeAdmissionPolicyRead::NotSet
        ));

        f.publish_group(&f.relay_sk, f.attestation(&f.relay_sk, true))
            .expect("the founding relay attests");
        assert_eq!(f.role(&f.relay), Some(GroupMemberRole::RelayTee));

        let TeeAdmissionPolicyRead::Set(policy) =
            read_tee_admission_policy(&f.store, &f.ns).expect("read")
        else {
            panic!("the attestation is the namespace's first policy");
        };
        assert_eq!(
            policy.mode,
            TeeAdmissionMode::Relay,
            "relay mode by default"
        );
        assert_eq!(policy.allowed_tcb_statuses, vec!["UpToDate".to_owned()]);
        assert!(
            policy.accept_mock,
            "a mock build's policy admits its mock quotes"
        );
        let trust = policy.release_trust.expect("a signed-release policy");
        assert_eq!(trust.allowed_profiles, vec!["locked-read-only".to_owned()]);

        // And the relay is now a verifier: it can admit further fleet TEEs.
        assert!(crate::MembershipPolicy::new(&f.store, f.ns)
            .is_tee_attestation_verifier(&f.relay)
            .expect("read"));
    }

    #[test]
    fn the_founding_relay_attests_once() {
        let f = founding();
        f.found().expect("founded");
        f.publish_group(&f.relay_sk, f.attestation(&f.relay_sk, true))
            .expect("first");
        let err = f
            .publish_group(&f.relay_sk, f.attestation(&f.relay_sk, true))
            .expect_err("a second attestation");
        assert!(err.to_string().contains("already attested"), "{err}");
    }

    /// A member other than the founding relay cannot use the founding path to
    /// make itself a TEE, whatever its quote.
    #[test]
    fn only_the_founding_relay_may_attest() {
        let f = founding();
        f.found().expect("founded");
        let other_sk = PrivateKey::from([0x3C; 32]);
        let err = f
            .publish_group(&other_sk, f.attestation(&other_sk, true))
            .expect_err("not the founding relay");
        assert!(err.to_string().contains("founding relay"), "{err}");
        assert_eq!(f.role(&f.relay), Some(GroupMemberRole::Member));
    }

    /// The relay cannot present somebody else's quote as its own: the quote is
    /// verified against the key that signed the op.
    #[test]
    fn a_quote_for_another_key_is_refused() {
        let f = founding();
        f.found().expect("founded");
        let mut op = f.attestation(&f.relay_sk, true);
        if let GroupOp::FoundingRelayAttested { quote, .. } = &mut op {
            *quote = crate::tee::tests::mock_quote_for(&PrivateKey::from([0x4D; 32]).public_key());
        }
        let _refused = f.publish_group(&f.relay_sk, op).expect_err("wrong key");
        assert_eq!(f.role(&f.relay), Some(GroupMemberRole::Member));
    }

    #[test]
    fn a_mock_quote_claimed_as_real_is_refused() {
        let f = founding();
        f.found().expect("founded");
        let err = f
            .publish_group(&f.relay_sk, f.attestation(&f.relay_sk, false))
            .expect_err("claims a real quote");
        assert!(err.to_string().contains("mock"), "{err}");
    }

    /// A namespace founded by its own node has no founding relay, so nobody can
    /// take the self-admission path there.
    #[test]
    fn a_namespace_founded_by_a_node_has_no_founding_relay() {
        let f = founding();
        f.publish_root(&f.author_sk, f.genesis())
            .expect("self-founded");
        let err = f
            .publish_group(&f.author_sk, f.attestation(&f.author_sk, true))
            .expect_err("no founding relay");
        assert!(
            err.to_string().contains("not founded through a relay"),
            "{err}"
        );
    }

    #[test]
    fn a_founding_cannot_be_replayed_or_repeated() {
        let f = founding();
        f.found().expect("founded");
        assert!(matches!(
            refusal(f.publish_root(&f.relay_sk, f.wrapped(f.genesis(), 2))),
            Some(DelegationRefusal::GroupAlreadyExists(_))
        ));
    }

    /// The genesis the author signs names the author: a relay cannot found a
    /// namespace in the author's name for somebody else.
    #[test]
    fn a_genesis_naming_someone_else_is_refused() {
        let f = founding();
        let other = account_for(&PrivateKey::from([0x3C; 32]).public_key());
        let inner = RootOp::NamespaceCreatedV2 {
            founder: other,
            account: real_join_account(&f.author_sk.public_key()),
            salt: SALT,
        };
        let _refused = f
            .publish_root(&f.relay_sk, f.wrapped(inner, 1))
            .expect_err("the founder must be the author");
        assert!(MetaRepository::new(&f.store)
            .load(&f.ns)
            .expect("read")
            .is_none());
    }

    #[test]
    fn a_genesis_must_be_published_by_the_named_relay() {
        let f = founding();
        assert!(matches!(
            refusal(f.publish_root(&f.author_sk, f.wrapped(f.genesis(), 1))),
            Some(DelegationRefusal::SignerIsNotExecutor)
        ));
    }

    /// Sealing follows the inner op: a delegated genesis travels in the clear,
    /// as every genesis must, and other delegated root ops stay sealed.
    #[test]
    fn a_delegated_genesis_is_not_sealed() {
        let f = founding();
        assert!(!calimero_governance_types::root_op_is_sealable(
            &f.wrapped(f.genesis(), 1)
        ));
        assert!(calimero_governance_types::root_op_is_sealable(&f.wrapped(
            RootOp::GroupReparented {
                child_group_id: [1; 32].into(),
                new_parent_id: [2; 32].into(),
            },
            1
        )));
    }

    /// The founding relay's self-admission folds as a `RelayTee` membership on
    /// its own; wrapped, which the apply refuses, it must fold as nothing.
    #[test]
    fn a_wrapped_founding_attestation_folds_as_nothing() {
        let f = founding();
        let RootOp::OnBehalf { delegation, .. } = f.wrapped(f.genesis(), 1) else {
            unreachable!("wrapped() builds a wrapper");
        };
        let fold = |op: &GroupOp| {
            let envelope = SignedNamespaceOp::sign(
                &f.relay_sk,
                f.ns.to_bytes().into(),
                vec![],
                1,
                NamespaceOp::Group {
                    group_id: f.ns.to_bytes().into(),
                    key_id: [0u8; 32].into(),
                    encrypted: calimero_governance_types::EncryptedGroupOp {
                        nonce: [0u8; 12],
                        ciphertext: Vec::new(),
                    },
                    key_rotation: None,
                },
            )
            .expect("sign");
            crate::unified_op_decode::op_from_namespace_op(
                &envelope,
                Some(op),
                [0x01; 32],
                calimero_storage::logical_clock::HybridTimestamp::default(),
                &[],
            )
            .payload
        };
        let bare = f.attestation(&f.relay_sk, true);
        assert!(
            matches!(
                fold(&bare),
                calimero_op::OpPayload::MemberJoinedWithDevice {
                    role: GroupMemberRole::RelayTee,
                    ..
                }
            ),
            "control: the bare attestation seats a RelayTee"
        );
        let wrapped = GroupOp::OnBehalf {
            op: Box::new(bare),
            delegation,
        };
        assert_eq!(fold(&wrapped), calimero_op::OpPayload::Noop);
    }
}
