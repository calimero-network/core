//! Every replica reads every owner's entry by key, not only its own.
//!
//! Each replica writes one key of its own into each owned collection, and every
//! replica then reads every other replica's entry back the two ways an app
//! names one: `entries_at(key)` (every holder of a key) and `get_by(owner, key)`.
//! The shapes are the ones apps ship: `Moderated<IndexedMap>` (a forum's
//! posts), `WriteOnce<UnorderedMap>` (a chat's message ids) and
//! `WriteOnce<SortedMap>` read by an account's key prefix (a game's moves),
//! each a top-level field. The index and ordered reads are rebuilt on the
//! replica from what apply linked, so they are checked there too.
//!
//! Run with: `cargo test -p calimero-storage --features testing --test converge_owned_reads`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use calimero_account::AccountId;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{
    IndexValue, Indexed, IndexedMap, MergeStrategy, Mergeable, Moderated, SortedMap, UnorderedMap,
    WriteOnce,
};
use calimero_storage::testing::converge;
use calimero_storage::{env, rekey_field_if_supported};

const REPLICAS: usize = 2;

const SEEDS: [u64; 2] = [1, 0x5EED];

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Post {
    created_at: u64,
    deleted: bool,
}

impl Indexed for Post {
    const INDEXES: &'static [&'static str] = &["feed"];

    fn index_keys(&self, _index: usize, out: &mut Vec<Vec<u8>>) {
        (&self.deleted, &self.created_at).encode_index(out);
    }
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Apps {
    posts: Moderated<IndexedMap<String, Post>>,
    ids: WriteOnce<UnorderedMap<String, u64>>,
    shots: WriteOnce<SortedMap<String, u64>>,
}

// Structural: a test fixture, merged by the storage layer's own rules.
#[diagnostic::do_not_recommend]
impl MergeStrategy for Apps {
    const DISPATCHED: bool = false;
}

impl Mergeable for Apps {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.posts.merge(&other.posts)?;
        self.ids.merge(&other.ids)?;
        self.shots.merge(&other.shots)
    }
}

impl calimero_storage::collections::rekey::RekeyTarget for Apps {
    fn rekey_relative_to(&mut self, parent_id: calimero_storage::address::Id) {
        use calimero_storage::collections::rekey::field_child_id;
        rekey_field_if_supported!(&mut self.posts, field_child_id(parent_id, "posts"));
        rekey_field_if_supported!(&mut self.ids, field_child_id(parent_id, "ids"));
        rekey_field_if_supported!(&mut self.shots, field_child_id(parent_id, "shots"));
    }
}

fn me() -> AccountId {
    AccountId::from(env::account_id())
}

/// The key each replica writes: its own account, so every key has one holder.
fn my_key() -> String {
    hex::encode(env::account_id())
}

fn value_of(owner: &AccountId) -> u64 {
    u64::from(owner.as_bytes()[0])
}

#[test]
fn every_replica_reads_every_owners_entry_by_key() {
    for seed in SEEDS {
        converge::<Apps>()
            .replicas(REPLICAS)
            .seed(seed)
            .ops(|a: &mut Apps| {
                let post = Post {
                    created_at: value_of(&me()),
                    deleted: false,
                };
                a.posts.insert(my_key(), post).unwrap();
            })
            .ops(|a: &mut Apps| {
                a.ids.insert(my_key(), value_of(&me())).unwrap();
            })
            .ops(|a: &mut Apps| {
                a.shots
                    .insert(format!("{}/0", my_key()), value_of(&me()))
                    .unwrap();
            })
            .invariant("posts: one entry per replica", |a: &Apps| {
                a.posts.entries_with_owners().unwrap().len() == REPLICAS
            })
            .invariant("posts: entries_at finds every holder", |a: &Apps| {
                a.posts
                    .entries_with_owners()
                    .unwrap()
                    .into_iter()
                    .all(|(owner, key, _)| {
                        let at = a.posts.entries_at(&key).unwrap();
                        at.len() == 1 && at[0].0 == owner
                    })
            })
            .invariant("posts: get_by names every holder", |a: &Apps| {
                a.posts
                    .entries_with_owners()
                    .unwrap()
                    .into_iter()
                    .all(|(owner, key, _)| a.posts.get_by(&owner, &key).unwrap().is_some())
            })
            .invariant("posts: the feed index holds every post", |a: &Apps| {
                a.posts.query("feed").eq(&false).entries().unwrap().len() == REPLICAS
            })
            .invariant("ids: entries_at finds every holder", |a: &Apps| {
                let all = a.ids.entries_with_owners().unwrap();
                all.len() == REPLICAS
                    && all.into_iter().all(|(owner, key, value)| {
                        a.ids.entries_at(&key).unwrap() == vec![(owner, value)]
                    })
            })
            .invariant("shots: get_by names every holder", |a: &Apps| {
                let all = a.shots.entries_with_owners().unwrap();
                all.len() == REPLICAS
                    && all.into_iter().all(|(owner, key, value)| {
                        a.shots.get_by(&owner, &key).unwrap() == Some(value)
                            && a.shots.entries_at(&key).unwrap() == vec![(owner, value)]
                    })
            })
            .invariant("shots: an owner's prefix finds its row", |a: &Apps| {
                a.shots.prefix(b"").unwrap().count() == REPLICAS
                    && a.shots
                        .entries_with_owners()
                        .unwrap()
                        .into_iter()
                        .all(|(owner, key, _)| {
                            let prefix = format!("{}/", hex::encode(owner.as_bytes()));
                            a.shots
                                .prefix(prefix.as_bytes())
                                .unwrap()
                                .map(|(k, _)| k)
                                .collect::<Vec<_>>()
                                == vec![key]
                        })
            })
            .assert_all_replicas_equal();
    }
}
