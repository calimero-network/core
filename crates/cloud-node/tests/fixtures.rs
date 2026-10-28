//! The shared fixture helper builds both components, and the cache serves the second call.

mod common;

use std::time::Instant;

#[test]
fn fixtures_build_into_components_and_are_cached_by_source_hash() {
    for name in ["kv", "org-gated"] {
        let first = common::component(name);
        assert_eq!(&first[..4], b"\0asm", "{name}");
        let started = Instant::now();
        let second = common::component(name);
        assert_eq!(first, second, "{name}");
        assert!(
            started.elapsed().as_secs() < 2,
            "{name}: the second call rebuilt"
        );
        let abi = calimero_app_build::abi_of(&first).unwrap().expect("abi");
        assert!(abi["methods"].is_object(), "{name}");
    }
}
