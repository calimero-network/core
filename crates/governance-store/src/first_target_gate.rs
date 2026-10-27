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
//!
//! # Which application a group ran at a cut
//!
//! [`applications_at_cut`] asks the same ancestry a wider question for the
//! device-link scope check: not whether the group had chosen, but what it
//! could have been running. A link scoped to some applications reaches a group
//! only if every application the group may have targeted at the link's cut is
//! one of them, so whether a replica folded a concurrent `TargetApplicationSet`
//! before or after the link no longer decides whether the device is bound
//! there (core#4603).

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
use calimero_primitives::application::{ApplicationId, ZERO_APPLICATION_ID};
use calimero_store::Store;
use eyre::Result as EyreResult;

use crate::authorizer::{ensure_cut_resolvable, AtCutAuthorizer};
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
            if !parents.is_empty() {
                ensure_cut_resolvable(
                    authorizer,
                    group_id,
                    parents,
                    "the group's application at the op's cut",
                )?;
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

/// The most op visits one [`applications_at_cut`] walk makes, counting the
/// walks into the groups a subgroup was created under. A link's walk reads
/// every op since the group's newest target choice, and the history behind
/// that choice, so a namespace with a longer history answers
/// [`ApplicationsAtCut::Unknown`] and the link falls back as an unreadable
/// ancestry does.
pub(crate) const MAX_TARGET_WALK_VISITS: usize = 16_384;

/// The applications a group may have been targeting at a cut.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ApplicationsAtCut {
    /// Every application some peer that folded exactly the cut could read as
    /// the group's target. [`ZERO_APPLICATION_ID`] stands for "none chosen".
    /// More than one when target choices are concurrent with each other, or a
    /// `CascadeUpgrade` may or may not have matched the group.
    Known(BTreeSet<ApplicationId>),
    /// Part of the ancestry is not readable here, or it is longer than
    /// [`MAX_TARGET_WALK_VISITS`].
    Unknown,
}

/// The applications `group_id` may have been targeting at the cut named by
/// `parents`, read from the cut's ancestry in the namespace op log.
///
/// What writes a group's target: a `TargetApplicationSet` on the group, which
/// replaces everything before it; the group's own `GroupCreated`, which copies
/// its parent's target at that creation's cut; the namespace genesis, which
/// starts the root at none; and a `CascadeUpgrade` signed on the group or on a
/// group above it, which moves the group only if its bytecode matched when the
/// cascade applied. That is read from live rows the ancestry does not record,
/// so a cascade adds its application without taking away the ones before it.
/// A writer that is an ancestor of a replacing writer in the cut is out of
/// force and is dropped.
///
/// Which groups are above `group_id` is read from the live parent edges, as
/// the cascade's own apply reads its descendants.
///
/// # Errors
/// A store failure.
pub(crate) fn applications_at_cut(
    store: &Store,
    group_id: &ContextGroupId,
    parents: &[[u8; 32]],
) -> EyreResult<ApplicationsAtCut> {
    if parents.is_empty() {
        return Ok(ApplicationsAtCut::Unknown);
    }
    let namespace = NamespaceRepository::new(store).resolve(group_id)?;
    let walk = AncestryWalk {
        store,
        namespace: NamespaceId::from(namespace.to_bytes()),
    };
    let mut budget = MAX_TARGET_WALK_VISITS;
    walk.applications_at(*group_id, parents, &mut budget)
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
        let key = GroupKeyring::load_covering_key_by_id(self.store, &group, key_id)?;
        Ok(key.and_then(|key| GroupKeyring::decrypt_op(&key, encrypted).ok()))
    }
}

/// What one op in the ancestry wrote to the target of the group being asked
/// after.
enum Write {
    Nothing,
    /// Replaced the target with this application.
    Set(ApplicationId),
    /// May have moved the target to this application.
    Maybe(ApplicationId),
    /// Created the group under this parent, copying the parent's target.
    CreatedUnder(ContextGroupId),
    /// Concerns the group but cannot be read here.
    Unreadable,
}

/// An op in the ancestry that wrote the group's target.
struct Writer {
    hash: [u8; 32],
    applications: BTreeSet<ApplicationId>,
}

impl AncestryWalk<'_> {
    /// The applications `group` may have targeted at `parents`, drawing on
    /// `budget` for every op visited.
    ///
    /// One pass over the ancestry. An op is reached either "in force" (no
    /// replacing writer lies between it and the cut) or "shadowed" (one does);
    /// a shadowed op only passes the shadow on to its parents, and one reached
    /// both ways is shadowed. The answer is the writers still in force.
    fn applications_at(
        &self,
        group: ContextGroupId,
        parents: &[[u8; 32]],
        budget: &mut usize,
    ) -> EyreResult<ApplicationsAtCut> {
        let op_log = NamespaceOpLogService::new(self.store, self.namespace);
        let above = self.groups_above(group)?;
        let mut shadowed: HashMap<[u8; 32], bool> = HashMap::new();
        let mut queue: VecDeque<([u8; 32], bool)> =
            parents.iter().map(|hash| (*hash, false)).collect();
        let mut writers: Vec<Writer> = Vec::new();
        while let Some((hash, shadow)) = queue.pop_front() {
            match shadowed.get(&hash) {
                Some(true) => continue,
                Some(false) if !shadow => continue,
                _ => {}
            }
            let _previous = shadowed.insert(hash, shadow);
            let Some(left) = budget.checked_sub(1) else {
                return Ok(ApplicationsAtCut::Unknown);
            };
            *budget = left;
            // Absent or undecodable: ancestry this node cannot vouch for, and
            // a shadow that stops here could leave a writer counted that the
            // cut has replaced.
            let Ok(Some(op)) = op_log.get_signed_op(hash) else {
                return Ok(ApplicationsAtCut::Unknown);
            };
            if shadow {
                queue.extend(op.parent_op_hashes.iter().map(|parent| (*parent, true)));
                continue;
            }
            let replaced = match self.write(group, &above, &op)? {
                Write::Unreadable => return Ok(ApplicationsAtCut::Unknown),
                Write::Set(application) => Some(BTreeSet::from([application])),
                Write::CreatedUnder(parent) => {
                    match self.applications_at(parent, &op.parent_op_hashes, budget)? {
                        ApplicationsAtCut::Known(applications) => Some(applications),
                        ApplicationsAtCut::Unknown => return Ok(ApplicationsAtCut::Unknown),
                    }
                }
                // The genesis: the root starts with no application.
                Write::Nothing if op.parent_op_hashes.is_empty() => {
                    Some(BTreeSet::from([ZERO_APPLICATION_ID]))
                }
                Write::Maybe(application) => {
                    writers.push(Writer {
                        hash,
                        applications: BTreeSet::from([application]),
                    });
                    None
                }
                Write::Nothing => None,
            };
            let shadow_parents = replaced.is_some();
            if let Some(applications) = replaced {
                writers.push(Writer { hash, applications });
            }
            queue.extend(
                op.parent_op_hashes
                    .iter()
                    .map(|parent| (*parent, shadow_parents)),
            );
        }
        Ok(ApplicationsAtCut::Known(
            writers
                .into_iter()
                .filter(|writer| shadowed.get(&writer.hash) == Some(&false))
                .flat_map(|writer| writer.applications)
                .collect(),
        ))
    }

    /// `group` and every group above it, by the live parent edges.
    fn groups_above(&self, group: ContextGroupId) -> EyreResult<HashSet<ContextGroupId>> {
        let namespaces = NamespaceRepository::new(self.store);
        let mut above = HashSet::from([group]);
        let mut current = group;
        while let Some(parent) = namespaces.parent(&current)? {
            if !above.insert(parent) {
                break;
            }
            current = parent;
        }
        Ok(above)
    }

    fn write(
        &self,
        group: ContextGroupId,
        above: &HashSet<ContextGroupId>,
        op: &SignedNamespaceOp,
    ) -> EyreResult<Write> {
        let creation = |finding| match finding {
            Finding::CreatedUnder(parent) => Write::CreatedUnder(parent),
            _ => Write::Nothing,
        };
        let is_root = group.to_bytes() == self.namespace.to_bytes();
        match &op.op {
            // No root op creates the namespace root, and none writes a target.
            NamespaceOp::Root(_) | NamespaceOp::RootSealed { .. } if is_root => Ok(Write::Nothing),
            NamespaceOp::Root(root) => Ok(creation(created(group, root))),
            NamespaceOp::RootSealed { key_id, encrypted } => {
                match crate::namespace::open_sealed_root_op(
                    self.store,
                    self.namespace,
                    key_id.as_bytes(),
                    encrypted,
                ) {
                    Ok(Some(root)) => Ok(creation(created(group, &root))),
                    Ok(None) | Err(_) => Ok(Write::Unreadable),
                }
            }
            NamespaceOp::Group {
                group_id,
                key_id,
                encrypted,
                ..
            } => {
                // Only an op on this group or a group above it can write this
                // group's target, so nothing else needs opening.
                if !above.contains(group_id) {
                    return Ok(Write::Nothing);
                }
                let Some(inner) = self.open_group_op(*group_id, key_id.as_bytes(), encrypted)?
                else {
                    return Ok(Write::Unreadable);
                };
                Ok(match unwrap_on_behalf(&inner) {
                    GroupOp::TargetApplicationSet {
                        target_application_id,
                        ..
                    } if *group_id == group => Write::Set(*target_application_id),
                    GroupOp::CascadeUpgrade {
                        target_application_id,
                        ..
                    } => Write::Maybe(*target_application_id),
                    _ => Write::Nothing,
                })
            }
            // The remaining envelopes carry joins, which set no application.
            _ => Ok(Write::Nothing),
        }
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
        AccountGenesis, AccountId, AccountMemberEndorsement, DeviceCert, DeviceId,
        GovernanceDelegation, GovernanceOpKind, GovernanceTerms, GovernanceWarrant, KemPublicKey,
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
        account_for, derived_group_id, enrol_member, real_join_account, test_store, STAMPED_AT,
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
            stamped_at: STAMPED_AT,
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
                            executor_key: self.relay_sk.public_key(),
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
                .apply_signed_op(&signed)?
                .verdict()?;
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
                    registers: op.registered_context(),
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
                stamped_at: STAMPED_AT,
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
            stamped_at: STAMPED_AT,
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

    /// A `TargetApplicationSet` on `group` choosing `application`.
    fn target(application: [u8; 32]) -> GroupOp {
        GroupOp::TargetApplicationSet {
            bytecode_id: BytecodeId::from(BUNDLE),
            target_application_id: ApplicationId::from(application),
            package: "com.example.app".to_owned(),
            version: "1.0.0".to_owned(),
        }
    }

    const APP_B: [u8; 32] = [0x89; 32];
    const DEVICE_SEED: u8 = 0x5D;

    impl Rig {
        /// A link of a fresh account's device into the root, endorsed by
        /// the founder, its scope naming `applications`.
        fn link(&self, applications: &[[u8; 32]]) -> GroupOp {
            let owner_sk = PrivateKey::from([DEVICE_SEED; 32]);
            let genesis = AccountGenesis::new(owner_sk.public_key());
            let account = genesis.account_id();
            let device = DeviceId::mint(account, [DEVICE_SEED; 16]);
            let cert = DeviceCert::sign(
                &owner_sk,
                account,
                device,
                &owner_sk.public_key(),
                &KemPublicKey::from([DEVICE_SEED; 32]),
                0,
                0,
            )
            .expect("cert");
            GroupOp::AccountDeviceLinked {
                genesis,
                chain: vec![],
                cert,
                endorsement: AccountMemberEndorsement::sign(&self.founder_sk, account)
                    .expect("endorse"),
                scope: Box::new(crate::test_fixtures::device_scope(
                    &owner_sk,
                    &cert,
                    applications
                        .iter()
                        .copied()
                        .map(ApplicationId::from)
                        .collect(),
                    0,
                )),
            }
        }

        fn linked_device_bound(&self) -> bool {
            let account =
                AccountGenesis::new(PrivateKey::from([DEVICE_SEED; 32]).public_key()).account_id();
            crate::AccountBindingRepository::new(&self.store)
                .live_bindings(&self.ns)
                .expect("read")
                .iter()
                .any(|binding| binding.account == account)
        }

        fn applications_at(&self, parents: &[[u8; 32]]) -> super::ApplicationsAtCut {
            super::applications_at_cut(&self.store, &self.ns, parents).expect("walk")
        }
    }

    fn known(applications: &[[u8; 32]]) -> super::ApplicationsAtCut {
        super::ApplicationsAtCut::Known(
            applications
                .iter()
                .copied()
                .map(ApplicationId::from)
                .collect(),
        )
    }

    /// core#4603: a link made while the root targets A, scoped to A, races a
    /// `TargetApplicationSet` to B. Both replicas bind the device, whichever of
    /// the two ops each folds first, because the link is judged at its own cut.
    #[test]
    fn a_link_racing_a_target_change_binds_on_either_side_of_it() {
        let mut bound = Vec::new();
        for target_first in [true, false] {
            let r = rig();
            let at_a = r
                .group_op(&r.founder_sk, r.ns, vec![r.genesis], &target(APP))
                .expect("the root targets A");
            let link = r.link(&[APP]);
            if target_first {
                let _b = r
                    .group_op(&r.founder_sk, r.ns, vec![at_a], &target(APP_B))
                    .expect("the root moves to B");
                assert_eq!(r.target(&r.ns), ApplicationId::from(APP_B), "live reads B");
                let _link = r
                    .group_op(&r.founder_sk, r.ns, vec![at_a], &link)
                    .expect("the link applies");
            } else {
                let _link = r
                    .group_op(&r.founder_sk, r.ns, vec![at_a], &link)
                    .expect("the link applies");
                let _b = r
                    .group_op(&r.founder_sk, r.ns, vec![at_a], &target(APP_B))
                    .expect("the root moves to B");
            }
            bound.push(r.linked_device_bound());
        }
        assert_eq!(
            bound,
            [true, true],
            "the same signed link reaches the same verdict on both replicas"
        );
    }

    /// The same race, but the link's scope does not name the target in force at
    /// its cut: refused on both replicas, even the one whose live target the
    /// scope names.
    #[test]
    fn a_link_scoped_to_the_next_target_is_refused_on_either_side_of_it() {
        let mut bound = Vec::new();
        for target_first in [true, false] {
            let r = rig();
            let at_a = r
                .group_op(&r.founder_sk, r.ns, vec![r.genesis], &target(APP))
                .expect("the root targets A");
            let link = r.link(&[APP_B]);
            let move_to_b = |r: &Rig| {
                let _b = r
                    .group_op(&r.founder_sk, r.ns, vec![at_a], &target(APP_B))
                    .expect("the root moves to B");
            };
            if target_first {
                move_to_b(&r);
            }
            let _link = r
                .group_op(&r.founder_sk, r.ns, vec![at_a], &link)
                .expect("a refused link is still logged");
            if !target_first {
                move_to_b(&r);
            }
            bound.push(r.linked_device_bound());
        }
        assert_eq!(bound, [false, false]);
    }

    /// Two target choices concurrent with each other are both in force at a cut
    /// holding both: a link there binds only if its scope names both.
    #[test]
    fn concurrent_targets_at_a_cut_must_all_be_in_scope() {
        let r = rig();
        let a = r
            .group_op(&r.founder_sk, r.ns, vec![r.genesis], &target(APP))
            .expect("A");
        let b = r
            .group_op(&r.founder_sk, r.ns, vec![r.genesis], &target(APP_B))
            .expect("B, concurrent with A");
        assert_eq!(r.applications_at(&[a, b]), known(&[APP, APP_B]));

        let _narrow = r
            .group_op(&r.founder_sk, r.ns, vec![a, b], &r.link(&[APP]))
            .expect("logged");
        assert!(!r.linked_device_bound(), "B may be the target at the cut");
        let r = rig();
        let a = r
            .group_op(&r.founder_sk, r.ns, vec![r.genesis], &target(APP))
            .expect("A");
        let b = r
            .group_op(&r.founder_sk, r.ns, vec![r.genesis], &target(APP_B))
            .expect("B");
        let _wide = r
            .group_op(&r.founder_sk, r.ns, vec![a, b], &r.link(&[APP, APP_B]))
            .expect("logged");
        assert!(
            r.linked_device_bound(),
            "a scope naming both reaches the group"
        );
    }

    /// A choice the cut has replaced is out of force, even where another head
    /// of the cut reaches it by a path that does not pass the replacement.
    #[test]
    fn a_replaced_target_is_out_of_force_at_the_cut() {
        let r = rig();
        let a = r
            .group_op(&r.founder_sk, r.ns, vec![r.genesis], &target(APP))
            .expect("A");
        let b = r
            .group_op(&r.founder_sk, r.ns, vec![a], &target(APP_B))
            .expect("B replaces A");
        let (_, beside) = r.subgroup(0xE3, vec![a]);
        assert_eq!(r.applications_at(&[b, beside]), known(&[APP_B]));
        assert_eq!(r.applications_at(&[beside]), known(&[APP]));
        assert_eq!(r.applications_at(&[r.genesis]), known(&[[0u8; 32]]));
    }

    /// A subgroup runs what its parent ran at its creation's cut, until it
    /// chooses for itself.
    #[test]
    fn a_subgroup_reads_its_parents_target_at_its_creation() {
        let r = rig();
        let a = r
            .group_op(&r.founder_sk, r.ns, vec![r.genesis], &target(APP))
            .expect("A");
        let (sub, created) = r.subgroup(0xE4, vec![a]);
        let sub = ContextGroupId::from(sub);
        let _b = r
            .group_op(&r.founder_sk, r.ns, vec![created], &target(APP_B))
            .expect("the root moves on");
        let at = |parents: &[[u8; 32]]| {
            super::applications_at_cut(&r.store, &sub, parents).expect("walk")
        };
        assert_eq!(at(&[created]), known(&[APP]));
        let own = r
            .group_op(&r.founder_sk, sub, vec![created], &target(APP_B))
            .expect("the subgroup chooses");
        assert_eq!(at(&[own]), known(&[APP_B]));
    }

    /// A cascade moves the group only if its bytecode matched when it applied,
    /// which the ancestry does not record: it adds its application to the ones
    /// in force before it, and a later choice replaces both.
    #[test]
    fn a_cascade_adds_its_application_until_a_choice_replaces_it() {
        let r = rig();
        let a = r
            .group_op(&r.founder_sk, r.ns, vec![r.genesis], &target(APP))
            .expect("A");
        let cascade = r
            .group_op(
                &r.founder_sk,
                r.ns,
                vec![a],
                &GroupOp::CascadeUpgrade {
                    from_bytecode_id: BytecodeId::from(BUNDLE),
                    bytecode_id: BytecodeId::from([0x78; 32]),
                    target_application_id: ApplicationId::from(APP_B),
                    to_state_version: 0,
                    migration: None,
                    cascade_hlc: calimero_storage::logical_clock::HybridTimestamp::zero(),
                    package: "com.example.app".to_owned(),
                    version: "2.0.0".to_owned(),
                },
            )
            .expect("the root cascades to B");
        assert_eq!(r.applications_at(&[cascade]), known(&[APP, APP_B]));

        let c = r
            .group_op(&r.founder_sk, r.ns, vec![cascade], &target([0x8A; 32]))
            .expect("C replaces both");
        assert_eq!(r.applications_at(&[c]), known(&[[0x8A; 32]]));
    }

    /// An ancestry with an op this node does not hold is not "no target".
    #[test]
    fn a_missing_ancestor_leaves_the_target_unknown() {
        let r = rig();
        assert_eq!(
            r.applications_at(&[[0xAB; 32]]),
            super::ApplicationsAtCut::Unknown
        );
    }
}
