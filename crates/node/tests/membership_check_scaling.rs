//! Readiness beacons, acks and migration heartbeats each ask whether ONE signer
//! speaks for a namespace member, once per gossip message. That answer must not
//! cost more as the namespace grows: it used to list every member row and search
//! the list.
//!
//! A scaling guard (see `docs/benchmarking.md`): the namespace holds one bound
//! signer and `n` other member rows, so only the member count moves. Reading one
//! member row grows ~1x from 250 to 2000 members; listing them grows ~8x.
//!
//! On RocksDB, because the in-memory test store clones a whole column per
//! iterator and would charge the signer's binding scan for the member rows.

use std::time::{Duration, Instant};

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::signer_is_namespace_member;
use calimero_governance_store::MembershipRepository;
use calimero_governance_types::NamespaceId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::config::StoreConfig;
use calimero_store::Store;
use calimero_store_rocksdb::RocksDB;
use tempfile::TempDir;

const SMALL: usize = 250;
const LARGE: usize = 8 * SMALL;
const MAX_GROWTH: f64 = 4.0;
const CALLS: usize = 50;

fn measure(n: usize) -> Duration {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().to_owned().try_into().expect("utf-8 path");
    let store = Store::open::<RocksDB>(&StoreConfig::new(path)).expect("open rocksdb");

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

    (0..3)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..CALLS {
                assert!(signer_is_namespace_member(
                    &store,
                    NamespaceId::from(namespace_id),
                    &signer
                ));
            }
            start.elapsed()
        })
        .min()
        .expect("three runs")
}

#[test]
fn the_membership_check_does_not_grow_with_the_member_count() {
    let _warm = measure(SMALL);
    let small = measure(SMALL);
    let large = measure(LARGE);
    let growth = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        growth <= MAX_GROWTH,
        "{SMALL} -> {LARGE} members took {small:?} -> {large:?} for {CALLS} checks, \
         {growth:.1}x; reading one member row is ~1x, listing them ~8x"
    );
}
