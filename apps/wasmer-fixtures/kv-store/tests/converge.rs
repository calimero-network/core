//! Convergence coverage for the canonical `UnorderedMap<String, LwwRegister>`
//! key-value store. Distinct keys written concurrently must all survive (map
//! union); `LwwRegister` values converge by HLC last-writer-wins.

use kv_store::KvStore;

use calimero_storage::testing::converge_app;

#[test]
fn distinct_keys_all_survive() {
    converge_app(KvStore::init)
        .replicas(3)
        .ops(|s| {
            let _ = s.set("a".into(), "1".into());
        })
        .ops(|s| {
            let _ = s.set("b".into(), "2".into());
        })
        .ops(|s| {
            let _ = s.set("c".into(), "3".into());
        })
        .invariant("all three keys present", |s| s.len().unwrap_or(0) == 3)
        .assert_all_replicas_equal();
}
