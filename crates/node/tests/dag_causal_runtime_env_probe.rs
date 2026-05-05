//! Regression probe for #2272 review 🔴: confirms the seam between
//! `MainStorage`'s `RUNTIME_ENV` thread-local route and direct datastore
//! reads behaves the way the production fix in `delta_store.rs` assumes.
//!
//! The bug this guards against: `resolve_effective_writers_for_delta`
//! runs **before** `context_client.execute()`, so the `RUNTIME_ENV`
//! thread-local that `MainStorage::storage_read` consults is not yet
//! installed. Reads via `MainStorage` fall through to an in-process
//! empty mock and return `None` — silently disabling the entire
//! DAG-causal verifier in production.
//!
//! What this probe pins:
//!
//! 1. **Inside** a `with_runtime_env` scope, `MainStorage::storage_read`
//!    *does* see writes that went through the same env. (Sanity check —
//!    if this ever fails, the bridge itself is broken.)
//!
//! 2. **Outside** that scope, `MainStorage::storage_read` returns
//!    `None` even when the bytes are present in the underlying store.
//!    This is the regression: it's what made the silent-disable
//!    possible.
//!
//! 3. A direct `Handle<Store>::get` against the same state key
//!    returns the bytes regardless of whether `RUNTIME_ENV` is set.
//!    This is what the production fix uses.
//!
//! If any of these pin assumptions changes (e.g. someone wires
//! `MainStorage` to fall through to the datastore handle), this probe
//! fails loudly and the production path needs re-evaluation.

use std::sync::Arc;

use borsh::{from_slice, to_vec};
use calimero_node_primitives::sync::storage_bridge::create_runtime_env;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::env::{storage_write, with_runtime_env};
use calimero_storage::rotation_log::{self, RotationLog, RotationLogEntry};
use calimero_storage::store::{Key as StorageKey, MainStorage, StorageAdaptor};
use calimero_store::db::InMemoryDB;
use calimero_store::{key, Store};

fn pk(b: u8) -> PublicKey {
    PublicKey::from([b; 32])
}

fn dummy_entry() -> RotationLogEntry {
    use core::num::NonZeroU128;

    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

    let ts = Timestamp::new(NTP64(100), ID::from(NonZeroU128::new(1).unwrap()));
    RotationLogEntry {
        delta_id: [0xAB; 32],
        delta_hlc: HybridTimestamp::new(ts),
        signer: Some(pk(0xAA)),
        new_writers: [pk(0xAA), pk(0xBB)].into_iter().collect(),
        writers_nonce: 1,
    }
}

#[tokio::test]
async fn rotation_log_route_seam() {
    // Production-shaped backend: real `Store` over an in-memory DB.
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let context_id = ContextId::from([0xCA; 32]);
    let executor = pk(0xEE);
    let entity_id = Id::from([0xEF; 32]);

    let env = create_runtime_env(&store, context_id, executor);

    // ---- Phase 1: write through the env (mimics WASM-side rotation log
    //               persistence — what `Interface::apply_action`'s write
    //               hook does inside `context_client.execute()`).
    let log = RotationLog {
        snapshot: None,
        entries: vec![dummy_entry()],
    };
    let log_bytes = to_vec(&log).expect("encode rotation log");
    with_runtime_env(env.clone(), || {
        // Bypass `rotation_log::save` to avoid coupling this probe to its
        // internal layout — write the raw bytes under the same key the
        // bridge expects.
        let written = storage_write(StorageKey::RotationLog(entity_id), &log_bytes);
        // First write returns false (no prior value); idempotent re-runs
        // would return true. We only assert the operation completed.
        let _ = written;
    });

    // ---- Phase 2 (sanity): inside the env, `MainStorage` sees the write.
    let inside_env_read = with_runtime_env(env.clone(), || {
        MainStorage::storage_read(StorageKey::RotationLog(entity_id))
    });
    assert!(
        inside_env_read.is_some(),
        "regression: MainStorage::storage_read returned None *inside* the \
         RuntimeEnv scope — the bridge itself is broken"
    );

    // ---- Phase 3 (the bug): outside the env, `MainStorage` falls
    //               through to an empty in-process mock and returns None
    //               — even though the bytes are still in the store.
    //               This is what made #2272's silent-disable possible.
    let outside_env_main = MainStorage::storage_read(StorageKey::RotationLog(entity_id));
    assert!(
        outside_env_main.is_none(),
        "PIN INVALIDATED: MainStorage::storage_read returned Some *outside* \
         RuntimeEnv. If this fires, MainStorage now falls through to a \
         real backend, which means the production `load_rotation_log_direct` \
         workaround in delta_store.rs may no longer be needed — re-evaluate."
    );

    // Also confirm: through `rotation_log::load::<MainStorage>` (the
    // path #2272 originally took), the answer is None outside the env.
    let outside_env_load = rotation_log::load::<MainStorage>(entity_id)
        .expect("rotation_log::load should not error on missing key");
    assert!(
        outside_env_load.is_none(),
        "regression: rotation_log::load::<MainStorage> returned Some \
         outside RuntimeEnv — see PIN INVALIDATED note above."
    );

    // ---- Phase 4 (the fix): a direct `Handle<Store>::get` against the
    //               state key returns the bytes regardless of env.
    //               This is what `load_rotation_log_direct` uses.
    let storage_key = StorageKey::RotationLog(entity_id).to_bytes();
    let state_key = key::ContextState::new(context_id, storage_key);
    let handle = store.handle();
    let direct_bytes: Vec<u8> = handle
        .get(&state_key)
        .expect("datastore read")
        .expect("rotation log present in store")
        .value
        .into_boxed()
        .into_vec();
    drop(handle);

    let decoded = from_slice::<RotationLog>(&direct_bytes).expect("decode rotation log");
    assert_eq!(
        decoded.entries.len(),
        1,
        "direct read decoded a different number of entries than written"
    );
}
