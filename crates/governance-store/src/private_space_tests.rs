//! Structure rules of private spaces, judged where each op is applied.

use std::cell::Cell;

use calimero_account::AccountId;
use calimero_context_client::local_governance::{
    GroupOp, RootOp, SignedGroupOp, SignedNamespaceOp,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::VisibilityMode;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;

use crate::namespace::NamespaceGovernance;
use crate::test_fixtures::{
    bootstrap_namespace_with_admin_account, derived_group_id, enrol_member, group_created,
    seal_for_test, test_store,
};
use crate::{
    apply_local_signed_group_op, CapabilitiesRepository, MetaRepository, NamespaceRepository,
};

const NS: [u8; 32] = [0xB1; 32];

struct Space {
    store: Store,
    sk: PrivateKey,
    admin: AccountId,
    nonce: Cell<u64>,
    tag: Cell<u8>,
}

impl Space {
    fn new() -> Self {
        let store = test_store();
        let ((sk, _), admin) = bootstrap_namespace_with_admin_account(&store, NS);
        Self {
            store,
            sk,
            admin,
            nonce: Cell::new(0),
            tag: Cell::new(0),
        }
    }

    fn next_nonce(&self) -> u64 {
        self.nonce.set(self.nonce.get() + 1);
        self.nonce.get()
    }

    fn root_op(&self, op: RootOp) -> EyreResult<()> {
        let ns = ContextGroupId::from(NS);
        let signed = SignedNamespaceOp::sign(
            &self.sk,
            NS.into(),
            vec![],
            self.next_nonce(),
            seal_for_test(&self.store, ns, op),
        )?;
        NamespaceGovernance::new(&self.store, NS.into())
            .apply_signed_op(&signed)
            .map(|_| ())
    }

    fn group_op(&self, group: [u8; 32], op: GroupOp) -> EyreResult<()> {
        let signed = SignedGroupOp::sign(&self.sk, group.into(), vec![], self.next_nonce(), op)?;
        apply_local_signed_group_op(&self.store, &signed)
    }

    fn create(&self, parent: [u8; 32], restricted: bool) -> EyreResult<[u8; 32]> {
        self.tag.set(self.tag.get() + 1);
        let tag = self.tag.get();
        self.root_op(group_created(self.admin, parent, restricted, tag))?;
        Ok(derived_group_id(&self.admin, parent, restricted, tag))
    }

    fn open(&self, parent: [u8; 32]) -> [u8; 32] {
        self.create(parent, false).expect("create an open folder")
    }

    fn private(&self, parent: [u8; 32]) -> [u8; 32] {
        self.create(parent, true).expect("create a private space")
    }

    fn reparent(&self, child: [u8; 32], new_parent: [u8; 32]) -> EyreResult<()> {
        self.root_op(RootOp::GroupReparented {
            child_group_id: child.into(),
            new_parent_id: new_parent.into(),
        })
    }

    fn delete(&self, group: [u8; 32]) -> EyreResult<()> {
        let payload = NamespaceRepository::new(&self.store)
            .collect_subtree_for_cascade(&group.into())
            .expect("collect the subtree");
        self.root_op(RootOp::GroupDeleted {
            root_group_id: group.into(),
            cascade_group_ids: payload
                .descendant_groups
                .iter()
                .map(|g| g.to_bytes().into())
                .collect(),
            cascade_context_ids: vec![],
        })
    }

    fn set_visibility(&self, group: [u8; 32], mode: VisibilityMode) -> EyreResult<()> {
        self.group_op(group, GroupOp::SubgroupVisibilitySet { mode })
    }

    fn add_member(&self, group: [u8; 32]) -> EyreResult<()> {
        let pk =
            PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng)).public_key();
        let member = enrol_member(&self.store, &ContextGroupId::from(NS), &pk);
        self.group_op(
            group,
            GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            },
        )
    }

    fn parent(&self, group: [u8; 32]) -> Option<[u8; 32]> {
        NamespaceRepository::new(&self.store)
            .parent(&group.into())
            .unwrap()
            .map(|p| p.to_bytes())
    }

    fn exists(&self, group: [u8; 32]) -> bool {
        MetaRepository::new(&self.store)
            .load(&group.into())
            .unwrap()
            .is_some()
    }
}

#[test]
fn a_private_space_may_sit_in_the_open_tree() {
    let s = Space::new();
    let folder = s.open(NS);
    let space = s.private(folder);
    assert_eq!(s.parent(space), Some(folder));
    s.open(space);
}

#[test]
fn a_private_space_cannot_be_created_inside_a_private_space() {
    let s = Space::new();
    let space = s.private(NS);
    let inner_folder = s.open(space);

    assert!(s.create(space, true).is_err(), "directly under a space");
    assert!(s.create(inner_folder, true).is_err(), "deeper in a space");
    assert!(s.create(space, false).is_ok(), "an open folder is fine");
}

#[test]
fn making_a_folder_private_is_refused_inside_a_space() {
    let s = Space::new();
    let space = s.private(NS);
    let inner = s.open(space);

    assert!(s.set_visibility(inner, VisibilityMode::Restricted).is_err());
}

#[test]
fn making_a_folder_private_is_refused_when_its_subtree_holds_a_space() {
    let s = Space::new();
    let folder = s.open(NS);
    let _space = s.private(folder);

    assert!(s
        .set_visibility(folder, VisibilityMode::Restricted)
        .is_err());
}

#[test]
fn making_an_open_folder_private_is_allowed_in_the_open_tree() {
    let s = Space::new();
    let folder = s.open(NS);
    s.open(folder);

    s.set_visibility(folder, VisibilityMode::Restricted)
        .expect("a clean open subtree may become a space");
    assert_eq!(
        CapabilitiesRepository::new(&s.store)
            .subgroup_visibility(&folder.into())
            .unwrap(),
        VisibilityMode::Restricted
    );
}

#[test]
fn a_space_cannot_be_moved_under_a_space() {
    let s = Space::new();
    let a = s.private(NS);
    let b = s.private(NS);

    assert!(s.reparent(b, a).is_err());
    assert_eq!(s.parent(b), Some(NS));
}

#[test]
fn a_folder_holding_a_space_cannot_be_moved_into_a_space() {
    let s = Space::new();
    let space = s.private(NS);
    let folder = s.open(NS);
    let _nested = s.private(folder);

    assert!(s.reparent(folder, space).is_err());
}

#[test]
fn a_folder_cannot_leave_a_space() {
    let s = Space::new();
    let space = s.private(NS);
    let inner = s.open(space);
    let outside = s.open(NS);

    assert!(s.reparent(inner, outside).is_err(), "out to an open folder");
    assert!(s.reparent(inner, NS).is_err(), "out to the root");
    assert_eq!(s.parent(inner), Some(space));
}

#[test]
fn folders_move_into_a_space_and_within_it() {
    let s = Space::new();
    let space = s.private(NS);
    let inner_a = s.open(space);
    let inner_b = s.open(space);
    let outside = s.open(NS);

    s.reparent(outside, space).expect("into a space");
    s.reparent(inner_a, inner_b).expect("within a space");

    let folder = s.open(NS);
    let other = s.private(folder);
    s.reparent(other, NS)
        .expect("a space moves in the open tree");
}

#[test]
fn a_group_holding_a_nested_space_cannot_be_deleted() {
    let s = Space::new();
    let folder = s.open(NS);
    let space = s.private(folder);

    assert!(s.delete(folder).is_err());
    assert!(s.exists(folder) && s.exists(space));
    s.delete(space).expect("the space itself can be deleted");
    s.delete(folder).expect("and then the empty folder");
}

#[test]
fn a_space_with_only_open_folders_can_be_deleted() {
    let s = Space::new();
    let space = s.private(NS);
    s.open(space);

    s.delete(space).expect("delete the space and its folders");
    assert!(!s.exists(space));
}

#[test]
fn members_are_added_at_the_root_or_a_space_root() {
    let s = Space::new();
    let folder = s.open(NS);
    let space = s.private(NS);
    let inner = s.open(space);

    assert!(s.add_member(folder).is_err(), "an open folder");
    assert!(s.add_member(inner).is_err(), "a folder in a space");
    s.add_member(NS).expect("the workspace root");
    s.add_member(space).expect("a space root");
}
