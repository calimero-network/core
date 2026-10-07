#![no_main]
//! Signed group ops applied through `NamespaceGovernance`. The input is a short list of ops
//! over a fixed cast, each with its own parents, delivered in a chosen order.

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_governance_store::test_fixtures::{
    enrol_local_device, enrol_member, sample_meta_with_admin, test_store,
};
use calimero_governance_store::{
    CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository,
    NamespaceGovernance, NamespaceRepository,
};
use calimero_governance_types::{GroupOp, KeyId, NamespaceId, NamespaceOp, SignedNamespaceOp};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use libfuzzer_sys::fuzz_target;

const NS: [u8; 32] = [0xB8; 32]; // the namespace and its root group
const GROUP_KEY: [u8; 32] = [0x97; 32]; // seals every group op
const LOCAL_SEED: [u8; 32] = [5; 32]; // the node's own device
const SEEDS: [[u8; 32]; 4] = [[1; 32], [2; 32], [3; 32], [4; 32]]; // fixed so a crash replays
const OWNER: usize = 0;
const ADMIN: usize = 1;
const MEMBER: usize = 2;
const OUTSIDER: usize = 3;
const MAX_OPS: usize = 16; // ops built from one input
const RECORD_LEN: usize = 6; // actor, kind, target, arg, parents, delivery
const PARENT_WINDOW: usize = 8; // a parents bit names one of the last ops built

/// What the apply path answers about the cast and the group.
#[derive(Debug, PartialEq)]
struct Answers {
    roles: [Option<GroupMemberRole>; 4],
    capabilities: [Option<u32>; 4],
    default_capabilities: Option<u32>,
    admins: usize,
}

struct Cast {
    store: Store,
    group: ContextGroupId,
    keys: [PrivateKey; 4],
    accounts: [AccountId; 4],
    nonces: [u64; 4],
}

fuzz_target!(|data: &[u8]| {
    let mut cast = Cast::new();
    let mut hashes = Vec::new();
    for (actor, member, role) in [
        (OWNER, ADMIN, GroupMemberRole::Admin),
        (OWNER, MEMBER, GroupMemberRole::Member),
    ] {
        let account = cast.accounts[member];
        let op = cast.sign(
            actor,
            &hashes,
            GroupOp::MemberAdded {
                member: account,
                role,
            },
        );
        apply(&cast.store, &op).expect("the owner adds a member");
        hashes.push(op.content_hash().expect("op hashes"));
    }

    let mut arrivals = Vec::new();
    for (index, record) in data.chunks_exact(RECORD_LEN).take(MAX_OPS).enumerate() {
        let [actor, kind, target, arg, parents, delivery] =
            <[u8; RECORD_LEN]>::try_from(record).expect("chunks_exact yields whole records");
        let actor = usize::from(actor) % 4;
        let parents: Vec<[u8; 32]> = (0..PARENT_WINDOW)
            .filter(|bit| parents >> bit & 1 == 1)
            .filter_map(|bit| hashes.len().checked_sub(bit + 1))
            .map(|at| hashes[at])
            .collect();
        let op = cast.sign(actor, &parents, cast.group_op(kind, target, arg));
        hashes.push(op.content_hash().expect("op hashes"));
        arrivals.push((delivery, index, actor, op));
    }
    arrivals.sort_by_key(|(delivery, index, ..)| (*delivery, *index));

    for (_, _, actor, op) in &arrivals {
        cast.apply_and_check(*actor, op);
    }
});

fn apply(store: &Store, op: &SignedNamespaceOp) -> eyre::Result<()> {
    NamespaceGovernance::new(store, op.namespace_id)
        .apply_signed_op(op)
        .map(|_| ())
}

impl Cast {
    fn new() -> Self {
        let store = test_store();
        let group = ContextGroupId::from(NS);
        let keys = SEEDS.map(PrivateKey::from);
        let accounts = keys
            .each_ref()
            .map(|key| enrol_member(&store, &group, &key.public_key()));

        let local = PrivateKey::from(LOCAL_SEED).public_key();
        let (local_account, _, _) = enrol_local_device(&store, &group, &local);
        NamespaceRepository::new(&store)
            .store_identity(&group, &local, &LOCAL_SEED)
            .expect("store the node identity");
        MetaRepository::new(&store)
            .save(&group, &sample_meta_with_admin(accounts[OWNER]))
            .expect("root meta");
        let members = MembershipRepository::new(&store);
        for (account, role) in [
            (accounts[OWNER], GroupMemberRole::Admin),
            (local_account, GroupMemberRole::Member),
        ] {
            members
                .add_member(&group, &account, role)
                .expect("seed a member");
        }
        CapabilitiesRepository::new(&store)
            .set_default_capabilities(&group, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
            .expect("genesis default");
        GroupKeyring::new(&store, group)
            .store_key(&GROUP_KEY)
            .expect("genesis key");

        Self {
            store,
            group,
            keys,
            accounts,
            nonces: [0; 4],
        }
    }

    fn group_op(&self, kind: u8, target: u8, arg: u8) -> GroupOp {
        let member = self.accounts[usize::from(target) % 4];
        let role = [
            GroupMemberRole::Admin,
            GroupMemberRole::Member,
            GroupMemberRole::ReadOnly,
        ][usize::from(arg) % 3]
            .clone();
        let capabilities = MemberCapabilities::from_bits_truncate(u32::from(arg));
        match kind % 7 {
            0 => GroupOp::MemberAdded { member, role },
            1 => GroupOp::MemberRemoved {
                member,
                expected_group_state_hash: [0; 32],
                expected_context_state_hashes: Vec::new(),
            },
            2 => GroupOp::MemberLeft {
                member,
                expected_group_state_hash: [0; 32],
                expected_context_state_hashes: Vec::new(),
            },
            3 => GroupOp::MemberRoleSet { member, role },
            4 => GroupOp::MemberCapabilitySet {
                member,
                capabilities,
            },
            5 => GroupOp::DefaultCapabilitiesSet { capabilities },
            _ => GroupOp::Noop,
        }
    }

    fn sign(&mut self, actor: usize, parents: &[[u8; 32]], op: GroupOp) -> SignedNamespaceOp {
        self.nonces[actor] += 1;
        SignedNamespaceOp::sign(
            &self.keys[actor],
            NamespaceId::from(NS),
            parents.to_vec(),
            self.nonces[actor],
            NamespaceOp::Group {
                group_id: self.group,
                key_id: KeyId::from(GroupKeyring::key_id_for(&GROUP_KEY)),
                encrypted: GroupKeyring::encrypt_op(&GROUP_KEY, &op).expect("op seals"),
                key_rotation: None,
            },
        )
        .expect("a fixed-seed op signs")
    }

    fn answers(&self) -> Answers {
        let members = MembershipRepository::new(&self.store);
        let capabilities = CapabilitiesRepository::new(&self.store);
        Answers {
            roles: self
                .accounts
                .each_ref()
                .map(|account| members.role_of(&self.group, account).expect("role")),
            capabilities: self.accounts.each_ref().map(|account| {
                capabilities
                    .member_capability(&self.group, account)
                    .expect("capability")
            }),
            default_capabilities: capabilities
                .default_capabilities(&self.group)
                .expect("default capabilities"),
            admins: self
                .accounts
                .iter()
                .filter(|account| members.is_admin(&self.group, account).expect("admin"))
                .count(),
        }
    }

    /// Whether `actor` holds authority over members: an admin, or a member with a granted capability.
    fn holds_authority(&self, actor: usize, answers: &Answers) -> bool {
        let account = self.accounts[actor];
        MembershipRepository::new(&self.store)
            .is_admin(&self.group, &account)
            .expect("admin")
            || (answers.roles[actor].is_some() && answers.capabilities[actor].unwrap_or(0) != 0)
    }

    fn apply_and_check(&self, actor: usize, op: &SignedNamespaceOp) {
        let before = self.answers();
        let authorised = self.holds_authority(actor, &before);
        let applied = apply(&self.store, op);
        let after = self.answers();

        if applied.is_err() {
            assert_eq!(before, after, "a refused op changed the group");
        }
        if before.admins > 0 {
            assert!(after.admins > 0, "the group lost its last admin");
        }
        let gained_role = match (&before.roles[OUTSIDER], &after.roles[OUTSIDER]) {
            (None, Some(_)) => true,
            (was, Some(GroupMemberRole::Admin)) => *was != Some(GroupMemberRole::Admin),
            _ => false,
        };
        let gained_capability = after.capabilities[OUTSIDER].unwrap_or(0)
            & !before.capabilities[OUTSIDER].unwrap_or(0)
            != 0;
        assert!(
            authorised || !(gained_role || gained_capability),
            "the outsider gained standing from an op nobody with authority signed"
        );
    }
}
