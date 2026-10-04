//! Readiness beacons, acks and migration heartbeats each resolve ONE signing key
//! to its account and ask whether that account is a namespace member, once per
//! gossip message. Neither answer may cost more as the namespace grows: the key
//! lookup used to scan every device binding, and the membership check to list
//! every member row.
//!
//! Scaling guards (see `docs/benchmarking.md`). Each holds the part it does not
//! measure fixed and grows the other 8x; a point read grows ~1x, a scan ~8x.
//!
//! On RocksDB, because the in-memory test store clones a whole column per
//! iterator, which would charge any scan for every row in the column.

use std::time::{Duration, Instant};

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::signer_is_namespace_member;
use calimero_governance_store::{member_account_in_namespace, MembershipRepository};
use calimero_governance_types::NamespaceId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::config::StoreConfig;
use calimero_store::Store;
use calimero_store_rocksdb::RocksDB;
use tempfile::TempDir;

const MAX_GROWTH: f64 = 4.0;
const CALLS: usize = 50;

fn rocksdb() -> (TempDir, Store) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().to_owned().try_into().expect("utf-8 path");
    let store = Store::open::<RocksDB>(&StoreConfig::new(path)).expect("open rocksdb");
    (dir, store)
}

fn assert_flat(
    what: &str,
    small_n: usize,
    large_n: usize,
    mut measure: impl FnMut(usize) -> Duration,
) {
    let _warm = measure(small_n);
    let small = measure(small_n);
    let large = measure(large_n);
    let growth = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        growth <= MAX_GROWTH,
        "{what}: {small_n} -> {large_n} took {small:?} -> {large:?} for {CALLS} calls, \
         {growth:.1}x; a point read is ~1x, a scan ~8x"
    );
}

fn fastest_of_three(mut calls: impl FnMut()) -> Duration {
    (0..3)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..CALLS {
                calls();
            }
            start.elapsed()
        })
        .min()
        .expect("three runs")
}

/// One bound signer and `n` other member rows.
fn membership_check(n: usize) -> Duration {
    let (_dir, store) = rocksdb();

    let namespace_id = [0x5C; 32];
    let group = ContextGroupId::from(namespace_id);
    let members = MembershipRepository::new(&store);
    for i in 0..n {
        let mut account = [0xA5; 32];
        account[..8].copy_from_slice(&(i as u64).to_le_bytes());
        members
            .add_member(&group, &AccountId::from(account), GroupMemberRole::Member)
            .expect("add member");
    }
    let signer = PrivateKey::from([0x3C; 32]).public_key();
    let account = calimero_governance_store::test_fixtures::enrol_member(&store, &group, &signer);
    members
        .add_member(&group, &account, GroupMemberRole::Member)
        .expect("add the signer's account");

    fastest_of_three(|| {
        assert!(signer_is_namespace_member(
            &store,
            NamespaceId::from(namespace_id),
            &signer
        ));
    })
}

/// `n` bound devices, each its own account; the probed key is one of them, or a
/// stranger's that no binding names.
fn key_lookup(n: usize, stranger: bool) -> Duration {
    let (_dir, store) = rocksdb();
    let group = ContextGroupId::from([0x6D; 32]);
    let key_for = |i: usize| {
        let mut seed = [0x2E; 32];
        seed[..8].copy_from_slice(&(i as u64 + 1).to_le_bytes());
        PrivateKey::from(seed).public_key()
    };
    for i in 0..n {
        let _ = calimero_governance_store::test_fixtures::enrol_member(&store, &group, &key_for(i));
    }
    let probe = if stranger {
        PrivateKey::from([0xEE; 32]).public_key()
    } else {
        key_for(n / 2)
    };
    fastest_of_three(|| {
        let account = member_account_in_namespace(&store, &group, &probe).expect("lookup");
        assert_eq!(account.is_none(), stranger);
    })
}

#[test]
fn the_membership_check_does_not_grow_with_the_member_count() {
    assert_flat("membership check", 250, 2000, membership_check);
}

#[test]
fn a_signing_key_lookup_does_not_grow_with_the_device_count() {
    assert_flat("member key lookup", 125, 1000, |n| key_lookup(n, false));
    assert_flat("stranger key lookup", 125, 1000, |n| key_lookup(n, true));
}
