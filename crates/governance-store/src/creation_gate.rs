//! The at-cut half of admitting a delegated context registration.
//!
//! # Why it is shaped this way
//!
//! A delegated registration ([`GroupOp::ContextRegisteredOnBehalf`]) is signed by
//! a relay and authorized by a member. The member's signed
//! [`ContextCreationWarrant`] is what decides it, so this gate asks every
//! question of the AUTHOR the self-signed registration asks of its signer — is
//! it a member, may it write, does it hold `CAN_CREATE_CONTEXT` — and asks the
//! relay only whether it may act for members here at all, by the same rule a
//! delegated write uses ([`crate::warrant_gate`]).
//!
//! **One function, run by the author's relay and by every peer.** The relay
//! runs it before it executes anything (so a refusal never costs an `init` run)
//! and again, like every peer, when the op applies. The two runs differ only in
//! the cut the capability is read at, which is the same split every other
//! governance gate has.
//!
//! **What the op claims is re-derived, never trusted.** The context id is
//! derived from the warrant's seed, and the application, service and name are
//! compared with the warrant's. The relay chooses nothing about the context it
//! registers; a relay that tries is refused by every replica.
//!
//! **Replay is refused by the same ledger delegated writes use.** A warrant pins
//! a seed and so exactly one context id, and the nonce is spent in that
//! context's per-device window when the op applies. Replaying the warrant after
//! an admin has detached the context would otherwise re-register it; the spent
//! nonce is what stops that.
//!
//! **`not_after` is not checked here**, for the reason set out in
//! [`crate::warrant_gate`]: wall-clock expiry must not gate an apply, or peers
//! applying at different times stop converging. The relay checks it at its API.
//!
//! [`GroupOp::ContextRegisteredOnBehalf`]: calimero_context_client::local_governance::GroupOp::ContextRegisteredOnBehalf
//! [`ContextCreationWarrant`]: calimero_account::ContextCreationWarrant

use calimero_account::{ContextCreationDelegation, VerifiedCreationWarrant};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::{key, types, Store};
use eyre::Result as EyreResult;

use crate::account_bindings::AccountBindingRepository;
use crate::warrant_gate::{executor_refusal_for_group, WarrantRefusal};
use crate::{MembershipRepository, PermissionChecker};

/// Why a delegated context registration was refused.
///
/// Typed for the same reason [`WarrantRefusal`] is: the cases send an operator
/// to different places — a malformed bundle is a client bug, a missing
/// capability is an admin who has not granted it, and a mismatch between the op
/// and its warrant is a relay rewriting what the member signed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum CreationRefusal {
    /// The warrant or one of its certificates does not verify.
    #[error("the creation warrant does not verify: {0}")]
    InvalidDelegation(String),
    /// The warrant authorises creation in a different group.
    #[error("the creation warrant is for a different group than the one it is registered in")]
    GroupMismatch,
    /// The op is signed by a key other than the executor key the bundle names.
    #[error("the registration is not signed by the executor the creation warrant authorises")]
    SignerIsNotExecutor,
    /// The op names a context other than the one the warrant's seed derives.
    #[error("the registration names a context the creation warrant's seed does not derive")]
    ContextIdMismatch,
    /// The op registers a different application than the warrant pins.
    #[error("the registration runs a different application than the creation warrant pins")]
    ApplicationMismatch,
    /// The op names a different service than the warrant pins.
    #[error("the registration names a different service than the creation warrant pins")]
    ServiceMismatch,
    /// The op records a different name than the warrant pins.
    #[error("the registration records a different name than the creation warrant pins")]
    NameMismatch,
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
    /// The author is neither an admin nor a holder of `CAN_CREATE_CONTEXT`.
    #[error("the author may not create contexts in this group (not an admin and CAN_CREATE_CONTEXT is not set)")]
    AuthorMayNotCreate,
    /// The executor may not act for members in this group.
    #[error("the executor may not act for members in this group: {0}")]
    Executor(WarrantRefusal),
    /// This warrant has already registered its context.
    #[error("this creation warrant has already been spent")]
    AlreadySpent,
}

/// What a registration op claims about the context it registers — the fields
/// the gate compares against the warrant.
#[derive(Clone, Copy, Debug)]
pub struct ClaimedRegistration<'a> {
    /// The context the op registers.
    pub context_id: &'a ContextId,
    /// The application it runs.
    pub application_id: &'a ApplicationId,
    /// Its service, for a multi-service bundle.
    pub service_name: &'a Option<String>,
    /// Its display name.
    pub name: &'a Option<String>,
}

/// Check everything about a delegated registration that needs no store: the
/// bundle verifies, and the op claims exactly what the warrant pins, signed by
/// the executor the warrant authorises.
///
/// # Errors
/// The [`CreationRefusal`] naming the first mismatch.
pub fn check_warrant_terms(
    group_id: &ContextGroupId,
    signer: &PublicKey,
    claim: ClaimedRegistration<'_>,
    delegation: &ContextCreationDelegation,
) -> Result<VerifiedCreationWarrant, CreationRefusal> {
    let warrant = delegation
        .verify()
        .map_err(|err| CreationRefusal::InvalidDelegation(err.to_string()))?;

    if warrant.group != group_id.to_bytes() {
        return Err(CreationRefusal::GroupMismatch);
    }
    // The bundle's executor key is certified for the executor account by
    // `verify`; requiring it to be the op's signer is what makes that
    // certificate about the key that actually published this.
    if delegation.executor_key != *signer {
        return Err(CreationRefusal::SignerIsNotExecutor);
    }
    if ContextId::from_seed(warrant.seed) != *claim.context_id {
        return Err(CreationRefusal::ContextIdMismatch);
    }
    if warrant.application_id != *claim.application_id {
        return Err(CreationRefusal::ApplicationMismatch);
    }
    if warrant.service_name != *claim.service_name {
        return Err(CreationRefusal::ServiceMismatch);
    }
    if warrant.name != *claim.name {
        return Err(CreationRefusal::NameMismatch);
    }
    Ok(warrant)
}

/// Admit a delegated registration at the cut `permissions` carries.
///
/// **Read-only**, like [`crate::warrant_gate::check_delegated_delta`]: the
/// nonce is spent by [`spend_creation_nonce`] after the registration applied.
///
/// # Errors
/// A [`CreationRefusal`] for a registration that must not apply; the
/// permission checker's own `AuthorityUndecidable` when the cut cannot be
/// resolved here yet (retried, never a refusal); or a store failure.
pub fn check_delegated_creation(
    store: &Store,
    permissions: &PermissionChecker<'_>,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    claim: ClaimedRegistration<'_>,
    delegation: &ContextCreationDelegation,
) -> EyreResult<VerifiedCreationWarrant> {
    let warrant = check_warrant_terms(group_id, signer, claim, delegation)?;

    let bindings = AccountBindingRepository::new(store);
    if bindings.is_revoked(group_id, delegation.author_proof.statement.device)? {
        return Err(CreationRefusal::AuthorDeviceRevoked.into());
    }
    if bindings.is_revoked(group_id, delegation.executor_proof.statement.device)? {
        return Err(CreationRefusal::ExecutorDeviceRevoked.into());
    }

    // Membership and role first, then the capability. The capability read
    // alone is not enough: at a cut it falls back to the namespace's default
    // mask for an account with no row, so a stranger would inherit whatever
    // the default grants. An admin by genesis has no member row either, and is
    // let through by the admin half of the capability gate below.
    let role =
        MembershipRepository::new(store).effective_role(group_id, &warrant.author_account)?;
    match role {
        Some((role, _)) if role.is_read_only() => {
            return Err(CreationRefusal::AuthorIsReadOnly.into());
        }
        Some(_) => {}
        None => {
            if !permissions.is_admin_account(&warrant.author_account)? {
                return Err(CreationRefusal::AuthorNotAMember.into());
            }
        }
    }
    if !permissions.is_account_authorized_with_capability(
        &warrant.author_account,
        MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
    )? {
        return Err(CreationRefusal::AuthorMayNotCreate.into());
    }

    if let Some(refusal) = executor_refusal_for_group(store, group_id, warrant.executor)? {
        return Err(CreationRefusal::Executor(refusal).into());
    }

    let _admitted = next_nonce_state(store, claim.context_id, &warrant)?;
    Ok(warrant)
}

/// Record the creation warrant's nonce as spent in the new context's ledger.
///
/// Call only after [`check_delegated_creation`] passed and the registration
/// applied, under the same group lock.
///
/// # Errors
/// [`CreationRefusal::AlreadySpent`] if it was spent in between, or a store
/// failure.
pub fn spend_creation_nonce(
    store: &Store,
    context_id: &ContextId,
    warrant: &VerifiedCreationWarrant,
) -> EyreResult<()> {
    let next = next_nonce_state(store, context_id, warrant)?;
    let key = key::ContextWarrantNonce::new(*context_id, warrant.author_device_key);
    store.handle().put(&key, &next)?;
    Ok(())
}

/// The ledger state after accepting this warrant's nonce, or
/// [`CreationRefusal::AlreadySpent`].
///
/// The context's own warrant ledger, keyed by the author device: the same
/// window delegated writes into that context spend from, so a creation warrant
/// and a later method warrant from the same device can never share a nonce.
fn next_nonce_state(
    store: &Store,
    context_id: &ContextId,
    warrant: &VerifiedCreationWarrant,
) -> EyreResult<types::ContextWarrantNonce> {
    let key = key::ContextWarrantNonce::new(*context_id, warrant.author_device_key);
    match store.handle().get(&key)? {
        Some(seen) => {
            let seen: types::ContextWarrantNonce = seen;
            Ok(seen
                .accept(warrant.nonce)
                .ok_or(CreationRefusal::AlreadySpent)?)
        }
        None => Ok(types::ContextWarrantNonce::first(warrant.nonce)),
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::{
        AccountId, ContextCreationDelegation, ContextCreationTerms, ContextCreationWarrant,
    };
    use calimero_context_client::local_governance::GroupOp;
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::blobs::BlobId;
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::Store;

    use super::{
        check_delegated_creation, check_warrant_terms, ClaimedRegistration, CreationRefusal,
    };
    use crate::authorizer::LIVE_FALLBACK_AUTHORIZER;
    use crate::ops::group::{dispatch, GroupApplyCtx};
    use crate::test_fixtures::{
        enrol_member, nest_for_test, real_join_account, sample_meta_with_admin, test_store,
        FixedAuthorizer, UnresolvableAuthorizer,
    };
    use crate::warrant_gate::WarrantRefusal;
    use crate::{
        get_context_service_name, get_group_for_context, AccountBindingRepository,
        CapabilitiesRepository, MembershipRepository, MetaRepository, MetadataRepository,
        PermissionChecker,
    };

    const GROUP: [u8; 32] = [0xC0; 32];
    const SEED: [u8; 32] = [0xC5; 32];
    const APP: [u8; 32] = [0xCC; 32];
    const AUTHOR_SK: [u8; 32] = [0x0A; 32];
    const RELAY_SK: [u8; 32] = [0x0B; 32];
    const GENESIS_ADMIN: [u8; 32] = [0xEE; 32];
    const INIT: &[u8] = br#"{"name":"general"}"#;
    /// A non-empty cut, so an at-cut authorizer is actually consulted: real
    /// authorizers abstain on an empty cut by contract.
    const CUT: &[[u8; 32]] = &[[0x01; 32]];

    struct World {
        store: Store,
        group: ContextGroupId,
        author: AccountId,
        author_sk: PrivateKey,
        relay: AccountId,
        relay_pk: PublicKey,
    }

    /// A group in which a delegated creation is supposed to be admitted: the
    /// author is a `Member` holding `CAN_CREATE_CONTEXT`, the relay a `Member`
    /// holding `CAN_AUTHOR_ON_BEHALF` and NOT `CAN_CREATE_CONTEXT` — the relay
    /// must never need the author's authority.
    fn world() -> World {
        let store = test_store();
        let group = ContextGroupId::from(GROUP);
        MetaRepository::new(&store)
            .save(
                &group,
                &sample_meta_with_admin(AccountId::from(GENESIS_ADMIN)),
            )
            .expect("save meta");

        let author_sk = PrivateKey::from(AUTHOR_SK);
        let relay_sk = PrivateKey::from(RELAY_SK);
        let author = enrol_member(&store, &group, &author_sk.public_key());
        let relay = enrol_member(&store, &group, &relay_sk.public_key());

        let membership = MembershipRepository::new(&store);
        membership
            .add_member(&group, &author, GroupMemberRole::Member)
            .expect("add author");
        membership
            .add_member(&group, &relay, GroupMemberRole::Member)
            .expect("add relay");
        let caps = CapabilitiesRepository::new(&store);
        caps.set_member_capability(
            &group,
            &author,
            MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
        )
        .expect("author may create");
        caps.set_member_capability(
            &group,
            &relay,
            MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
        )
        .expect("relay may act for members");

        World {
            store,
            group,
            author,
            author_sk,
            relay,
            relay_pk: relay_sk.public_key(),
        }
    }

    fn terms(w: &World) -> ContextCreationTerms {
        ContextCreationTerms {
            group: w.group.to_bytes(),
            seed: SEED,
            author_account: w.author,
            executor: w.relay,
            application_id: ApplicationId::from(APP),
            service_name: Some("chat".to_owned()),
            name: Some("general".to_owned()),
            init_hash: ContextCreationWarrant::init_hash(INIT),
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 1,
            not_after: u64::MAX,
        }
    }

    fn bundle_with(w: &World, terms: ContextCreationTerms) -> ContextCreationDelegation {
        let author_pk = w.author_sk.public_key();
        ContextCreationDelegation {
            warrant: Box::new(ContextCreationWarrant::sign(&w.author_sk, terms).expect("sign")),
            author_proof: real_join_account(&author_pk),
            executor_proof: real_join_account(&w.relay_pk),
            executor_key: w.relay_pk,
        }
    }

    fn bundle(w: &World) -> ContextCreationDelegation {
        bundle_with(w, terms(w))
    }

    fn context() -> ContextId {
        ContextId::from_seed(SEED)
    }

    fn check(w: &World, delegation: &ContextCreationDelegation) -> eyre::Result<()> {
        check_on(w, delegation, &context())
    }

    fn check_on(
        w: &World,
        delegation: &ContextCreationDelegation,
        context_id: &ContextId,
    ) -> eyre::Result<()> {
        check_delegated_creation(
            &w.store,
            &PermissionChecker::new(&w.store, w.group),
            &w.group,
            &w.relay_pk,
            ClaimedRegistration {
                context_id,
                application_id: &ApplicationId::from(APP),
                service_name: &Some("chat".to_owned()),
                name: &Some("general".to_owned()),
            },
            delegation,
        )
        .map(|_| ())
    }

    fn refusal(result: eyre::Result<()>) -> CreationRefusal {
        let err = result.expect_err("must be refused");
        err.downcast_ref::<CreationRefusal>()
            .cloned()
            .unwrap_or_else(|| panic!("not a CreationRefusal: {err:?}"))
    }

    fn op(delegation: ContextCreationDelegation) -> GroupOp {
        GroupOp::ContextRegisteredOnBehalf {
            context_id: context(),
            application_id: ApplicationId::from(APP),
            blob_id: BlobId::from([0xBB; 32]),
            source: String::new(),
            service_name: Some("chat".to_owned()),
            package: String::new(),
            version: String::new(),
            name: Some("general".to_owned()),
            delegation: Box::new(delegation),
        }
    }

    /// Apply `op` as the relay would publish it, through the real dispatcher.
    fn apply(w: &World, op: &GroupOp) -> eyre::Result<()> {
        apply_signed_by(w, op, &w.relay_pk)
    }

    fn apply_signed_by(w: &World, op: &GroupOp, signer: &PublicKey) -> eyre::Result<()> {
        let mut ctx = GroupApplyCtx::new_with_apply_auth(
            &w.store,
            &w.group,
            signer,
            &[],
            &LIVE_FALLBACK_AUTHORIZER,
        );
        let handled = dispatch(&mut ctx, op)?;
        assert!(handled, "the dispatcher must recognize the variant");
        Ok(())
    }

    // ── the accept direction ────────────────────────────────────────────

    #[test]
    fn a_member_with_create_rights_may_create_through_a_granted_relay() {
        let w = world();
        check(&w, &bundle(&w)).expect("must be admitted");
    }

    /// The whole feature in one assertion: the RELAY holds no
    /// `CAN_CREATE_CONTEXT`, and must not need it.
    #[test]
    fn the_relay_needs_no_create_rights_of_its_own() {
        let w = world();
        let relay_caps = CapabilitiesRepository::new(&w.store)
            .member_capability(&w.group, &w.relay)
            .expect("read")
            .unwrap_or(0);
        assert_eq!(
            relay_caps & MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
            0,
            "precondition: the relay cannot create contexts for itself"
        );
        check(&w, &bundle(&w)).expect("the author's authority is what counts");
    }

    /// And the self-signed path stays closed to it: the same relay publishing a
    /// plain `ContextRegistered` is refused, which is the gap this op exists for.
    #[test]
    fn the_relay_still_cannot_register_a_context_in_its_own_name() {
        let w = world();
        let plain = GroupOp::ContextRegistered {
            context_id: context(),
            application_id: ApplicationId::from(APP),
            blob_id: BlobId::from([0xBB; 32]),
            source: String::new(),
            service_name: None,
            package: String::new(),
            version: String::new(),
        };
        let _refused = apply(&w, &plain).expect_err("a relay has no create rights of its own");
        assert_eq!(
            get_group_for_context(&w.store, &context()).expect("read"),
            None
        );
    }

    #[test]
    fn an_admin_author_needs_no_capability_bit() {
        let w = world();
        MembershipRepository::new(&w.store)
            .add_member(&w.group, &w.author, GroupMemberRole::Admin)
            .expect("promote");
        CapabilitiesRepository::new(&w.store)
            .set_member_capability(&w.group, &w.author, 0)
            .expect("drop the bit");
        check(&w, &bundle(&w)).expect("an admin may create");
    }

    /// The namespace founder has no member row — its authority lives in the
    /// group meta. It must still be able to create through a relay.
    #[test]
    fn the_genesis_admin_may_create_without_a_member_row() {
        let w = world();
        MetaRepository::new(&w.store)
            .save(&w.group, &sample_meta_with_admin(w.author))
            .expect("make the author the founder");
        MembershipRepository::new(&w.store)
            .remove_member(&w.group, &w.author)
            .expect("drop the row");
        check(&w, &bundle(&w)).expect("the founder may create");
    }

    #[test]
    fn a_tee_relay_acts_by_its_role_without_a_capability_bit() {
        let w = world();
        MembershipRepository::new(&w.store)
            .set_role(&w.group, &w.relay, GroupMemberRole::RelayTee)
            .expect("make it a relay TEE");
        CapabilitiesRepository::new(&w.store)
            .set_member_capability(&w.group, &w.relay, 0)
            .expect("drop the bit");
        check(&w, &bundle(&w)).expect("a RelayTee carries members' creations by its role");
    }

    // ── the author's authority ──────────────────────────────────────────

    #[test]
    fn an_author_without_create_rights_is_refused() {
        let w = world();
        CapabilitiesRepository::new(&w.store)
            .set_member_capability(&w.group, &w.author, 0)
            .expect("drop the bit");
        assert_eq!(
            refusal(check(&w, &bundle(&w))),
            CreationRefusal::AuthorMayNotCreate
        );
    }

    /// A different bit is not the right bit.
    #[test]
    fn an_author_holding_only_other_capabilities_is_refused() {
        let w = world();
        CapabilitiesRepository::new(&w.store)
            .set_member_capability(
                &w.group,
                &w.author,
                (MemberCapabilities::CAN_INVITE_MEMBERS | MemberCapabilities::CAN_CREATE_SUBGROUP)
                    .bits(),
            )
            .expect("set other bits");
        assert_eq!(
            refusal(check(&w, &bundle(&w))),
            CreationRefusal::AuthorMayNotCreate
        );
    }

    #[test]
    fn a_read_only_author_is_refused() {
        let w = world();
        MembershipRepository::new(&w.store)
            .set_role(&w.group, &w.author, GroupMemberRole::ReadOnly)
            .expect("demote");
        assert_eq!(
            refusal(check(&w, &bundle(&w))),
            CreationRefusal::AuthorIsReadOnly
        );
    }

    #[test]
    fn a_removed_author_is_refused() {
        let w = world();
        MembershipRepository::new(&w.store)
            .remove_member(&w.group, &w.author)
            .expect("remove");
        assert_eq!(
            refusal(check(&w, &bundle(&w))),
            CreationRefusal::AuthorNotAMember
        );
    }

    /// A stranger's creation is refused even when the namespace default mask
    /// would grant `CAN_CREATE_CONTEXT` — a default mask is for members.
    #[test]
    fn a_stranger_is_refused_even_under_a_permissive_default_mask() {
        let w = world();
        let stranger_sk = PrivateKey::from([0x5A; 32]);
        let stranger = crate::test_fixtures::account_for(&stranger_sk.public_key());
        CapabilitiesRepository::new(&w.store)
            .set_default_capabilities(&w.group, MemberCapabilities::CAN_CREATE_CONTEXT.bits())
            .expect("permissive default");

        let delegation = ContextCreationDelegation {
            warrant: Box::new(
                ContextCreationWarrant::sign(
                    &stranger_sk,
                    ContextCreationTerms {
                        author_account: stranger,
                        ..terms(&w)
                    },
                )
                .expect("sign"),
            ),
            author_proof: real_join_account(&stranger_sk.public_key()),
            executor_proof: real_join_account(&w.relay_pk),
            executor_key: w.relay_pk,
        };
        assert_eq!(
            refusal(check(&w, &delegation)),
            CreationRefusal::AuthorNotAMember
        );
    }

    #[test]
    fn a_revoked_author_device_is_refused() {
        let w = world();
        let device = bundle(&w).author_proof.statement.device;
        AccountBindingRepository::new(&w.store)
            .apply_revocation(&w.group, device)
            .expect("revoke");
        assert_eq!(
            refusal(check(&w, &bundle(&w))),
            CreationRefusal::AuthorDeviceRevoked
        );
    }

    #[test]
    fn a_revoked_executor_device_is_refused() {
        let w = world();
        let device = bundle(&w).executor_proof.statement.device;
        AccountBindingRepository::new(&w.store)
            .apply_revocation(&w.group, device)
            .expect("revoke");
        assert_eq!(
            refusal(check(&w, &bundle(&w))),
            CreationRefusal::ExecutorDeviceRevoked
        );
    }

    // ── the relay's standing ────────────────────────────────────────────

    #[test]
    fn a_relay_without_an_authorship_grant_is_refused() {
        let w = world();
        CapabilitiesRepository::new(&w.store)
            .set_member_capability(&w.group, &w.relay, 0)
            .expect("drop the grant");
        assert_eq!(
            refusal(check(&w, &bundle(&w))),
            CreationRefusal::Executor(WarrantRefusal::ExecutorMayNotAuthor)
        );
    }

    #[test]
    fn a_tee_replica_never_carries_a_creation() {
        let w = world();
        MembershipRepository::new(&w.store)
            .set_role(&w.group, &w.relay, GroupMemberRole::ReadOnlyTee)
            .expect("make it a replica");
        assert_eq!(
            refusal(check(&w, &bundle(&w))),
            CreationRefusal::Executor(WarrantRefusal::ExecutorIsTeeReplica)
        );
    }

    #[test]
    fn a_read_only_relay_is_refused() {
        let w = world();
        MembershipRepository::new(&w.store)
            .set_role(&w.group, &w.relay, GroupMemberRole::ReadOnly)
            .expect("demote");
        assert_eq!(
            refusal(check(&w, &bundle(&w))),
            CreationRefusal::Executor(WarrantRefusal::ExecutorIsReadOnly)
        );
    }

    #[test]
    fn a_relay_that_is_not_a_member_is_refused() {
        let w = world();
        MembershipRepository::new(&w.store)
            .remove_member(&w.group, &w.relay)
            .expect("remove");
        assert_eq!(
            refusal(check(&w, &bundle(&w))),
            CreationRefusal::Executor(WarrantRefusal::ExecutorMayNotAuthor)
        );
    }

    // ── what the op claims against what was signed ──────────────────────

    #[test]
    fn a_warrant_for_another_group_is_refused() {
        let w = world();
        let delegation = bundle_with(
            &w,
            ContextCreationTerms {
                group: [0x99; 32],
                ..terms(&w)
            },
        );
        assert_eq!(
            refusal(check(&w, &delegation)),
            CreationRefusal::GroupMismatch
        );
    }

    #[test]
    fn a_context_id_the_seed_does_not_derive_is_refused() {
        let w = world();
        assert_eq!(
            refusal(check_on(&w, &bundle(&w), &ContextId::from([0x42; 32]))),
            CreationRefusal::ContextIdMismatch
        );
    }

    #[test]
    fn a_rewritten_application_service_or_name_is_refused() {
        let w = world();
        let delegation = bundle(&w);
        let context_id = context();
        let group = w.group;
        let claim = |app: [u8; 32], service: Option<&str>, name: Option<&str>| {
            let application_id = ApplicationId::from(app);
            let service_name = service.map(str::to_owned);
            let name = name.map(str::to_owned);
            check_warrant_terms(
                &group,
                &w.relay_pk,
                ClaimedRegistration {
                    context_id: &context_id,
                    application_id: &application_id,
                    service_name: &service_name,
                    name: &name,
                },
                &delegation,
            )
            .map(|_| ())
        };

        claim(APP, Some("chat"), Some("general")).expect("the signed claim is accepted");
        assert_eq!(
            claim([0x01; 32], Some("chat"), Some("general")),
            Err(CreationRefusal::ApplicationMismatch)
        );
        assert_eq!(
            claim(APP, Some("other"), Some("general")),
            Err(CreationRefusal::ServiceMismatch)
        );
        assert_eq!(
            claim(APP, None, Some("general")),
            Err(CreationRefusal::ServiceMismatch)
        );
        assert_eq!(
            claim(APP, Some("chat"), Some("random")),
            Err(CreationRefusal::NameMismatch)
        );
        assert_eq!(
            claim(APP, Some("chat"), None),
            Err(CreationRefusal::NameMismatch)
        );
    }

    /// A relay presenting another operator's captured bundle — or signing with a
    /// key other than the one the bundle certifies — is refused.
    #[test]
    fn an_op_signed_by_anyone_but_the_named_executor_key_is_refused() {
        let w = world();
        let impostor = PrivateKey::from([0x77; 32]).public_key();
        let result = check_delegated_creation(
            &w.store,
            &PermissionChecker::new(&w.store, w.group),
            &w.group,
            &impostor,
            ClaimedRegistration {
                context_id: &context(),
                application_id: &ApplicationId::from(APP),
                service_name: &Some("chat".to_owned()),
                name: &Some("general".to_owned()),
            },
            &bundle(&w),
        )
        .map(|_| ());
        assert_eq!(refusal(result), CreationRefusal::SignerIsNotExecutor);
    }

    #[test]
    fn a_warrant_tampered_after_signing_is_refused() {
        let w = world();
        let mut delegation = bundle(&w);
        delegation.warrant.nonce += 1;
        assert!(matches!(
            refusal(check(&w, &delegation)),
            CreationRefusal::InvalidDelegation(_)
        ));
    }

    #[test]
    fn a_bundle_naming_another_executor_account_is_refused() {
        let w = world();
        // Signed for a different executor account: the relay's proof no longer
        // matches the account the warrant names.
        let delegation = bundle_with(
            &w,
            ContextCreationTerms {
                executor: w.author,
                ..terms(&w)
            },
        );
        assert!(matches!(
            refusal(check(&w, &delegation)),
            CreationRefusal::InvalidDelegation(_)
        ));
    }

    // ── at the cut ──────────────────────────────────────────────────────

    /// The capability is decided at the op's cut, like every governance gate:
    /// a projection that says "no" wins over live rows that say "yes".
    #[test]
    fn the_capability_is_decided_at_the_cut_when_there_is_one() {
        let w = world();
        let denied = FixedAuthorizer(false);
        let permissions = PermissionChecker::new(&w.store, w.group).with_apply_auth(CUT, &denied);
        let result = check_delegated_creation(
            &w.store,
            &permissions,
            &w.group,
            &w.relay_pk,
            ClaimedRegistration {
                context_id: &context(),
                application_id: &ApplicationId::from(APP),
                service_name: &Some("chat".to_owned()),
                name: &Some("general".to_owned()),
            },
            &bundle(&w),
        )
        .map(|_| ());
        assert_eq!(refusal(result), CreationRefusal::AuthorMayNotCreate);
    }

    /// And a projection that says "yes" admits an author whose live row has
    /// since lost the bit — the op is judged as authored.
    #[test]
    fn a_grant_at_the_cut_admits_despite_a_later_live_revoke() {
        let w = world();
        CapabilitiesRepository::new(&w.store)
            .set_member_capability(&w.group, &w.author, 0)
            .expect("revoke live");
        let granted = FixedAuthorizer(true);
        let permissions = PermissionChecker::new(&w.store, w.group).with_apply_auth(CUT, &granted);
        check_delegated_creation(
            &w.store,
            &permissions,
            &w.group,
            &w.relay_pk,
            ClaimedRegistration {
                context_id: &context(),
                application_id: &ApplicationId::from(APP),
                service_name: &Some("chat".to_owned()),
                name: &Some("general".to_owned()),
            },
            &bundle(&w),
        )
        .expect("the verdict at the cut decides");
    }

    /// An unresolvable cut parks the op (retried later), it does not refuse it —
    /// a refusal here would diverge this replica from peers that had folded it.
    #[test]
    fn an_unresolvable_cut_parks_instead_of_refusing() {
        let w = world();
        let unresolvable = UnresolvableAuthorizer;
        let permissions =
            PermissionChecker::new(&w.store, w.group).with_apply_auth(CUT, &unresolvable);
        let err = check_delegated_creation(
            &w.store,
            &permissions,
            &w.group,
            &w.relay_pk,
            ClaimedRegistration {
                context_id: &context(),
                application_id: &ApplicationId::from(APP),
                service_name: &Some("chat".to_owned()),
                name: &Some("general".to_owned()),
            },
            &bundle(&w),
        )
        .expect_err("must not decide");
        assert!(
            err.downcast_ref::<CreationRefusal>().is_none(),
            "an undecidable cut is not a refusal: {err:?}"
        );
        assert!(
            err.downcast_ref::<crate::ApplyError>()
                .is_some_and(|e| matches!(e, crate::ApplyError::AuthorityUndecidable { .. })),
            "expected AuthorityUndecidable, got {err:?}"
        );
    }

    // ── the apply handler ───────────────────────────────────────────────

    #[test]
    fn applying_the_op_registers_the_context_with_its_service_and_name() {
        let w = world();
        apply(&w, &op(bundle(&w))).expect("apply");

        assert_eq!(
            get_group_for_context(&w.store, &context()).expect("read"),
            Some(w.group)
        );
        assert_eq!(
            get_context_service_name(&w.store, &context()).expect("read"),
            Some("chat".to_owned())
        );
        let meta = MetadataRepository::new(&w.store)
            .context_metadata(&w.group, &context())
            .expect("read")
            .expect("the name is recorded with the registration");
        assert_eq!(meta.name.as_deref(), Some("general"));
        assert_eq!(
            meta.updated_by,
            w.author_sk.public_key(),
            "the name is attributed to the author's device, not the relay"
        );
    }

    #[test]
    fn applying_a_refused_op_writes_nothing() {
        let w = world();
        CapabilitiesRepository::new(&w.store)
            .set_member_capability(&w.group, &w.author, 0)
            .expect("drop the bit");
        let _refused = apply(&w, &op(bundle(&w))).expect_err("refused");

        assert_eq!(
            get_group_for_context(&w.store, &context()).expect("read"),
            None
        );
        assert_eq!(
            MetadataRepository::new(&w.store)
                .context_metadata(&w.group, &context())
                .expect("read"),
            None
        );
    }

    #[test]
    fn a_replayed_creation_warrant_is_refused() {
        let w = world();
        apply(&w, &op(bundle(&w))).expect("first apply");

        let err = apply(&w, &op(bundle(&w))).expect_err("a replay must be refused");
        assert_eq!(
            err.downcast_ref::<CreationRefusal>(),
            Some(&CreationRefusal::AlreadySpent)
        );
    }

    /// The case the nonce ledger exists for: an admin detaches the context, and
    /// the relay tries to put it back with the warrant it already spent.
    #[test]
    fn a_detached_context_cannot_be_re_registered_by_replaying_its_warrant() {
        let w = world();
        apply(&w, &op(bundle(&w))).expect("first apply");
        crate::contexts::unregister_context_from_group(&w.store, &w.group, &context())
            .expect("detach");

        let err = apply(&w, &op(bundle(&w))).expect_err("a replay must be refused");
        assert_eq!(
            err.downcast_ref::<CreationRefusal>(),
            Some(&CreationRefusal::AlreadySpent)
        );
        assert_eq!(
            get_group_for_context(&w.store, &context()).expect("read"),
            None
        );
    }

    /// Registering never moves a context: one registered to another group stays there.
    #[test]
    fn a_delegated_registration_cannot_move_a_context_out_of_another_group() {
        let w = world();
        let theirs = ContextGroupId::from([0xC9; 32]);
        crate::context_tree::ContextTreeService::new(&w.store, theirs)
            .register_context(&context())
            .expect("registered to another group");

        let _refused = apply(&w, &op(bundle(&w))).expect_err("must not move the context");
        assert_eq!(
            get_group_for_context(&w.store, &context()).expect("read"),
            Some(theirs)
        );
    }

    /// A second, fresh warrant (new seed) from the same author creates a second
    /// context — the ledger is per context, not a global spend of the device.
    #[test]
    fn a_fresh_warrant_creates_a_second_context() {
        let w = world();
        apply(&w, &op(bundle(&w))).expect("first");

        let seed = [0xD7; 32];
        let second = bundle_with(&w, ContextCreationTerms { seed, ..terms(&w) });
        let op2 = match op(second) {
            GroupOp::ContextRegisteredOnBehalf {
                application_id,
                blob_id,
                source,
                service_name,
                package,
                version,
                name,
                delegation,
                ..
            } => GroupOp::ContextRegisteredOnBehalf {
                context_id: ContextId::from_seed(seed),
                application_id,
                blob_id,
                source,
                service_name,
                package,
                version,
                name,
                delegation,
            },
            _ => unreachable!(),
        };
        apply(&w, &op2).expect("second");
        assert_eq!(
            get_group_for_context(&w.store, &ContextId::from_seed(seed)).expect("read"),
            Some(w.group)
        );
    }

    #[test]
    fn the_op_must_be_published_by_the_executor_key() {
        let w = world();
        let author_pk = w.author_sk.public_key();
        let err = apply_signed_by(&w, &op(bundle(&w)), &author_pk)
            .expect_err("only the named executor key may publish it");
        assert_eq!(
            err.downcast_ref::<CreationRefusal>(),
            Some(&CreationRefusal::SignerIsNotExecutor)
        );
    }

    // ── subgroups: where contexts actually live ─────────────────────────

    /// Contexts (channels, DMs) live in subgroups. A member who can create in
    /// the subgroup, served by a relay admitted once at the namespace root,
    /// must be able to create there.
    #[test]
    fn a_creation_in_an_open_subgroup_through_a_root_relay_is_admitted() {
        let store = test_store();
        let namespace = ContextGroupId::from([0xB1; 32]);
        let subgroup = ContextGroupId::from([0xB2; 32]);
        for gid in [namespace, subgroup] {
            MetaRepository::new(&store)
                .save(
                    &gid,
                    &sample_meta_with_admin(AccountId::from(GENESIS_ADMIN)),
                )
                .expect("meta");
        }
        nest_for_test(&store, &namespace, &subgroup);
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
            .expect("open");

        let author_sk = PrivateKey::from(AUTHOR_SK);
        let relay_sk = PrivateKey::from(RELAY_SK);
        let author = enrol_member(&store, &namespace, &author_sk.public_key());
        let relay = enrol_member(&store, &namespace, &relay_sk.public_key());
        let membership = MembershipRepository::new(&store);
        membership
            .add_member(&subgroup, &author, GroupMemberRole::Member)
            .expect("author in subgroup");
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &subgroup,
                &author,
                MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
            )
            .expect("author may create in the subgroup");
        membership
            .add_member(&namespace, &relay, GroupMemberRole::RelayTee)
            .expect("relay admitted at the root");
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &namespace,
                &relay,
                MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
            )
            .expect("relay inherits into open subgroups");

        let delegation = ContextCreationDelegation {
            warrant: Box::new(
                ContextCreationWarrant::sign(
                    &author_sk,
                    ContextCreationTerms {
                        group: subgroup.to_bytes(),
                        seed: SEED,
                        author_account: author,
                        executor: relay,
                        application_id: ApplicationId::from(APP),
                        service_name: None,
                        name: None,
                        init_hash: ContextCreationWarrant::init_hash(INIT),
                        account_heads: vec![],
                        governance_floor: vec![],
                        nonce: 1,
                        not_after: u64::MAX,
                    },
                )
                .expect("sign"),
            ),
            author_proof: real_join_account(&author_sk.public_key()),
            executor_proof: real_join_account(&relay_sk.public_key()),
            executor_key: relay_sk.public_key(),
        };
        check_delegated_creation(
            &store,
            &PermissionChecker::new(&store, subgroup),
            &subgroup,
            &relay_sk.public_key(),
            ClaimedRegistration {
                context_id: &context(),
                application_id: &ApplicationId::from(APP),
                service_name: &None,
                name: &None,
            },
            &delegation,
        )
        .expect("a subgroup creation through a root relay must be admitted");
    }
}
