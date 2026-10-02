//! A delegated `TargetApplicationSet` is a group's FIRST application choice, or
//! nothing.
//!
//! # Why it is shaped this way
//!
//! A relay may carry a member's first choice of application (a namespace
//! founded through a relay has none, and without one no context can be created
//! in it), but never an upgrade: moving a group from the code it runs to other
//! code is a decision every member lives with, and it stays with a node that
//! signs it itself. So the question every peer asks of a delegated
//! `TargetApplicationSet` is "had this group chosen an application yet?".
//!
//! **Answered at the op's cut, not from live rows.** A peer that has already
//! folded a concurrent node-path `TargetApplicationSet` reads a non-zero target
//! live, while one that has not reads zero, so a live read would apply the same
//! op on one replica and refuse it on the other, and nothing later reconciles
//! them. Instead the op's own ancestry is searched for anything that gave the
//! group a target: a `TargetApplicationSet` on the group (signed or delegated),
//! a `CascadeUpgrade` that could have matched a group with no target, or — for
//! a subgroup — the target it inherited when it was created, which is its
//! parent's at that creation's own cut. The ancestry is the same set of ops on
//! every peer that holds it, so every such peer reaches the same verdict.
//!
//! **An ancestry this node cannot read** (an op missing from its log, or one of
//! the group's ops it holds no key for) is not "no target": it is handled like
//! any at-cut gate that cannot resolve its cut. Where the op has no cut, or the
//! apply has no projection to contradict the live rows, the live target
//! decides; otherwise the op parks as undecidable and is retried once the
//! history arrives.
//!
//! The walk runs only for a delegated `TargetApplicationSet`, which a group
//! sees at most once in its life, so its cost is not on any hot path.

use std::collections::{HashSet, VecDeque};

use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
use calimero_primitives::application::ZERO_APPLICATION_ID;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use crate::authorizer::AtCutAuthorizer;
use crate::delegation_gate::DelegationRefusal;
use crate::namespace::NamespaceOpLogService;
use crate::{GroupKeyring, MetaRepository, NamespaceRepository};

/// What the ancestry of a cut says about a group's application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetAtCut {
    /// Nothing in the ancestry gave the group an application.
    Unchosen,
    /// Something in the ancestry did.
    Chosen,
    /// Part of the ancestry is not readable here.
    Unknown,
}

/// Refuse a delegated `TargetApplicationSet` unless `group_id` had chosen no
/// application at the cut named by `parents`.
///
/// # Errors
/// [`DelegationRefusal::TargetApplicationAlreadyChosen`];
/// `ApplyError::AuthorityUndecidable` when the cut is real but its ancestry is
/// not readable here (retried, not a refusal); or a store failure.
pub(crate) fn refuse_unless_first_target(
    store: &Store,
    group_id: &ContextGroupId,
    parents: &[[u8; 32]],
    authorizer: &dyn AtCutAuthorizer,
) -> EyreResult<()> {
    let verdict = if parents.is_empty() {
        // No causal context to contradict the live rows: the same rule every
        // at-cut gate follows for an empty cut.
        TargetAtCut::Unknown
    } else {
        let namespace = NamespaceRepository::new(store).resolve(group_id)?;
        let walk = AncestryWalk {
            store,
            namespace: NamespaceId::from(namespace.to_bytes()),
        };
        walk.target_at(*group_id, parents)?
    };
    let chosen = match verdict {
        TargetAtCut::Unchosen => false,
        TargetAtCut::Chosen => true,
        TargetAtCut::Unknown => {
            if !parents.is_empty() && !authorizer.can_resolve_cut(group_id, parents) {
                bail!(crate::ApplyError::AuthorityUndecidable {
                    group_id: group_id.to_string(),
                    signer: "the group's application at the op's cut".to_owned(),
                });
            }
            targets_an_application_now(store, group_id)?
        }
    };
    if chosen {
        return Err(DelegationRefusal::TargetApplicationAlreadyChosen(group_id.to_string()).into());
    }
    Ok(())
}

/// Refuse, from this node's current view, a delegated `TargetApplicationSet`
/// every peer would refuse on apply: the relay's API asks this before it
/// resolves or publishes anything, so a member is told `403` rather than having
/// the op dropped everywhere.
///
/// # Errors
/// [`DelegationRefusal::TargetApplicationAlreadyChosen`], or a store failure.
pub fn refuse_unless_untargeted(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    if targets_an_application_now(store, group_id)? {
        return Err(DelegationRefusal::TargetApplicationAlreadyChosen(group_id.to_string()).into());
    }
    Ok(())
}

fn targets_an_application_now(store: &Store, group_id: &ContextGroupId) -> EyreResult<bool> {
    Ok(MetaRepository::new(store)
        .load(group_id)?
        .is_some_and(|meta| meta.target.application_id != ZERO_APPLICATION_ID))
}

struct AncestryWalk<'a> {
    store: &'a Store,
    namespace: NamespaceId,
}

/// What one op in the ancestry says about the group being asked after.
enum Finding {
    Nothing,
    /// It gave the group an application.
    Chosen,
    /// It created the group under this parent, inheriting the parent's target.
    CreatedUnder(ContextGroupId),
    /// It concerns the group but cannot be read here.
    Unreadable,
}

impl AncestryWalk<'_> {
    /// Whether anything in the ancestry of `parents` gave `group` an application.
    fn target_at(&self, group: ContextGroupId, parents: &[[u8; 32]]) -> EyreResult<TargetAtCut> {
        let op_log = NamespaceOpLogService::new(self.store, self.namespace);
        let mut unknown = false;
        let mut visited: HashSet<[u8; 32]> = HashSet::new();
        let mut queue: VecDeque<[u8; 32]> = parents.iter().copied().collect();
        while let Some(hash) = queue.pop_front() {
            if !visited.insert(hash) {
                continue;
            }
            // Absent, or present in a form this node cannot decode (an opaque
            // skeleton): either way this is ancestry it cannot vouch for.
            let Ok(Some(op)) = op_log.get_signed_op(hash) else {
                unknown = true;
                continue;
            };
            match self.finding(group, &op)? {
                Finding::Nothing => {}
                Finding::Chosen => return Ok(TargetAtCut::Chosen),
                Finding::Unreadable => unknown = true,
                // The group began with whatever its parent targeted at the
                // creation's own cut; nothing older concerns it.
                Finding::CreatedUnder(parent) => {
                    match self.target_at(parent, &op.parent_op_hashes)? {
                        TargetAtCut::Chosen => return Ok(TargetAtCut::Chosen),
                        TargetAtCut::Unknown => unknown = true,
                        TargetAtCut::Unchosen => {}
                    }
                    continue;
                }
            }
            queue.extend(op.parent_op_hashes.iter().copied());
        }
        Ok(if unknown {
            TargetAtCut::Unknown
        } else {
            TargetAtCut::Unchosen
        })
    }

    fn finding(&self, group: ContextGroupId, op: &SignedNamespaceOp) -> EyreResult<Finding> {
        match &op.op {
            NamespaceOp::Root(root) => Ok(created(group, root)),
            NamespaceOp::RootSealed { key_id, encrypted } => {
                match crate::namespace::open_sealed_root_op(
                    self.store,
                    self.namespace,
                    key_id.as_bytes(),
                    encrypted,
                ) {
                    Ok(Some(root)) => Ok(created(group, &root)),
                    // A sealed root op may be the group's creation; unread, it
                    // is ancestry this node cannot vouch for.
                    Ok(None) | Err(_) => Ok(Finding::Unreadable),
                }
            }
            NamespaceOp::Group {
                group_id,
                key_id,
                encrypted,
                ..
            } => {
                let ours = *group_id == group;
                let Some(inner) = self.open_group_op(*group_id, key_id.as_bytes(), encrypted)?
                else {
                    // Another group's op this node cannot read cannot be a
                    // target set on this group; only a cascade could reach
                    // it, and one from a group with no application matches
                    // nothing (see below).
                    return Ok(if ours {
                        Finding::Unreadable
                    } else {
                        Finding::Nothing
                    });
                };
                Ok(match unwrap_on_behalf(&inner) {
                    GroupOp::TargetApplicationSet { .. } if ours => Finding::Chosen,
                    // A cascade rewrites every descendant whose bytecode equals
                    // `from_bytecode_id`, and a group with no application has
                    // the zero one. Which groups were descendants at the time is
                    // not recorded, so any such cascade counts.
                    GroupOp::CascadeUpgrade {
                        from_bytecode_id, ..
                    } if from_bytecode_id.to_bytes() == [0u8; 32] => Finding::Chosen,
                    _ => Finding::Nothing,
                })
            }
            // The remaining envelopes carry joins, which set no application.
            _ => Ok(Finding::Nothing),
        }
    }

    /// Decrypt a group op with the key it names, from the group's keyring or,
    /// for a group encrypted under the namespace key, the namespace's.
    fn open_group_op(
        &self,
        group: ContextGroupId,
        key_id: &[u8; 32],
        encrypted: &calimero_governance_types::EncryptedGroupOp,
    ) -> EyreResult<Option<GroupOp>> {
        let key = match GroupKeyring::new(self.store, group).load_key_by_id(key_id)? {
            Some(key) => Some(key),
            None => GroupKeyring::new(self.store, ContextGroupId::from(self.namespace.to_bytes()))
                .load_key_by_id(key_id)?,
        };
        Ok(key.and_then(|key| GroupKeyring::decrypt_op(&key, encrypted).ok()))
    }
}

fn unwrap_on_behalf(op: &GroupOp) -> &GroupOp {
    match op {
        GroupOp::OnBehalf { op, .. } => op,
        other => other,
    }
}

fn created(group: ContextGroupId, root: &RootOp) -> Finding {
    let root = match root {
        RootOp::OnBehalf { op, .. } => op,
        other => other,
    };
    match root {
        RootOp::GroupCreated {
            group_id,
            parent_id,
            ..
        } if *group_id == group => Finding::CreatedUnder(*parent_id),
        _ => Finding::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use calimero_account::{
        AccountId, GovernanceDelegation, GovernanceOpKind, GovernanceTerms, GovernanceWarrant,
    };
    use calimero_context_client::local_governance::{
        GroupOp, NamespaceOp, RootOp, SignedNamespaceOp,
    };
    use calimero_context_config::types::{BytecodeId, ContextGroupId};
    use calimero_context_config::MemberCapabilities;
    use calimero_primitives::application::{ApplicationId, ZERO_APPLICATION_ID};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::Store;

    use crate::delegation_gate::DelegationRefusal;
    use crate::namespace::NamespaceGovernance;
    use crate::test_fixtures::{
        account_for, derived_group_id, enrol_member, real_join_account, test_store,
    };
    use crate::{CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository};

    const SALT: [u8; 32] = [0x6D; 32];
    const NAMESPACE_KEY: [u8; 32] = [0x4B; 32];
    const APP: [u8; 32] = [0x88; 32];
    const BUNDLE: [u8; 32] = [0x77; 32];

    /// A namespace founded through a relay, exactly as production founds one:
    /// its genesis carries no application, so the root targets zero.
    struct Rig {
        store: Store,
        ns: ContextGroupId,
        founder_sk: PrivateKey,
        relay_sk: PrivateKey,
        relay: AccountId,
        genesis: [u8; 32],
        nonce: Cell<u64>,
    }

    fn rig() -> Rig {
        let founder_sk = PrivateKey::from([0x1C; 32]);
        let relay_sk = PrivateKey::from([0x2D; 32]);
        let founder = account_for(&founder_sk.public_key());
        let relay = account_for(&relay_sk.public_key());
        let ns = ContextGroupId::from(calimero_account::founded_namespace_id(&founder, &SALT));
        let rig = Rig {
            store: test_store(),
            ns,
            founder_sk,
            relay_sk,
            relay,
            genesis: [0; 32],
            nonce: Cell::new(0),
        };
        let _key_id = GroupKeyring::new(&rig.store, ns)
            .store_key(&NAMESPACE_KEY)
            .expect("the namespace key the relay minted");
        let genesis = RootOp::NamespaceCreatedV2 {
            founder,
            account: real_join_account(&rig.founder_sk.public_key()),
            salt: SALT,
        };
        let wrapped = RootOp::OnBehalf {
            delegation: Box::new(rig.delegation(
                ns,
                &rig.founder_sk,
                GovernanceOpKind::Root,
                &borsh::to_vec(&genesis).expect("encode"),
            )),
            op: Box::new(genesis),
        };
        let genesis = rig
            .apply(&rig.relay_sk, vec![], NamespaceOp::Root(wrapped))
            .expect("founded through the relay");
        assert_eq!(rig.target(&ns), ZERO_APPLICATION_ID, "no application yet");
        Rig { genesis, ..rig }
    }

    fn first(version: &str) -> GroupOp {
        GroupOp::TargetApplicationSet {
            bytecode_id: BytecodeId::from(BUNDLE),
            target_application_id: ApplicationId::from(APP),
            package: "com.example.app".to_owned(),
            version: version.to_owned(),
        }
    }

    impl Rig {
        fn next_nonce(&self) -> u64 {
            self.nonce.set(self.nonce.get() + 1);
            self.nonce.get()
        }

        fn delegation(
            &self,
            scope: ContextGroupId,
            author_sk: &PrivateKey,
            kind: GovernanceOpKind,
            form: &[u8],
        ) -> GovernanceDelegation {
            GovernanceDelegation {
                warrant: Box::new(
                    GovernanceWarrant::sign(
                        author_sk,
                        GovernanceTerms {
                            scope: scope.to_bytes(),
                            kind,
                            author_account: account_for(&author_sk.public_key()),
                            executor: self.relay,
                            op_hash: GovernanceWarrant::op_hash(kind, form),
                            account_heads: vec![],
                            governance_floor: vec![],
                            nonce: self.next_nonce(),
                            not_after: u64::MAX,
                        },
                    )
                    .expect("sign"),
                ),
                author_proof: real_join_account(&author_sk.public_key()),
                executor_proof: real_join_account(&self.relay_sk.public_key()),
                executor_key: self.relay_sk.public_key(),
            }
        }

        /// Sign and apply one namespace op on `parents`, returning its hash.
        fn apply(
            &self,
            signer: &PrivateKey,
            parents: Vec<[u8; 32]>,
            op: NamespaceOp,
        ) -> eyre::Result<[u8; 32]> {
            let signed = SignedNamespaceOp::sign(
                signer,
                self.ns.to_bytes().into(),
                parents,
                self.next_nonce(),
                op,
            )?;
            let _applied = NamespaceGovernance::new(&self.store, self.ns.to_bytes().into())
                .apply_signed_op(&signed)?;
            Ok(signed.content_hash()?)
        }

        fn group_op(
            &self,
            signer: &PrivateKey,
            group: ContextGroupId,
            parents: Vec<[u8; 32]>,
            op: &GroupOp,
        ) -> eyre::Result<[u8; 32]> {
            self.apply(
                signer,
                parents,
                NamespaceOp::Group {
                    group_id: group,
                    key_id: GroupKeyring::key_id_for(&NAMESPACE_KEY).into(),
                    encrypted: GroupKeyring::encrypt_op(&NAMESPACE_KEY, op)?,
                    key_rotation: None,
                },
            )
        }

        /// `author_sk` asks the relay to publish `inner`: the warrant commits to
        /// its delegable form, the relay publishes it with `bytecode_id` filled.
        fn delegated(
            &self,
            author_sk: &PrivateKey,
            group: ContextGroupId,
            parents: Vec<[u8; 32]>,
            inner: GroupOp,
        ) -> eyre::Result<[u8; 32]> {
            let wrapped = self.wrapped(author_sk, group, inner);
            self.group_op(&self.relay_sk, group, parents, &wrapped)
        }

        fn wrapped(
            &self,
            author_sk: &PrivateKey,
            group: ContextGroupId,
            inner: GroupOp,
        ) -> GroupOp {
            let form = borsh::to_vec(&inner.delegable_form().expect("delegable")).expect("encode");
            GroupOp::OnBehalf {
                delegation: Box::new(self.delegation(
                    group,
                    author_sk,
                    GovernanceOpKind::Group,
                    &form,
                )),
                op: Box::new(inner),
            }
        }

        fn target(&self, group: &ContextGroupId) -> ApplicationId {
            MetaRepository::new(&self.store)
                .load(group)
                .expect("read")
                .expect("the group exists")
                .target
                .application_id
        }

        /// A plain member of the namespace holding `caps`.
        fn member(&self, seed: u8, caps: MemberCapabilities) -> PrivateKey {
            let sk = PrivateKey::from([seed; 32]);
            let account = enrol_member(&self.store, &self.ns, &sk.public_key());
            MembershipRepository::new(&self.store)
                .add_member(&self.ns, &account, GroupMemberRole::Member)
                .expect("seat");
            CapabilitiesRepository::new(&self.store)
                .set_member_capability(&self.ns, &account, caps.bits())
                .expect("caps");
            sk
        }

        /// An Open subgroup the founder creates on `parents`, salted `[tag; 32]`:
        /// its id and the creating op's hash.
        fn subgroup(&self, tag: u8, parents: Vec<[u8; 32]>) -> ([u8; 32], [u8; 32]) {
            let admin = account_for(&self.founder_sk.public_key());
            let id = derived_group_id(&admin, self.ns.to_bytes(), false, tag);
            let created = RootOp::GroupCreated {
                group_id: id.into(),
                parent_id: self.ns,
                restricted: false,
                admin,
                salt: [tag; 32],
            };
            let sealed =
                crate::seal_root_op_for_publish(&self.store, self.ns.to_bytes().into(), created)
                    .expect("seal");
            let hash = self
                .apply(&self.founder_sk, parents, sealed)
                .expect("the founder creates a subgroup");
            // The founder's node created it, so seat the relay to serve it, as
            // the founder would by granting it authorship there.
            let group = ContextGroupId::from(id);
            MembershipRepository::new(&self.store)
                .add_member(&group, &self.relay, GroupMemberRole::Member)
                .expect("seat the relay");
            CapabilitiesRepository::new(&self.store)
                .set_member_capability(
                    &group,
                    &self.relay,
                    MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
                )
                .expect("grant authorship");
            (id, hash)
        }
    }

    fn refusal(result: eyre::Result<[u8; 32]>) -> Option<DelegationRefusal> {
        result
            .expect_err("refused")
            .chain()
            .find_map(|cause| cause.downcast_ref::<DelegationRefusal>())
            .cloned()
    }

    fn already_chosen(result: eyre::Result<[u8; 32]>) -> bool {
        matches!(
            refusal(result),
            Some(DelegationRefusal::TargetApplicationAlreadyChosen(_))
        )
    }

    #[test]
    fn a_founder_sets_the_first_application_through_a_relay() {
        let r = rig();
        let _set = r
            .delegated(&r.founder_sk, r.ns, vec![r.genesis], first("1.2.3"))
            .expect("a first choice rides the relay");
        let meta = MetaRepository::new(&r.store)
            .load(&r.ns)
            .expect("read")
            .expect("meta");
        assert_eq!(meta.target.application_id, ApplicationId::from(APP));
        assert_eq!(
            meta.target.bytecode_id, BUNDLE,
            "the bundle blob id the relay filled in"
        );
        assert_eq!(&*meta.target.package, "com.example.app");
        assert_eq!(&*meta.target.version, "1.2.3");
    }

    /// Changing an application a group already runs is an upgrade, and an
    /// upgrade is published by a node as itself, never by a relay.
    #[test]
    fn an_upgrade_does_not_ride_a_relay() {
        let r = rig();
        let set = r
            .delegated(&r.founder_sk, r.ns, vec![r.genesis], first("1.2.3"))
            .expect("first");
        assert!(already_chosen(r.delegated(
            &r.founder_sk,
            r.ns,
            vec![set],
            first("2.0.0")
        )));
        let meta = MetaRepository::new(&r.store)
            .load(&r.ns)
            .expect("read")
            .expect("meta");
        assert_eq!(&*meta.target.version, "1.2.3", "the target did not move");
    }

    #[test]
    fn a_target_the_founder_set_as_itself_refuses_a_delegated_one() {
        let r = rig();
        let own = r
            .group_op(&r.founder_sk, r.ns, vec![r.genesis], &first("1.0.0"))
            .expect("the founder's node sets the target as itself");
        assert!(already_chosen(r.delegated(
            &r.founder_sk,
            r.ns,
            vec![own],
            first("1.2.3")
        )));
    }

    #[test]
    fn a_member_needs_manage_application_to_choose() {
        let r = rig();
        let plain = r.member(0x3A, MemberCapabilities::CAN_CREATE_CONTEXT);
        let err = r
            .delegated(&plain, r.ns, vec![r.genesis], first("1.2.3"))
            .expect_err("a member without MANAGE_APPLICATION");
        assert!(
            err.chain()
                .any(|c| c.downcast_ref::<crate::CapabilitiesError>().is_some()),
            "refused by the ordinary MANAGE_APPLICATION gate, as the author: {err:?}"
        );
        assert_eq!(r.target(&r.ns), ZERO_APPLICATION_ID);

        let manager = r.member(0x3B, MemberCapabilities::MANAGE_APPLICATION);
        let _set = r
            .delegated(&manager, r.ns, vec![r.genesis], first("1.2.3"))
            .expect("MANAGE_APPLICATION is the author's authority to choose");
        assert_eq!(r.target(&r.ns), ApplicationId::from(APP));
    }

    /// The relay fills `bytecode_id` and nothing else: the version it publishes
    /// is the one the member signed.
    #[test]
    fn the_relay_may_fill_only_the_bytecode() {
        let r = rig();
        let form =
            borsh::to_vec(&first("1.2.3").delegable_form().expect("delegable")).expect("encode");
        let swapped = GroupOp::OnBehalf {
            delegation: Box::new(r.delegation(r.ns, &r.founder_sk, GovernanceOpKind::Group, &form)),
            op: Box::new(first("6.6.6")),
        };
        assert_eq!(
            refusal(r.group_op(&r.relay_sk, r.ns, vec![r.genesis], &swapped)),
            Some(DelegationRefusal::OpMismatch)
        );
        assert_eq!(r.target(&r.ns), ZERO_APPLICATION_ID);
    }

    /// Two first choices concurrent with each other: a node-path one and a
    /// delegated one, both on the genesis. Whichever this replica applies first,
    /// the delegated one was a first choice at its own cut, so it is admitted
    /// here exactly as on a replica that received it first.
    #[test]
    fn a_concurrent_first_choice_is_admitted_whatever_arrived_first() {
        let r = rig();
        let _own = r
            .group_op(&r.founder_sk, r.ns, vec![r.genesis], &first("1.0.0"))
            .expect("applied first here");
        assert_ne!(
            r.target(&r.ns),
            ZERO_APPLICATION_ID,
            "live, the group targets"
        );
        let _delegated = r
            .delegated(&r.founder_sk, r.ns, vec![r.genesis], first("1.2.3"))
            .expect("at its cut the group had chosen nothing");
    }

    /// The verdict comes from the op's ancestry, not the row: a replica whose
    /// live target reads zero still refuses an op whose cut includes a choice.
    #[test]
    fn a_choice_in_the_ancestry_refuses_even_where_the_row_reads_zero() {
        let r = rig();
        let own = r
            .group_op(&r.founder_sk, r.ns, vec![r.genesis], &first("1.0.0"))
            .expect("set");
        let mut meta = MetaRepository::new(&r.store)
            .load(&r.ns)
            .expect("read")
            .expect("meta");
        meta.target.application_id = ZERO_APPLICATION_ID;
        MetaRepository::new(&r.store)
            .save(&r.ns, &meta)
            .expect("zero the row");
        assert!(already_chosen(r.delegated(
            &r.founder_sk,
            r.ns,
            vec![own],
            first("1.2.3")
        )));
    }

    /// A subgroup starts with its parent's target at its creation's cut: one
    /// created after the root chose has chosen too, one created before has not.
    #[test]
    fn a_subgroup_inherits_the_verdict_from_its_creation_cut() {
        let r = rig();
        let (e1, early) = r.subgroup(0xE1, vec![r.genesis]);
        let root_choice = r
            .delegated(&r.founder_sk, r.ns, vec![early], first("1.2.3"))
            .expect("the root chooses");
        let (e2, late) = r.subgroup(0xE2, vec![root_choice]);

        // Decided by the ancestry, readable in full here, not by the rows.
        let walk = super::AncestryWalk {
            store: &r.store,
            namespace: r.ns.to_bytes().into(),
        };
        assert_eq!(
            walk.target_at(ContextGroupId::from(e2), &[late])
                .expect("walk"),
            super::TargetAtCut::Chosen
        );
        assert_eq!(
            walk.target_at(ContextGroupId::from(e1), &[late])
                .expect("walk"),
            super::TargetAtCut::Unchosen
        );
        for (group, application) in [(e1, [0x99; 32]), (e2, [0u8; 32])] {
            let group = ContextGroupId::from(group);
            let mut meta = MetaRepository::new(&r.store)
                .load(&group)
                .expect("read")
                .expect("meta");
            meta.target.application_id = ApplicationId::from(application);
            MetaRepository::new(&r.store)
                .save(&group, &meta)
                .expect("skew the row");
        }

        assert!(
            already_chosen(r.delegated(
                &r.founder_sk,
                ContextGroupId::from(e2),
                vec![late],
                first("1.2.3")
            )),
            "created after the root chose, so it inherited a target"
        );
        let _first = r
            .delegated(
                &r.founder_sk,
                ContextGroupId::from(e1),
                vec![late],
                first("1.2.3"),
            )
            .expect("created before the root chose, so its first choice is still open");
    }

    /// An at-cut authorizer whose projection has not folded the op's cut, as a
    /// replica mid-backfill has: it can resolve nothing there but the relay's
    /// standing, which it answers from the store. Warrant admission reads
    /// standing at the cut first and would park the op there, before this
    /// gate is reached; answering it keeps the test on this gate's own read.
    struct Unfolded<'s>(&'s Store);

    impl crate::authorizer::AtCutAuthorizer for Unfolded<'_> {
        fn standing_reads_at_cut<'a>(
            &'a self,
            _: &ContextGroupId,
            _: &[[u8; 32]],
        ) -> Option<Box<dyn crate::StandingReads + 'a>> {
            Some(Box::new(crate::warrant_admission::LiveReads::new(self.0)))
        }
        fn is_admin_at_cut(
            &self,
            _: &ContextGroupId,
            _: &calimero_primitives::identity::PublicKey,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            None
        }
        fn is_admin_or_capability_at_cut(
            &self,
            _: &ContextGroupId,
            _: &calimero_primitives::identity::PublicKey,
            _: u32,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            None
        }
        fn is_admin_or_capability_account_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: u32,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            None
        }
        fn is_admin_account_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            None
        }
        fn is_last_admin_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            None
        }
        fn membership_path_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<crate::authorizer::AtCutMembershipPath> {
            None
        }
        fn effective_role_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<Option<calimero_primitives::context::GroupMemberRole>> {
            None
        }
        fn context_rotation_group_at_cut(
            &self,
            _: &ContextGroupId,
            _: &calimero_primitives::context::ContextId,
            _: &[[u8; 32]],
        ) -> Option<Option<ContextGroupId>> {
            None
        }
        fn can_resolve_cut(&self, _: &ContextGroupId, _: &[[u8; 32]]) -> bool {
            false
        }
    }

    /// An ancestry this node cannot read is not "no application": the op parks
    /// as undecidable instead of being judged against the live row, which reads
    /// zero here and would admit it.
    #[test]
    fn an_unreadable_ancestry_parks_rather_than_guessing() {
        let r = rig();
        let op = r.wrapped(&r.founder_sk, r.ns, first("1.2.3"));
        let missing: &[[u8; 32]] = &[[0xAB; 32]];
        let relay_pk = r.relay_sk.public_key();
        let unfolded = Unfolded(&r.store);
        let mut ctx = crate::ops::group::GroupApplyCtx::new_with_apply_auth(
            &r.store, &r.ns, &relay_pk, missing, &unfolded,
        );
        let err = crate::ops::group::dispatch(&mut ctx, &op).expect_err("undecidable");
        assert!(
            err.chain().any(|c| matches!(
                c.downcast_ref::<crate::ApplyError>(),
                Some(crate::ApplyError::AuthorityUndecidable { signer, .. })
                    if signer.contains("application at the op's cut")
            )),
            "undecidable at this gate, not a later one: {err:?}"
        );
        assert_eq!(r.target(&r.ns), ZERO_APPLICATION_ID, "nothing written");
    }
}
