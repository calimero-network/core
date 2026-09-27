//! Two accounts claiming one key of an owned collection split the context.
//!
//! An owned entry's id is derived from its key alone, and every node refuses a
//! write that would change an entry's owner. So when two accounts insert the
//! same key without seeing each other's write, each node keeps whichever claim
//! reached it first and refuses the other, forever: the nodes never agree on a
//! root hash again, and sync between them never converges.
//!
//! No race is needed. A member who writes their own claim to a key another
//! account already holds, and delivers it to a node that does not have the real
//! entry yet (a new joiner), splits that node from every other.
//!
//! These tests replay both orders on fresh stores and compare the result. They
//! are ignored until owned collections derive an entry's id from its owner as
//! well as its key, which makes the two claims different entries.

use calimero_account::AccountId;
use serial_test::serial;

use crate::action::Action;
use crate::collections::{Authored, LwwRegister, Root, UnorderedMap};
use crate::entities::{ChildInfo, Data, EntryRules, Metadata};
use crate::env;
use crate::tests::common::account_of_key;
use crate::tests::owned_rules::{act_as, apply, entry_bytes, key, signed, text};

type Posts = Authored<UnorderedMap<String, LwwRegister<String>>>;

/// Claims carry fixed timestamps, so two nodes differ only by what they kept.
const CLAIMED_AT: u64 = 1_700_000_000_000_000_000;

/// One node's copy of the collection, with the same ids on every node.
fn fresh_node() -> Root<Posts> {
    env::reset_for_testing();
    let _ = act_as(&key(0x01));
    Root::new(|| Posts::new_with_field_name("posts"))
}

/// A signed claim of `post` by `author`, as its node would ship it.
fn claim(posts: &Root<Posts>, post: &str, body: &str, author: u8, at: u64) -> Action {
    let sk = key(author);
    let id = posts.entry_id(&post.to_owned());
    let inner: &UnorderedMap<String, LwwRegister<String>> = posts;
    let parent = inner.id();
    let data = entry_bytes(id, &post.to_owned(), &text(body));
    signed(
        move |metadata| Action::Add {
            id,
            data,
            ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
            metadata,
        },
        account_of_key(&sk),
        EntryRules::OWNED,
        &sk,
        at,
    )
}

/// What a node holds for the contested key: its owner and its value.
type Held = (Option<AccountId>, Option<String>);

/// Apply `first` then `second` on a fresh node and return what it holds. The
/// second claim's refusal is the behaviour under test, so its result is ignored.
fn node_applying(first: (u8, &str), second: (u8, &str)) -> Held {
    let posts = fresh_node();
    for (author, body) in [first, second] {
        let action = claim(&posts, "p1", body, author, CLAIMED_AT + u64::from(author));
        let _ = apply(action, account_of_key(&key(author)));
    }
    let post = "p1".to_owned();
    (
        posts.owner_of(&post).expect("owner"),
        posts.get(&post).expect("get").map(|v| v.get().clone()),
    )
}

#[test]
#[serial]
#[ignore = "known split: owned entry ids come from the key alone; fixed by deriving them from the owner too"]
fn two_accounts_claiming_one_key_converge_whatever_the_order() {
    let alice_first = node_applying((0xA1, "alice's post"), (0xB0, "bob's post"));
    let bob_first = node_applying((0xB0, "bob's post"), (0xA1, "alice's post"));
    assert_eq!(
        alice_first, bob_first,
        "every node must hold the same entry once it has seen both claims"
    );
}

#[test]
#[serial]
#[ignore = "known split: owned entry ids come from the key alone; fixed by deriving them from the owner too"]
fn a_claim_delivered_to_a_joiner_first_does_not_split_it_from_the_group() {
    // The group holds Alice's post; Mallory hands a joiner her own claim of
    // the same key before the joiner has Alice's.
    let group = node_applying((0xA1, "alice's post"), (0xEE, "mallory's claim"));
    let joiner = node_applying((0xEE, "mallory's claim"), (0xA1, "alice's post"));
    assert_eq!(group, joiner, "the joiner must end where the group is");
}

/// The comparison is sound: the same claims in the same order leave two nodes
/// holding the same entry.
#[test]
#[serial]
fn the_same_claims_in_the_same_order_leave_the_same_entry() {
    let first = node_applying((0xA1, "alice's post"), (0xB0, "bob's post"));
    let again = node_applying((0xA1, "alice's post"), (0xB0, "bob's post"));
    assert_eq!(first, again);
}

/// Pins today's behaviour, which the two ignored tests above describe: a
/// second owner's claim of a taken key is refused, and which one a node keeps
/// depends only on arrival order.
#[test]
#[serial]
fn today_the_first_claim_to_arrive_is_kept_and_the_other_refused() {
    let posts = fresh_node();
    apply(
        claim(&posts, "p1", "alice's post", 0xA1, CLAIMED_AT + 1),
        account_of_key(&key(0xA1)),
    )
    .expect("the first claim lands");
    assert!(apply(
        claim(&posts, "p1", "bob's post", 0xB0, CLAIMED_AT + 2),
        account_of_key(&key(0xB0)),
    )
    .is_err());
    assert_eq!(
        posts
            .get(&"p1".to_owned())
            .expect("get")
            .map(|v| v.get().clone()),
        Some("alice's post".to_owned())
    );

    let alice_first = node_applying((0xA1, "alice's post"), (0xB0, "bob's post"));
    let bob_first = node_applying((0xB0, "bob's post"), (0xA1, "alice's post"));
    assert_ne!(
        alice_first, bob_first,
        "the two orders leave different entries"
    );
}
