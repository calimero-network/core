//! Two accounts writing one key of an owned collection hold two entries.
//!
//! Keys of an owned collection are per owner. An entry's id is derived from its
//! key AND its owner ([`owned_entry_id`]), and every node refuses an owned entry
//! at an id that is not its owner's, and anything else at an owner-derived id.
//! So two accounts inserting one key without seeing each other's write write
//! two different entities, and every node ends up holding both, whatever order
//! they arrive in.
//!
//! When the id came from the key alone, apply refused the second claim of a
//! key as an owner change, so each node kept whichever claim reached it first
//! and the nodes never agreed on a root hash again. No race was needed: a
//! member could split a new joiner from the group by handing it their own claim
//! of a taken key first.
//!
//! These tests replay both orders on fresh stores and compare what each node
//! holds, then play each way a patched peer might try to stand in the way of an
//! account's own write.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use ed25519_dalek::SigningKey;
use serial_test::serial;

use crate::action::Action;
use crate::address::Id;
use crate::collections::{
    compute_collection_id, compute_id, owned_entry_id, owned_keyed_entry_id, Authored,
    AuthoredVector, IndexValue, Indexed, IndexedMap, LwwRegister, Moderated, Root, SortedMap,
    UnorderedMap, UserStorage,
};
use crate::entities::{ChildInfo, Data, EntryRules, Metadata, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{Interface, StorageError};
use crate::store::{MainStorage, MockedStorage, StorageAdaptor};
use crate::tests::common::{
    account_of_key, apply_ctx_for, assert_every_owned_entry_is_bound, map_entry_bytes, sign_action,
};
use crate::tests::owned_rules::{
    act_as, apply, delete, is_gone, key, later, rules_of, signed, text,
};

/// Plain values, so two nodes holding the same entries hold the same bytes: a
/// register would carry each node's own clock.
type Posts = Authored<UnorderedMap<String, String>>;

/// Claims carry fixed timestamps, so two nodes differ only by what they kept.
const CLAIMED_AT: u64 = 1_700_000_000_000_000_000;

const ALICE: u8 = 0xA1;
const BOB: u8 = 0xB0;
const MALLORY: u8 = 0xEE;

fn account(seed: u8) -> AccountId {
    account_of_key(&key(seed))
}

/// One node's copy of the collection, with the same ids on every node, as the
/// `#[app::state]` macro leaves them.
fn fresh_node() -> Root<Posts> {
    env::reset_for_testing();
    let _ = act_as(&key(0x01));
    Root::new(|| {
        let mut posts = Posts::new_with_field_name("posts");
        posts.reassign_deterministic_id("posts");
        posts
    })
}

fn posts_id(posts: &Root<Posts>) -> Id {
    let inner: &UnorderedMap<String, String> = posts;
    inner.id()
}

/// A signed `Add` of the map entry `(key, value)` at `id` under `parent`,
/// stamped `owner` and signed by `signer`.
fn add_at<K: BorshSerialize + AsRef<[u8]>, V: BorshSerialize>(
    parent: Id,
    id: Id,
    key: &K,
    value: &V,
    owner: AccountId,
    signer: &SigningKey,
    at: u64,
) -> Action {
    let data = map_entry_bytes(id, key, value);
    signed(
        move |metadata| Action::Add {
            id,
            data,
            ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
            metadata,
        },
        owner,
        EntryRules::OWNED,
        signer,
        at,
    )
}

/// A signed claim of `post` by `author`, at the author's own id for it, as its
/// node would ship it.
fn claim(posts: &Root<Posts>, post: &str, body: &str, author: u8, at: u64) -> Action {
    let owner = account(author);
    let id = posts.entry_id_of(&owner, &post.to_owned());
    add_at(
        posts_id(posts),
        id,
        &post.to_owned(),
        &body.to_owned(),
        owner,
        &key(author),
        at,
    )
}

fn metadata_of(action: &Action) -> Metadata {
    match action {
        Action::Add { metadata, .. }
        | Action::Update { metadata, .. }
        | Action::DeleteRef { metadata, .. } => metadata.clone(),
    }
}

fn full_hash_of(id: Id) -> Option<[u8; 32]> {
    <Index<MainStorage>>::get_hashes_for(id)
        .expect("hashes")
        .map(|(full, _own)| full)
}

/// What a node holds: every entry with its owner, and the hashes a peer
/// compares against its own.
#[derive(Debug, PartialEq)]
struct Held {
    entries: Vec<(AccountId, String, String)>,
    collection_hash: Option<[u8; 32]>,
    root_hash: Option<[u8; 32]>,
}

fn held(posts: &Root<Posts>) -> Held {
    let mut entries: Vec<_> = posts
        .entries_with_owners()
        .expect("entries")
        .into_iter()
        .collect();
    entries.sort();
    Held {
        entries,
        collection_hash: full_hash_of(posts_id(posts)),
        root_hash: full_hash_of(Id::root()),
    }
}

/// Apply `first` then `second` on a fresh node, and return what it holds.
fn node_applying(first: (u8, &str), second: (u8, &str)) -> Held {
    let posts = fresh_node();
    for (author, body) in [first, second] {
        let action = claim(&posts, "p1", body, author, CLAIMED_AT + u64::from(author));
        apply(action, account(author)).expect("every claim lands");
    }
    assert_every_owned_entry_is_bound();
    held(&posts)
}

fn not_allowed<T: core::fmt::Debug>(result: Result<T, StorageError>) -> bool {
    matches!(result, Err(StorageError::ActionNotAllowed(_)))
}

// ---------------------------------------------------------------------------
// Two owners, one key
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn two_accounts_claiming_one_key_converge_whatever_the_order() {
    let alice_first = node_applying((ALICE, "alice's post"), (BOB, "bob's post"));
    let bob_first = node_applying((BOB, "bob's post"), (ALICE, "alice's post"));
    assert_eq!(
        alice_first, bob_first,
        "every node must hold the same entries once it has seen both claims"
    );
    assert_eq!(alice_first.entries.len(), 2, "both claims are kept");
}

#[test]
#[serial]
fn a_claim_delivered_to_a_joiner_first_does_not_split_it_from_the_group() {
    // The group holds Alice's post; Mallory hands a joiner her own claim of
    // the same key before the joiner has Alice's.
    let group = node_applying((ALICE, "alice's post"), (MALLORY, "mallory's claim"));
    let joiner = node_applying((MALLORY, "mallory's claim"), (ALICE, "alice's post"));
    assert_eq!(group, joiner, "the joiner must end where the group is");
}

/// The comparison is sound: the same claims in the same order leave two nodes
/// holding the same entries.
#[test]
#[serial]
fn the_same_claims_in_the_same_order_leave_the_same_entries() {
    let first = node_applying((ALICE, "alice's post"), (BOB, "bob's post"));
    let again = node_applying((ALICE, "alice's post"), (BOB, "bob's post"));
    assert_eq!(first, again);
}

#[test]
#[serial]
fn both_claims_are_kept_as_separate_entries() {
    let posts = fresh_node();
    for (author, body) in [(ALICE, "alice's post"), (BOB, "bob's post")] {
        let action = claim(&posts, "p1", body, author, CLAIMED_AT + u64::from(author));
        apply(action, account(author)).expect("every claim lands");
    }
    let post = "p1".to_owned();
    let body = |value: Option<String>| value;
    assert_ne!(
        posts.entry_id_of(&account(ALICE), &post),
        posts.entry_id_of(&account(BOB), &post)
    );

    // A key-only read is the caller's own entry, and no one else's.
    let _ = act_as(&key(ALICE));
    assert_eq!(
        body(posts.get(&post).expect("get")),
        Some("alice's post".into())
    );
    assert_eq!(posts.owner_of(&post).expect("owner"), Some(account(ALICE)));
    let _ = act_as(&key(BOB));
    assert_eq!(
        body(posts.get(&post).expect("get")),
        Some("bob's post".into())
    );
    let _ = act_as(&key(MALLORY));
    assert_eq!(body(posts.get(&post).expect("get")), None);
    assert!(!posts.contains(&post).expect("contains"));

    // The other's is read by naming its owner.
    assert_eq!(
        body(posts.get_by(&account(ALICE), &post).expect("get_by")),
        Some("alice's post".into())
    );
    assert_eq!(
        body(posts.get_by(&account(BOB), &post).expect("get_by")),
        Some("bob's post".into())
    );

    // Iteration and the count show both.
    assert_eq!(posts.len().expect("len"), 2);
    let mut bodies: Vec<_> = posts.entries().expect("entries").map(|(_, v)| v).collect();
    bodies.sort();
    assert_eq!(bodies, ["alice's post", "bob's post"]);
    let mut holders: Vec<_> = posts
        .entries_at(&post)
        .expect("entries_at")
        .into_iter()
        .map(|(owner, _)| owner)
        .collect();
    holders.sort();
    let mut expected = vec![account(ALICE), account(BOB)];
    expected.sort();
    assert_eq!(holders, expected);
    assert_eq!(
        posts.entries_by(&account(BOB)).expect("entries_by"),
        [(post, "bob's post".to_owned())]
    );
}

// ---------------------------------------------------------------------------
// Nothing else may stand at an account's owned id
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn an_owned_entry_at_another_account_s_id_is_refused() {
    let posts = fresh_node();
    let post = "p1".to_owned();
    let alices = posts.entry_id_of(&account(ALICE), &post);
    let squat = add_at(
        posts_id(&posts),
        alices,
        &post,
        &"mallory's".to_owned(),
        account(MALLORY),
        &key(MALLORY),
        CLAIMED_AT,
    );
    assert!(not_allowed(apply(squat, account(MALLORY))));

    let action = claim(&posts, "p1", "alice's post", ALICE, CLAIMED_AT + 1);
    apply(action, account(ALICE)).expect("alice's own claim lands");
    let squatted = held(&posts);
    let untouched = node_applying((ALICE, "alice's post"), (ALICE, "alice's post"));
    assert_eq!(squatted, untouched);
}

#[test]
#[serial]
fn an_owned_entry_signed_by_someone_else_is_refused() {
    let posts = fresh_node();
    let post = "p1".to_owned();
    let alices = posts.entry_id_of(&account(ALICE), &post);
    let forged = add_at(
        posts_id(&posts),
        alices,
        &post,
        &"mallory's".to_owned(),
        account(ALICE),
        &key(MALLORY),
        CLAIMED_AT,
    );
    assert!(apply(forged, account(MALLORY)).is_err());
    assert!(posts
        .get_by(&account(ALICE), &post)
        .expect("get_by")
        .is_none());

    let action = claim(&posts, "p1", "alice's post", ALICE, CLAIMED_AT + 1);
    apply(action, account(ALICE)).expect("alice's own claim lands");
}

#[test]
#[serial]
fn an_owned_entry_at_its_key_s_own_id_is_refused() {
    let posts = fresh_node();
    let post = "p1".to_owned();
    let key_only = compute_id(posts_id(&posts), post.as_bytes());
    let unbound = add_at(
        posts_id(&posts),
        key_only,
        &post,
        &"alice's post".to_owned(),
        account(ALICE),
        &key(ALICE),
        CLAIMED_AT,
    );
    assert!(not_allowed(apply(unbound, account(ALICE))));
    assert_eq!(posts.len().expect("len"), 0);
}

#[test]
#[serial]
fn only_the_owner_s_entry_lands_at_its_owned_id_in_either_order() {
    fn squats(parent: Id, id: Id) -> Vec<Action> {
        let at = CLAIMED_AT;
        let data = map_entry_bytes(id, &"p1".to_owned(), &"squat".to_owned());
        [
            StorageType::Public,
            StorageType::Frozen,
            StorageType::Shared {
                writers: BTreeMap::from([(account(MALLORY), crate::entities::OpMask::FULL)]),
                signature_data: None,
            },
            StorageType::SharedMember {
                anchor: parent,
                signature_data: None,
            },
        ]
        .into_iter()
        .map(|storage_type| Action::Add {
            id,
            data: data.clone(),
            ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
            metadata: Metadata {
                created_at: at,
                updated_at: at.into(),
                storage_type,
                ..Metadata::default()
            },
        })
        .collect()
    }

    let node = |squat_first: bool| {
        let posts = fresh_node();
        let post = "p1".to_owned();
        let alices = posts.entry_id_of(&account(ALICE), &post);
        let squat = || {
            for action in squats(posts_id(&posts), alices) {
                assert!(not_allowed(apply(action, account(MALLORY))));
            }
        };
        if squat_first {
            squat();
        }
        let action = claim(&posts, "p1", "alice's post", ALICE, CLAIMED_AT + 1);
        apply(action, account(ALICE)).expect("alice's claim lands after any squat");
        if !squat_first {
            squat();
            let wipe = Action::DeleteRef {
                id: alices,
                deleted_at: later(),
                metadata: Metadata::default(),
            };
            assert!(not_allowed(apply(wipe, account(MALLORY))));
        }
        held(&posts)
    };

    let squatted_first = node(true);
    let claimed_first = node(false);
    assert_eq!(squatted_first, claimed_first);
    assert_eq!(squatted_first.entries.len(), 1);
}

#[test]
#[serial]
fn a_moderator_s_removal_by_owner_is_accepted_everywhere() {
    type Board = Moderated<UnorderedMap<String, LwwRegister<String>>>;

    env::reset_for_testing();
    let moderator = key(0x40);
    let _ = act_as(&moderator);
    let mut board = Root::new(Board::new);
    for author in [ALICE, BOB] {
        let _ = act_as(&key(author));
        board
            .insert("p".to_owned(), text("buy now"))
            .expect("each posts its own p");
    }
    let post = "p".to_owned();
    assert_eq!(board.len().expect("len"), 2);

    // Locally: the moderator removes Bob's, and Alice's stays.
    let _ = act_as(&moderator);
    assert!(board
        .remove_by(&account(BOB), &post)
        .expect("moderate")
        .is_some());
    assert!(board.get_by(&account(BOB), &post).expect("get").is_none());
    assert!(board.get_by(&account(ALICE), &post).expect("get").is_some());

    // On apply: every node takes the moderator's signed delete of Alice's.
    let alices = board.entry_id_of(&account(ALICE), &post);
    let at = later();
    let removal = signed(
        delete(alices, at),
        account(ALICE),
        rules_of(alices),
        &moderator,
        at,
    );
    apply(removal, account_of_key(&moderator)).expect("a moderator's delete applies");
    assert!(is_gone(alices));
    assert!(board.get_by(&account(ALICE), &post).expect("get").is_none());
}

// ---------------------------------------------------------------------------
// An entry whose stored key does not derive its id
// ---------------------------------------------------------------------------

/// A value an `IndexedMap` can index.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
struct Tagged {
    tag: String,
    n: u64,
}

impl Indexed for Tagged {
    const INDEXES: &'static [&'static str] = &["tag"];

    fn index_keys(&self, index: usize, out: &mut Vec<Vec<u8>>) {
        if index == 0 {
            self.tag.encode_index(out);
        }
    }
}

fn tagged(tag: &str, n: u64) -> Tagged {
    Tagged {
        tag: tag.to_owned(),
        n,
    }
}

/// Alice's signed entry at her id for key `a`, but holding key `b`: what a
/// patched owner would write to hold one key under another key's slot.
fn misfiled<V: BorshSerialize>(parent: Id, value: &V) -> Action {
    let id = owned_keyed_entry_id(compute_id(parent, b"a"), &account(ALICE));
    alices_entry_at(parent, id, value)
}

/// The same entry at Alice's unkeyed owned id for `a`, where apply does not
/// look for a key: the id an owned vector element has.
fn unkeyed<V: BorshSerialize>(parent: Id, value: &V) -> Action {
    let id = owned_entry_id(compute_id(parent, b"a"), &account(ALICE));
    alices_entry_at(parent, id, value)
}

fn alices_entry_at<V: BorshSerialize>(parent: Id, id: Id, value: &V) -> Action {
    add_at(
        parent,
        id,
        &"b".to_owned(),
        value,
        account(ALICE),
        &key(ALICE),
        later(),
    )
}

#[test]
#[serial]
fn an_entry_whose_key_does_not_fit_its_id_is_never_read() {
    let posts = fresh_node();
    let parent = posts_id(&posts);
    assert!(
        not_allowed(apply(
            misfiled(parent, &"misfiled".to_owned()),
            account(ALICE)
        )),
        "apply finds the key in the bytes and refuses it"
    );
    apply(unkeyed(parent, &"unkeyed".to_owned()), account(ALICE))
        .expect("a correctly signed, correctly bound entry at an unkeyed id applies");
    let _ = act_as(&key(ALICE));
    for post in ["a", "b"] {
        let post = post.to_owned();
        assert!(posts.get(&post).expect("get").is_none());
        assert!(posts.get_by(&account(ALICE), &post).expect("get").is_none());
        assert!(posts.entries_at(&post).expect("entries_at").is_empty());
    }
    assert_eq!(posts.entries().expect("entries").count(), 0);
    assert!(posts.entries_with_owners().expect("entries").is_empty());
    assert!(posts.my_entries().expect("mine").is_empty());
    assert_eq!(posts.len().expect("len"), 0);
}

#[test]
#[serial]
fn an_entry_whose_key_does_not_fit_its_id_stays_out_of_ordered_reads_and_queries() {
    type Sorted = Authored<SortedMap<String, u64, MockedStorage<9401>>>;
    type Queried = Authored<IndexedMap<String, Tagged, MockedStorage<9402>>>;
    type Scanned = Authored<IndexedMap<String, Tagged, Unindexed>>;

    env::reset_for_testing();
    let _ = act_as(&key(ALICE));
    let ctx = apply_ctx_for(account(ALICE));

    let sorted = Sorted::new();
    let parent = (*sorted).id();
    assert!(not_allowed(Interface::<MockedStorage<9401>>::apply_action(
        misfiled(parent, &7_u64),
        &ctx
    )));
    Interface::<MockedStorage<9401>>::apply_action(unkeyed(parent, &7_u64), &ctx).expect("applies");
    assert_eq!(sorted.keys().expect("keys").count(), 0);
    assert_eq!(sorted.prefix(b"").expect("prefix").count(), 0);
    assert_eq!(
        sorted
            .range("a".to_owned()..="b".to_owned())
            .expect("range")
            .count(),
        0
    );
    assert_eq!(sorted.first().expect("first"), None);
    assert_eq!(sorted.last().expect("last"), None);

    let queried = Queried::new();
    let parent = (*queried).id();
    assert!(not_allowed(Interface::<MockedStorage<9402>>::apply_action(
        misfiled(parent, &tagged("t", 1)),
        &ctx
    )));
    Interface::<MockedStorage<9402>>::apply_action(unkeyed(parent, &tagged("t", 1)), &ctx)
        .expect("applies");
    assert_eq!(queried.query("tag").eq("t").count().expect("count"), 0);

    let scanned = Scanned::new();
    let parent = (*scanned).id();
    assert!(not_allowed(Interface::<Unindexed>::apply_action(
        misfiled(parent, &tagged("t", 1)),
        &ctx
    )));
    Interface::<Unindexed>::apply_action(unkeyed(parent, &tagged("t", 1)), &ctx).expect("applies");
    assert_eq!(scanned.query("tag").eq("t").count().expect("count"), 0);
}

/// Every count agrees with what the reads return, whichever entry a patched
/// owner sent: apply refuses a keyed entry holding another key, and an entry
/// at an unkeyed id is neither read nor counted.
#[test]
#[serial]
fn an_entry_whose_key_does_not_fit_its_id_is_not_counted() {
    type Sorted = Authored<SortedMap<String, u64, MockedStorage<9406>>>;
    type SortedScan = Authored<SortedMap<String, u64, Unindexed>>;
    type Queried = Authored<IndexedMap<String, Tagged, MockedStorage<9407>>>;
    type Scanned = Authored<IndexedMap<String, Tagged, Unindexed>>;

    fn send<S: StorageAdaptor>(parent: Id, value: &impl BorshSerialize) {
        let ctx = apply_ctx_for(account(ALICE));
        for forged in [misfiled(parent, value), unkeyed(parent, value)] {
            let _ = Interface::<S>::apply_action(forged, &ctx);
        }
    }

    // (read, what it counts, what it yields)
    let mut counts = Vec::new();

    let posts = fresh_node();
    send::<MainStorage>(posts_id(&posts), &"forged".to_owned());
    counts.push((
        "unordered len",
        posts.len().expect("len"),
        posts.entries().expect("entries").count(),
    ));

    let _ = act_as(&key(ALICE));

    let sorted = Sorted::new();
    send::<MockedStorage<9406>>((*sorted).id(), &7_u64);
    counts.push((
        "sorted len",
        sorted.len().expect("len"),
        sorted.entries().expect("entries").count(),
    ));

    let scan = SortedScan::new();
    send::<Unindexed>((*scan).id(), &7_u64);
    counts.push((
        "unindexed sorted len",
        scan.len().expect("len"),
        scan.entries().expect("entries").count(),
    ));

    let queried = Queried::new();
    send::<MockedStorage<9407>>((*queried).id(), &tagged("t", 1));
    counts.push((
        "indexed len",
        queried.len().expect("len"),
        queried.entries().expect("entries").count(),
    ));
    counts.push((
        "indexed query count",
        queried.query("tag").count().expect("count"),
        queried.query("tag").entries().expect("query").len(),
    ));

    let scanned = Scanned::new();
    send::<Unindexed>((*scanned).id(), &tagged("t", 1));
    counts.push((
        "scanned len",
        scanned.len().expect("len"),
        scanned.entries().expect("entries").count(),
    ));
    counts.push((
        "scanned query count",
        scanned.query("tag").count().expect("count"),
        scanned.query("tag").entries().expect("query").len(),
    ));

    let wrong: Vec<_> = counts.iter().filter(|(_, n, read)| n != read).collect();
    assert!(wrong.is_empty(), "counts disagree with reads: {wrong:?}");
}

#[test]
#[serial]
fn an_owned_map_entry_ends_in_its_key_s_length() {
    let mut posts = fresh_node();
    let _ = act_as(&key(ALICE));
    posts
        .insert("ab".to_owned(), "v".to_owned())
        .expect("insert");
    let id = posts.entry_id_of(&account(ALICE), &"ab".to_owned());
    let stored = MainStorage::storage_read(crate::store::Key::Entry(id)).expect("stored");

    // value (4 + 1) ++ key (4 + 2) ++ element id (32) ++ key length (4)
    let mut expected = vec![1, 0, 0, 0, b'v', 2, 0, 0, 0, b'a', b'b'];
    expected.extend_from_slice(id.as_bytes());
    expected.extend_from_slice(&2_u32.to_le_bytes());
    assert_eq!(stored, expected);
    assert_eq!(
        crate::collections::keyed_entry_key(&stored, id),
        Some(&b"ab"[..])
    );
    // The row the entry shares with its index leaves the id to the row's key.
    let row = crate::row::encode(
        id,
        &crate::row::Row {
            index: MainStorage::storage_read(crate::store::Key::Index(id)),
            data: Some(stored),
        },
    );
    assert!(!row.windows(32).any(|bytes| bytes == id.as_bytes()));
}

/// A repair re-sends an entry with no ancestors; the key is checked against
/// the parent it is stored under.
#[test]
#[serial]
fn a_repair_rewriting_an_owned_entry_with_another_key_is_refused() {
    let mut posts = fresh_node();
    let _ = act_as(&key(ALICE));
    posts
        .insert("a".to_owned(), "alice's".to_owned())
        .expect("insert");
    let id = posts.entry_id_of(&account(ALICE), &"a".to_owned());
    let rewrite = |post: &str| {
        let data = map_entry_bytes(id, &post.to_owned(), &"rewritten".to_owned());
        signed(
            move |metadata| Action::Update {
                id,
                data,
                ancestors: vec![],
                metadata,
            },
            account(ALICE),
            EntryRules::OWNED,
            &key(ALICE),
            later(),
        )
    };
    assert!(not_allowed(apply(rewrite("b"), account(ALICE))));
    apply(rewrite("a"), account(ALICE)).expect("the same key rewrites");
    assert_eq!(
        posts.get(&"a".to_owned()).expect("get").as_deref(),
        Some("rewritten")
    );
}

/// An ancestor this node lacks is created without its bytes, so an owned map
/// entry cannot be one: its key could never be checked.
#[test]
#[serial]
fn an_owned_map_entry_is_never_created_as_an_ancestor() {
    let posts = fresh_node();
    let parent = posts_id(&posts);
    let entry = owned_keyed_entry_id(compute_id(parent, b"a"), &account(ALICE));
    let beneath = compute_collection_id(Some(entry), "nested");
    let owner = account(ALICE);
    let stamp = StorageType::User {
        owner,
        signature_data: None,
        rules: EntryRules::OWNED,
    };
    let data = map_entry_bytes(
        owned_keyed_entry_id(compute_id(beneath, b"x"), &owner),
        &"x".to_owned(),
        &"under a missing entry".to_owned(),
    );
    let forged = signed(
        |metadata| Action::Add {
            id: owned_keyed_entry_id(compute_id(beneath, b"x"), &owner),
            data,
            ancestors: vec![
                ChildInfo::new(beneath, [0; 32], Metadata::default()),
                ChildInfo::new(
                    entry,
                    [0; 32],
                    Metadata {
                        storage_type: stamp,
                        ..Metadata::default()
                    },
                ),
                ChildInfo::new(parent, [0; 32], Metadata::default()),
            ],
            metadata,
        },
        owner,
        EntryRules::OWNED,
        &key(ALICE),
        later(),
    );
    assert!(not_allowed(apply(forged, owner)));
    assert!(!<Index<MainStorage>>::has_index(entry));
    assert_eq!(posts.len().expect("len"), 0);
}

/// A key whose bytes are not the tail of its encoding cannot be found in the
/// entry by a peer, so an honest node refuses to write it.
#[test]
#[serial]
fn an_honest_node_never_stores_an_owned_entry_whose_key_it_cannot_find() {
    #[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
    struct Headed {
        head: String,
        tail: String,
    }

    impl AsRef<[u8]> for Headed {
        fn as_ref(&self) -> &[u8] {
            self.head.as_bytes()
        }
    }

    env::reset_for_testing();
    let _ = act_as(&key(ALICE));
    let mut map = Root::new(Authored::<UnorderedMap<Headed, u64>>::new);
    let headed = Headed {
        head: "h".to_owned(),
        tail: "t".to_owned(),
    };
    assert!(matches!(
        map.insert(headed.clone(), 1),
        Err(crate::collections::StoreError::StorageError(
            StorageError::ActionNotAllowed(_)
        ))
    ));
    assert_eq!(map.len().expect("len"), 0);
    assert!(map.get(&headed).expect("get").is_none());
}

/// Writes nested in an owned entry replay on a fresh node in the order its
/// owner's node recorded them: the entry lands before anything beneath it.
#[test]
#[serial]
fn nested_writes_in_an_owned_entry_replay_on_a_fresh_node() {
    use crate::delta::{commit_causal_delta, reset_delta_context, set_current_heads};

    type Tags = UnorderedMap<String, LwwRegister<u64>>;
    type Threads = Authored<UnorderedMap<String, Tags>>;

    let fresh = || {
        env::reset_for_testing();
        let _ = act_as(&key(0x01));
        let threads = Root::new(|| {
            let mut threads = Threads::new_with_field_name("threads");
            threads.reassign_deterministic_id("threads");
            threads
        });
        reset_delta_context();
        set_current_heads(vec![[0; 32]]);
        threads
    };

    let mut threads = fresh();
    let _ = act_as(&key(ALICE));
    env::set_device_id(*key(ALICE).verifying_key().as_bytes());
    let mut tags = Tags::new();
    let _ = tags
        .insert("rust".to_owned(), LwwRegister::new(1))
        .expect("insert");
    threads.insert("p".to_owned(), tags).expect("insert");
    let mut tags = threads.get(&"p".to_owned()).expect("get").expect("alice's");
    let _ = tags
        .insert("go".to_owned(), LwwRegister::new(2))
        .expect("insert");
    let actions = commit_causal_delta(&[0; 32])
        .expect("commit")
        .expect("a delta")
        .actions;

    let threads = fresh();
    for action in actions {
        let mut action = action;
        let signature = sign_action(&action, &key(ALICE));
        if let Action::Add { metadata, .. } | Action::Update { metadata, .. } = &mut action {
            if let StorageType::User {
                signature_data: Some(sig),
                ..
            } = &mut metadata.storage_type
            {
                sig.signature = signature;
            }
        }
        apply(action, account(ALICE)).expect("every recorded write applies");
    }
    let tags = threads
        .get_by(&account(ALICE), &"p".to_owned())
        .expect("get")
        .expect("replayed");
    let mut names: Vec<_> = tags.entries().expect("entries").map(|(k, _)| k).collect();
    names.sort();
    assert_eq!(names, ["go", "rust"]);
    assert_eq!(threads.len().expect("len"), 1);
    assert_every_owned_entry_is_bound();
}

// ---------------------------------------------------------------------------
// Every read of a key several owners hold
// ---------------------------------------------------------------------------

/// Alice holds `a` and `b`, Bob holds `b` and `c`.
fn two_owners_write<C>(write: impl Fn(&mut C, &str, u64), map: &mut C) {
    let _ = act_as(&key(ALICE));
    write(map, "a", 1);
    write(map, "b", 2);
    let _ = act_as(&key(BOB));
    write(map, "b", 20);
    write(map, "c", 30);
}

/// The values at `b`, in the order the index files them: by entry id.
fn at_b_by_id(mut ids: [(Id, u64); 2]) -> Vec<u64> {
    ids.sort();
    ids.into_iter().map(|(_, n)| n).collect()
}

/// Storage with no ordered index, like `PrivateStorage`: every ordered read
/// takes the in-memory fallback.
struct Unindexed;

impl StorageAdaptor for Unindexed {
    fn storage_read(key: crate::store::Key) -> Option<Vec<u8>> {
        MockedStorage::<9405>::storage_read(key)
    }

    fn storage_remove(key: crate::store::Key) -> bool {
        MockedStorage::<9405>::storage_remove(key)
    }

    fn storage_write(key: crate::store::Key, value: &[u8]) -> bool {
        MockedStorage::<9405>::storage_write(key, value)
    }
}

fn assert_sorted_reads<S: StorageAdaptor>(map: &Authored<SortedMap<String, u64, S>>) {
    let b = "b".to_owned();
    let bs = at_b_by_id([
        (map.entry_id_of(&account(ALICE), &b), 2),
        (map.entry_id_of(&account(BOB), &b), 20),
    ]);
    let values = |pairs: Vec<(String, u64)>| pairs.into_iter().map(|(_, n)| n).collect::<Vec<_>>();

    assert_eq!(map.len().expect("len"), 4);
    assert_eq!(
        map.keys().expect("keys").collect::<Vec<_>>(),
        ["a", "b", "b", "c"]
    );
    assert_eq!(
        values(map.entries().expect("entries").collect()),
        [1, bs[0], bs[1], 30]
    );
    assert_eq!(
        values(map.range(b.clone()..=b.clone()).expect("range").collect()),
        bs
    );
    assert_eq!(
        values(
            map.range(b.clone().."c".to_owned())
                .expect("range")
                .collect()
        ),
        bs
    );
    assert_eq!(
        values(
            map.range((
                core::ops::Bound::Excluded("a".to_owned()),
                core::ops::Bound::Excluded("c".to_owned())
            ))
            .expect("range")
            .collect()
        ),
        bs
    );
    assert_eq!(values(map.prefix(b"b").expect("prefix").collect()), bs);
    assert_eq!(values(map.page(1, 2).expect("page")), bs);
    assert_eq!(values(map.page(3, 5).expect("page")), [30]);
    assert_eq!(map.first().expect("first"), Some(("a".to_owned(), 1)));
    assert_eq!(map.last().expect("last"), Some(("c".to_owned(), 30)));
}

#[test]
#[serial]
fn every_owner_s_entry_shows_in_every_read_of_an_unordered_map() {
    env::reset_for_testing();
    let mut map = Root::new(Authored::<UnorderedMap<String, u64>>::new);
    two_owners_write(
        |m: &mut Authored<UnorderedMap<String, u64>>, k: &str, v: u64| {
            m.insert(k.to_owned(), v).expect("insert");
        },
        &mut *map,
    );
    assert_eq!(map.len().expect("len"), 4);
    let mut pairs: Vec<_> = map.entries().expect("entries").collect();
    pairs.sort();
    assert_eq!(
        pairs,
        [("a", 1), ("b", 2), ("b", 20), ("c", 30)].map(|(k, v)| (k.to_owned(), v))
    );
    assert_every_owned_entry_is_bound();
}

#[test]
#[serial]
fn every_owner_s_entry_shows_in_every_read_of_a_sorted_map_with_its_index() {
    env::reset_for_testing();
    let mut map = Authored::<SortedMap<String, u64, MockedStorage<9403>>>::new();
    two_owners_write(
        |m, k, v| m.insert(k.to_owned(), v).expect("insert"),
        &mut map,
    );
    assert_sorted_reads(&map);

    // A removal drops only that owner's row.
    let _ = act_as(&key(ALICE));
    assert_eq!(map.remove(&"b".to_owned()).expect("remove"), Some(2));
    let at_b: Vec<_> = map.prefix(b"b").expect("prefix").collect();
    assert_eq!(at_b, [("b".to_owned(), 20)]);
    assert_eq!(map.len().expect("len"), 3);
}

#[test]
#[serial]
fn every_owner_s_entry_shows_in_every_read_of_a_sorted_map_without_one() {
    env::reset_for_testing();
    let mut map = Authored::<SortedMap<String, u64, Unindexed>>::new();
    assert!(!Unindexed::index_supported(), "this is the fallback's test");
    two_owners_write(
        |m, k, v| m.insert(k.to_owned(), v).expect("insert"),
        &mut map,
    );
    assert_sorted_reads(&map);
}

#[test]
#[serial]
fn every_owner_s_entry_shows_in_every_query_of_an_indexed_map() {
    type Queried = Authored<IndexedMap<String, Tagged, MockedStorage<9404>>>;
    type Scanned = Authored<IndexedMap<String, Tagged, Unindexed>>;

    env::reset_for_testing();
    let mut queried = Queried::new();
    let mut scanned = Scanned::new();
    let write = |k: &str, v: u64| tagged(if k == "b" { "b" } else { "other" }, v);
    two_owners_write(
        |m: &mut Queried, k: &str, v: u64| m.insert(k.to_owned(), write(k, v)).expect("insert"),
        &mut queried,
    );
    two_owners_write(
        |m: &mut Scanned, k: &str, v: u64| m.insert(k.to_owned(), write(k, v)).expect("insert"),
        &mut scanned,
    );

    let b = "b".to_owned();
    let ns = |pairs: Vec<(String, Tagged)>| pairs.into_iter().map(|(_, t)| t.n).collect::<Vec<_>>();

    for (bs, asc, desc, count, len) in [
        (
            at_b_by_id([
                (queried.entry_id_of(&account(ALICE), &b), 2),
                (queried.entry_id_of(&account(BOB), &b), 20),
            ]),
            ns(queried.query("tag").eq("b").entries().expect("query")),
            ns(queried
                .query("tag")
                .eq("b")
                .desc()
                .entries()
                .expect("query")),
            queried.query("tag").count().expect("count"),
            queried.len().expect("len"),
        ),
        (
            at_b_by_id([
                (scanned.entry_id_of(&account(ALICE), &b), 2),
                (scanned.entry_id_of(&account(BOB), &b), 20),
            ]),
            ns(scanned.query("tag").eq("b").entries().expect("query")),
            ns(scanned
                .query("tag")
                .eq("b")
                .desc()
                .entries()
                .expect("query")),
            scanned.query("tag").count().expect("count"),
            scanned.len().expect("len"),
        ),
    ] {
        let mut bs_desc = bs.clone();
        bs_desc.reverse();
        assert_eq!(asc, bs);
        assert_eq!(desc, bs_desc);
        assert_eq!(count, 4);
        assert_eq!(len, 4);
    }

    // A moderator-free owner removal keeps the other owner's row.
    let _ = act_as(&key(BOB));
    let _ = queried.remove(&b).expect("remove");
    assert_eq!(
        ns(queried.query("tag").eq("b").entries().expect("query")),
        [2]
    );
}

// ---------------------------------------------------------------------------
// Collections nested in two owners' entries at one key
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn nested_collections_under_two_owners_entries_are_distinct() {
    type Tags = UnorderedMap<String, LwwRegister<u64>>;
    type Threads = Authored<UnorderedMap<String, Tags>>;

    env::reset_for_testing();
    let mut threads = Root::new(Threads::new);
    for (author, tag) in [(ALICE, "rust"), (BOB, "go")] {
        let _ = act_as(&key(author));
        let mut tags = Tags::new();
        let _ = tags
            .insert(tag.to_owned(), LwwRegister::new(1))
            .expect("insert");
        threads.insert("p".to_owned(), tags).expect("insert");
    }
    let names = |owner: u8| -> (Id, Vec<String>) {
        let tags = threads
            .get_by(&account(owner), &"p".to_owned())
            .expect("get")
            .expect("present");
        let names = tags.entries().expect("entries").map(|(k, _)| k).collect();
        (tags.id(), names)
    };
    let (alices, alice_names) = names(ALICE);
    let (bobs, bob_names) = names(BOB);
    assert_ne!(alices, bobs, "each entry holds its own collection");
    assert_eq!(alice_names, ["rust"]);
    assert_eq!(bob_names, ["go"]);

    // Mallory aims at the id Alice's next nested entry takes.
    let target = owned_keyed_entry_id(compute_id(alices, b"x"), &account(ALICE));
    let value = LwwRegister::new(666_u64);
    let x = "x".to_owned();
    let squat = add_at(
        alices,
        target,
        &x,
        &value,
        account(MALLORY),
        &key(MALLORY),
        later(),
    );
    assert!(not_allowed(apply(squat, account(MALLORY))));
    let forged = add_at(
        alices,
        target,
        &x,
        &value,
        account(ALICE),
        &key(MALLORY),
        later(),
    );
    assert!(apply(forged, account(MALLORY)).is_err());

    let _ = act_as(&key(ALICE));
    let mut tags = threads.get(&"p".to_owned()).expect("get").expect("alice's");
    let _ = tags
        .insert(x, LwwRegister::new(2))
        .expect("alice's own write lands");
    assert_eq!(tags.entry_id("x"), target);
    assert_every_owned_entry_is_bound();
}

// ---------------------------------------------------------------------------
// UserStorage and AuthoredVector
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn a_user_slot_keyed_by_its_victim_never_blocks_the_victim_in_either_order() {
    type Profiles = UserStorage<u64>;

    let node = |forgery_first: bool| {
        env::reset_for_testing();
        let _ = act_as(&key(0x01));
        let profiles = Root::new(|| {
            let mut profiles = Profiles::new_with_field_name("profiles");
            profiles.reassign_deterministic_id("profiles");
            profiles
        });
        let slots = profiles.inner_id();
        let bob = account(BOB);
        let slot = compute_id(slots, bob.as_ref());
        let parked = add_at(
            slots,
            owned_keyed_entry_id(slot, &account(MALLORY)),
            &bob,
            &666_u64,
            account(MALLORY),
            &key(MALLORY),
            CLAIMED_AT,
        );
        let own = add_at(
            slots,
            owned_keyed_entry_id(slot, &bob),
            &bob,
            &2_u64,
            bob,
            &key(BOB),
            CLAIMED_AT + 1,
        );
        let (first, second) = if forgery_first {
            ((parked, MALLORY), (own, BOB))
        } else {
            ((own, BOB), (parked, MALLORY))
        };
        for (action, author) in [first, second] {
            apply(action, account(author)).expect("both are correctly bound");
        }
        assert_eq!(profiles.get_for_user(&bob).expect("get"), Some(2));
        let slots: Vec<_> = profiles.entries().expect("entries").collect();
        assert_eq!(slots, [(bob, 2)]);
        (slots, full_hash_of(Id::root()))
    };
    assert_eq!(node(true), node(false));
}

#[test]
#[serial]
fn an_entry_at_another_account_s_vector_slot_is_refused() {
    env::reset_for_testing();
    let _ = act_as(&key(ALICE));
    let mut list = Root::new(AuthoredVector::<u64>::new);
    let _ = list.push(7).expect("alice pushes");
    let alices = list.entry_id_at(0).expect("id").expect("slot");
    let parent = Id::root();
    let squat = add_at(
        parent,
        alices,
        &[0_u8; 0],
        &9_u64,
        account(MALLORY),
        &key(MALLORY),
        later(),
    );
    assert!(not_allowed(apply(squat, account(MALLORY))));
    let unbound = add_at(
        parent,
        Id::random(),
        &[0_u8; 0],
        &9_u64,
        account(MALLORY),
        &key(MALLORY),
        later(),
    );
    assert!(not_allowed(apply(unbound, account(MALLORY))));
    assert_eq!(list.len().expect("len"), 1);
    assert_eq!(list.get(0).expect("get"), Some(7));
}

// ---------------------------------------------------------------------------
// Snapshot leaves
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn snapshot_verification_refuses_unbound_owned_leaves_and_squats() {
    let posts = fresh_node();
    let parent = posts_id(&posts);
    let post = "p1".to_owned();
    let verify = |action: &Action| {
        let Action::Add { id, data, .. } = action else {
            unreachable!("built as an add")
        };
        Interface::<MainStorage>::verify_snapshot_entity_signature(
            *id,
            Some(parent),
            data,
            &metadata_of(action),
        )
    };

    let bound = claim(&posts, "p1", "alice's post", ALICE, CLAIMED_AT);
    assert!(verify(&bound).is_ok(), "a correctly bound leaf verifies");

    let unbound = add_at(
        parent,
        compute_id(parent, post.as_bytes()),
        &post,
        &"alice's post".to_owned(),
        account(ALICE),
        &key(ALICE),
        CLAIMED_AT,
    );
    assert!(not_allowed(verify(&unbound)));

    let alices = posts.entry_id_of(&account(ALICE), &post);
    let squat = Action::Add {
        id: alices,
        data: map_entry_bytes(alices, &post, &"squat".to_owned()),
        ancestors: vec![],
        metadata: Metadata::default(),
    };
    assert!(not_allowed(verify(&squat)));

    // A leaf holding another key than its id derives, and one whose parent is
    // not known, are refused like the same entry on apply.
    assert!(not_allowed(verify(&misfiled(
        parent,
        &"misfiled".to_owned()
    ))));
    let Action::Add { id, data, .. } = &bound else {
        unreachable!("built as an add")
    };
    assert!(not_allowed(
        Interface::<MainStorage>::verify_snapshot_entity_signature(
            *id,
            None,
            data,
            &metadata_of(&bound),
        )
    ));
}

// ---------------------------------------------------------------------------
// The local write path
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn an_honest_node_never_stores_an_unbound_owned_entry() {
    use crate::tests::common::Page;

    env::reset_for_testing();
    let _ = act_as(&key(ALICE));
    let root = crate::tests::common::setup_root_for_main();
    let mut element = crate::entities::Element::new(Some(Id::random()));
    element.set_user_domain(account(ALICE));
    let mut page = Page::new_from_element("unbound", element);
    assert!(not_allowed(Interface::<MainStorage>::add_child_to(
        root.id(),
        &mut page
    )));
    assert!(Interface::<MainStorage>::find_by_id_raw(page.id()).is_none());

    let mut squat = Page::new_from_element(
        "squat",
        crate::entities::Element::new(Some(owned_entry_id(Id::random(), &account(ALICE)))),
    );
    assert!(not_allowed(Interface::<MainStorage>::add_child_to(
        root.id(),
        &mut squat
    )));
}
