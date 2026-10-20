//! A trial runs an upgrade's migrations on a context in a buffer that is dropped.
//!
//! Driven through a live `ContextManager` and hand-written wasm modules, so each
//! outcome is a plain read of what the live store holds afterwards.

use calimero_context_client::group::{TrialRung, TrialUpgradeRequest};
use calimero_context_client::messages::MigrationParams;
use calimero_node_primitives::test_fixtures::signed_wasm;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::hash::Hash;
use calimero_store::db::Column;
use calimero_store::key;
use futures_util::io::Cursor;

use super::state_write_gate_tests::{fixture, Fixture, LocalRole, INITIAL_ROOT};

const CHILD_KEY: [u8; key::STATE_KEY_LEN] = [0xAB; key::STATE_KEY_LEN];

fn escape(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

fn buffer(ptr: u64, len: u64) -> Vec<u8> {
    [ptr.to_le_bytes(), len.to_le_bytes()].concat()
}

/// Exports `migrate` running `body`, and a check answering `check_verdict` when given.
/// Memory: the child key at 0, its value at 40, the index key and value at 48 and 56, the
/// returned state at 64, the root key at 96 (33 bytes), the verdict at 144, then descriptors.
fn module(body: &str, check_verdict: Option<bool>) -> Vec<u8> {
    let root_key = calimero_storage::constants::root_storage_key();
    let ok = |ptr, len| [vec![0u8; 8], buffer(ptr, len)].concat();
    let check = check_verdict.map_or(String::new(), |_| {
        r#"(func (export "__calimero_migration_check") (call $ret (i64.const 288)))"#.to_owned()
    });
    let wat = format!(
        r#"
        (module
            (import "env" "storage_write" (func $write (param i64 i64 i64) (result i32)))
            (import "env" "storage_read" (func $read (param i64 i64) (result i32)))
            (import "env" "storage_index_set" (func $index_set (param i64 i64) (result i32)))
            (import "env" "value_return" (func $ret (param i64)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{child}")
            (data (i32.const 40) "val")
            (data (i32.const 48) "ik")
            (data (i32.const 56) "iv")
            (data (i32.const 64) "NEW")
            (data (i32.const 96) "{root}")
            (data (i32.const 144) "{verdict}")
            (data (i32.const 160) "{child_key}")
            (data (i32.const 176) "{child_value}")
            (data (i32.const 192) "{index_key}")
            (data (i32.const 208) "{index_value}")
            (data (i32.const 224) "{root_key}")
            (data (i32.const 240) "{returned}")
            (data (i32.const 288) "{verdict_returned}")
            (func (export "migrate") {body}))
        "#,
        child = escape(&CHILD_KEY),
        root = escape(&root_key),
        verdict = escape(&[u8::from(check_verdict.unwrap_or(true))]),
        child_key = escape(&buffer(0, CHILD_KEY.len() as u64)),
        child_value = escape(&buffer(40, 3)),
        index_key = escape(&buffer(48, 2)),
        index_value = escape(&buffer(56, 2)),
        root_key = escape(&buffer(96, root_key.len() as u64)),
        returned = escape(&ok(64, 3)),
        verdict_returned = escape(&ok(144, 1)),
    );
    let wat = wat.replace(
        "(func (export \"migrate\")",
        &format!("{check}\n            (func (export \"migrate\")"),
    );
    wat::parse_str(wat).expect("parse the module")
}

const WRITES: &str = "(drop (call $write (i64.const 160) (i64.const 176) (i64.const 0)))
    (drop (call $index_set (i64.const 192) (i64.const 208)))
    (call $ret (i64.const 240))";
const TRAPS: &str = "unreachable";
/// Traps unless the previous rung's root is readable, then returns.
const NEEDS_ROOT: &str =
    "(if (i32.eqz (call $read (i64.const 224) (i64.const 0))) (then unreachable))
    (call $ret (i64.const 240))";

async fn rung(fx: &Fixture, wasm: Vec<u8>) -> TrialRung {
    let (blob_id, _) = fx
        .harness
        .node_client
        .add_blob(Cursor::new(signed_wasm(&wasm)), None, None)
        .await
        .expect("store the wasm");
    TrialRung {
        bytecode_id: *blob_id.digest(),
        migration: Some(MigrationParams {
            method: "migrate".to_owned(),
        }),
    }
}

async fn trial(fx: &Fixture, rungs: Vec<TrialRung>) -> eyre::Result<bool> {
    fx.harness
        .context_client
        .trial_upgrade(TrialUpgradeRequest {
            context_id: fx.context_id,
            rungs,
        })
        .await
}

async fn held() -> Fixture {
    fixture(LocalRole::Role(GroupMemberRole::Member)).await
}

/// Everything a trial must not touch: the root, entries, index rows and both markers.
fn assert_untouched(fx: &Fixture) {
    let handle = fx.store.handle();
    assert_eq!(fx.root(), Hash::from(INITIAL_ROOT));
    let state = |key: [u8; key::STATE_KEY_LEN]| {
        handle
            .get(&key::ContextState::new(fx.context_id, key))
            .expect("read state")
            .is_some()
    };
    assert!(!state(CHILD_KEY), "the child entry reached the live store");
    assert!(
        !state(calimero_storage::constants::root_storage_key()),
        "the root reached the live store"
    );
    let index = fx
        .store
        .raw_scan(Column::SortedIndex, &[], &[0xFF; 64], None)
        .expect("scan the index");
    assert!(index.is_empty(), "index rows reached the live store");
    assert!(handle
        .get(&key::ContextMigrationFailed::new(fx.context_id))
        .expect("read the marker")
        .is_none());
    assert!(crate::activation::activated_bytecode(&fx.store, &fx.context_id).is_none());
}

#[actix::test]
async fn a_passing_trial_leaves_nothing_behind() {
    let fx = held().await;
    let rungs = vec![rung(&fx, module(WRITES, Some(true))).await];
    assert!(trial(&fx, rungs).await.expect("the trial passes"));
    assert_untouched(&fx);
}

#[actix::test]
async fn a_trapping_migrate_fails_the_trial_and_leaves_nothing_behind() {
    let fx = held().await;
    let rungs = vec![rung(&fx, module(TRAPS, None)).await];
    assert!(trial(&fx, rungs).await.is_err(), "the trial fails");
    assert_untouched(&fx);
}

#[actix::test]
async fn a_failing_check_fails_the_trial_naming_the_context() {
    let fx = held().await;
    let rungs = vec![rung(&fx, module(WRITES, Some(false))).await];
    let err = trial(&fx, rungs).await.expect_err("the trial fails");
    assert!(
        format!("{err}").contains(&fx.context_id.to_string()),
        "the error names the context: {err}"
    );
    assert_untouched(&fx);
}

#[actix::test]
async fn a_rung_reads_the_output_of_the_one_before() {
    let fx = held().await;
    let alone = vec![rung(&fx, module(NEEDS_ROOT, None)).await];
    assert!(
        trial(&fx, alone).await.is_err(),
        "with no earlier rung there is no root to read"
    );

    let ladder = vec![
        rung(&fx, module(WRITES, None)).await,
        rung(&fx, module(NEEDS_ROOT, None)).await,
    ];
    assert!(trial(&fx, ladder).await.expect("the ladder passes"));
    assert_untouched(&fx);
}

#[actix::test]
async fn a_context_the_node_does_not_hold_is_not_trialled() {
    let fx = fixture(LocalRole::Outsider).await;
    let rungs = vec![rung(&fx, module(TRAPS, None)).await];
    assert!(!trial(&fx, rungs).await.expect("skipped, not failed"));
}
