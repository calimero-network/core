//! The shared climb, pinned against the three hand-written walks it replaced.
//!
//! `reference_is_member_at_cut` below is the **pre-refactor** `is_member_at_cut`
//! body, transcribed verbatim: its own loop, its own bound, its own wall check,
//! its own admin carve-out. Asserting the crate against it over every small tree
//! is what makes the refactor a behaviour-preserving one rather than a claim —
//! and it keeps a second, independent implementation of the rule in the tree, so
//! a future change to `open_ancestors` that alters membership cannot pass
//! silently.
//!
//! Kept deliberately as a duplicate: the value of an oracle is that it does NOT
//! share code with the thing it checks.

use std::collections::BTreeMap;

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_op::ScopeId;
use calimero_primitives::context::GroupMemberRole;

use crate::inheritance::{MemberPathAtCut, CAN_JOIN_OPEN_SUBGROUPS};
use crate::view::{AclView, SubgroupEdge};

const MAX_NAMESPACE_DEPTH: usize = calimero_context_config::MAX_NAMESPACE_DEPTH;

/// The pre-refactor `is_member_at_cut`, transcribed. Do not refactor to share
/// code with the crate — that would defeat its purpose.
fn reference_is_member_at_cut(
    view: &AclView,
    group: ContextGroupId,
    author: &AccountId,
    root: Option<(ContextGroupId, AccountId)>,
    default_cap_base: u32,
) -> bool {
    let is_admin = |g: ContextGroupId| -> bool {
        view.is_group_admin(author, g)
            || root.is_some_and(|(root_g, root_admin)| g == root_g && *author == root_admin)
    };

    if is_admin(group)
        || view
            .groups
            .get(&group)
            .is_some_and(|m| m.contains_key(author))
    {
        return true;
    }

    let effective_cap = |g: &ContextGroupId| -> u32 {
        let folded = view.capability(g, author);
        if folded != 0 {
            folded
        } else {
            default_cap_base
        }
    };

    let mut anchor_is_member: Option<bool> = None;
    let mut current = group;
    for _ in 0..=MAX_NAMESPACE_DEPTH {
        let Some(edge) = view.subgroups.get(&ScopeId::from(current.to_bytes())) else {
            return anchor_is_member.unwrap_or(false);
        };
        if edge.restricted {
            return anchor_is_member.unwrap_or(false);
        }
        let parent = ContextGroupId::from(*edge.parent.as_bytes());
        if is_admin(parent) {
            return true;
        }
        if anchor_is_member.is_none()
            && view
                .groups
                .get(&parent)
                .is_some_and(|m| m.contains_key(author))
        {
            anchor_is_member = Some(effective_cap(&parent) & CAN_JOIN_OPEN_SUBGROUPS != 0);
        }
        current = parent;
    }
    anchor_is_member.unwrap_or(false)
}

/// The pre-refactor `is_authorized_admin`, transcribed, for the same reason.
fn reference_is_authorized_admin(
    view: &AclView,
    group: ContextGroupId,
    author: &AccountId,
    root: Option<(ContextGroupId, AccountId)>,
) -> bool {
    let is_admin = |g: ContextGroupId| -> bool {
        view.is_group_admin(author, g)
            || view.is_root_admin(author)
            || root.is_some_and(|(root_g, root_admin)| g == root_g && *author == root_admin)
    };
    if is_admin(group) {
        return true;
    }
    let mut current = group;
    for _ in 0..=MAX_NAMESPACE_DEPTH {
        let Some(edge) = view.subgroups.get(&ScopeId::from(current.to_bytes())) else {
            return false;
        };
        if edge.restricted {
            return false;
        }
        let parent = ContextGroupId::from(*edge.parent.as_bytes());
        if is_admin(parent) {
            return true;
        }
        current = parent;
    }
    false
}

fn g(n: u8) -> ContextGroupId {
    ContextGroupId::from([n; 32])
}

/// One point in the sweep: a chain `g(0) → g(1) → … → g(depth)` plus the flags
/// that vary across it.
///
/// A struct rather than eight parameters, for the same reason the crate under
/// test stopped passing its view a field at a time — and because a sweep whose
/// axes are positional `u32`s is one transposed argument away from silently
/// testing something else.
#[derive(Clone, Copy)]
struct TreeSpec {
    depth: usize,
    /// Bit `i` set = `g(i)` holds a direct membership row for the author.
    rows: u32,
    /// Bit `i` set = the edge from `g(i)` to its parent is Restricted.
    restricted: u32,
    /// Bit `i` set = the author is the folded group admin of `g(i)`.
    admins: u32,
    /// The author's folded per-member capability at every group.
    cap: u32,
    /// Whether the view's global `root_admin` is the author.
    root_admin_is_author: bool,
}

impl TreeSpec {
    fn groups(self) -> usize {
        self.depth + 1
    }
}

/// Build the synthetic view `spec` describes.
fn chain_view(spec: TreeSpec, author: AccountId, other: AccountId) -> AclView {
    let TreeSpec {
        rows,
        restricted,
        admins,
        cap,
        root_admin_is_author,
        ..
    } = spec;
    let n = spec.groups();
    let mut groups: BTreeMap<ContextGroupId, BTreeMap<AccountId, GroupMemberRole>> =
        BTreeMap::new();
    let mut subgroups = BTreeMap::new();
    let mut group_admin = BTreeMap::new();
    let mut member_caps = BTreeMap::new();

    for i in 0..n {
        let gi = g(u8::try_from(i).expect("small"));
        let holder = if rows & (1 << i) == 0 { other } else { author };
        let _ = groups.insert(
            gi,
            [(holder, GroupMemberRole::Member)].into_iter().collect(),
        );
        if admins & (1 << i) != 0 {
            let _ = group_admin.insert(gi, author);
        }
        let _ = member_caps.insert((gi, author), cap);
        if i + 1 < n {
            let _ = subgroups.insert(
                ScopeId::from(gi.to_bytes()),
                SubgroupEdge {
                    parent: ScopeId::from(g(u8::try_from(i + 1).expect("small")).to_bytes()),
                    restricted: restricted & (1 << i) != 0,
                },
            );
        }
    }

    AclView {
        groups,
        subgroups,
        group_admin,
        member_caps,
        root_admin: root_admin_is_author.then_some(author),
        ..Default::default()
    }
}

/// What one swept tree hands to a check: the view, who is asking, the un-folded
/// `root` carve-out, and the materialized default capability.
struct Case {
    view: AclView,
    author: AccountId,
    root: Option<(ContextGroupId, AccountId)>,
    base_cap: u32,
}

/// Sweep every small tree and hand each one to `check`.
fn for_every_small_tree(mut check: impl FnMut(&Case)) {
    let author = AccountId::from([0xAA; 32]);
    let other = AccountId::from([0xBB; 32]);
    for depth in 0usize..=3 {
        let n = depth + 1;
        for rows in 0u32..(1 << n) {
            for restricted in 0u32..(1 << n) {
                for admins in 0u32..(1 << n) {
                    for cap in [0, CAN_JOIN_OPEN_SUBGROUPS] {
                        for root_carve in [false, true] {
                            for root_admin_is_author in [false, true] {
                                for base_cap in [0, CAN_JOIN_OPEN_SUBGROUPS] {
                                    let spec = TreeSpec {
                                        depth,
                                        rows,
                                        restricted,
                                        admins,
                                        cap,
                                        root_admin_is_author,
                                    };
                                    check(&Case {
                                        view: chain_view(spec, author, other),
                                        author,
                                        root: root_carve.then_some((
                                            g(u8::try_from(n - 1).expect("small")),
                                            author,
                                        )),
                                        base_cap,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn is_member_matches_the_walk_it_replaced() {
    let mut checked = 0usize;
    for_every_small_tree(|case| {
        let Case {
            view,
            author,
            root,
            base_cap,
        } = case;
        assert_eq!(
            view.is_member_at_cut(g(0), author, *root, *base_cap),
            reference_is_member_at_cut(view, g(0), author, *root, *base_cap),
            "membership diverged from the pre-refactor walk on {view:?} (root={root:?}, \
             base_cap={base_cap})"
        );
        checked += 1;
    });
    assert!(
        checked > 10_000,
        "the sweep must actually be wide: {checked}"
    );
}

#[test]
fn is_member_is_the_path_walk_with_the_role_discarded() {
    // The equivalence `calimero-context` depends on: it filters candidates with
    // `is_member_at_cut` and resolves each survivor's role with
    // `member_path_at_cut`. A disagreement would emit a member with no role, or a
    // role for a non-member.
    for_every_small_tree(|case| {
        assert_eq!(
            case.view
                .is_member_at_cut(g(0), &case.author, case.root, case.base_cap),
            !matches!(
                case.view
                    .member_path_at_cut(g(0), &case.author, case.root, case.base_cap),
                MemberPathAtCut::None
            ),
        );
    });
}

#[test]
fn admin_authority_matches_the_walk_it_replaced() {
    for_every_small_tree(|case| {
        let Case {
            view, author, root, ..
        } = case;
        assert_eq!(
            view.is_authorized_admin(g(0), author, *root),
            reference_is_authorized_admin(view, g(0), author, *root),
            "admin authority diverged from the pre-refactor walk on {view:?} (root={root:?})"
        );
    });
}

#[test]
fn a_restricted_edge_walls_off_an_admin_further_up() {
    // Pinned separately from the sweep because it is the rule most likely to be
    // "fixed" by someone who reads the climb and assumes authority should reach
    // through: it must not.
    let author = AccountId::from([0xAA; 32]);
    let mut view = chain_view(
        TreeSpec {
            depth: 2,
            rows: 0,
            restricted: 0b010,
            admins: 0b100,
            cap: 0,
            root_admin_is_author: false,
        },
        author,
        AccountId::from([0xBB; 32]),
    );
    assert!(
        !view.is_authorized_admin(g(0), &author, None),
        "an admin above a restricted edge must not administer through it"
    );
    // Open the wall and the same admin now reaches down.
    let _ = view.subgroups.insert(
        ScopeId::from(g(1).to_bytes()),
        SubgroupEdge {
            parent: ScopeId::from(g(2).to_bytes()),
            restricted: false,
        },
    );
    assert!(view.is_authorized_admin(g(0), &author, None));
}
