//! What a run may do with the node's private state (`#[app::private]`).
//!
//! Driven through [`execute`] with a hand-written wasm module that calls the
//! private-storage host functions directly, so "did it see the value?" and "did
//! it leave one behind?" are answered by the guest and by a later run rather
//! than by inspecting the store.
//!
//! A run for the node's own principal uses the node's private state. A delegated
//! run acts for someone else and must neither read that state nor change it.

use std::sync::Arc;

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use tokio::sync::RwLock;

use super::principal::Principal;
use super::state_write_gate_tests::global_runtime;
use super::storage::{ContextPrivateStorage, ContextStorage};
use super::{execute, ContextGuard};
use crate::test_support::{account_for, actor};

/// Two 32-byte private keys at 0 and 32, one value byte at 64, and the three
/// `{ptr: u64, len: u64}` descriptors the host functions read at 128 (first
/// key), 144 (second key) and 160 (value).
///
/// The `need_*` methods trap unless the named key is present or absent, so a
/// run's outcome says what it could see.
const MODULE: &str = r#"
    (module
        (import "env" "private_storage_read" (func $read (param i64 i64) (result i32)))
        (import "env" "private_storage_write" (func $write (param i64 i64) (result i32)))
        (import "env" "private_storage_remove" (func $remove (param i64 i64) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 0)
            "\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11\11"
            "\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22\22"
            "\07")
        (data (i32.const 128)
            "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00"
            "\20\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00"
            "\40\00\00\00\00\00\00\00\01\00\00\00\00\00\00\00")
        (func (export "write_first") (drop (call $write (i64.const 128) (i64.const 160))))
        (func (export "write_second") (drop (call $write (i64.const 144) (i64.const 160))))
        (func (export "remove_first") (drop (call $remove (i64.const 128) (i64.const 0))))
        (func (export "need_first") (if (i32.eqz (call $read (i64.const 128) (i64.const 0))) (then unreachable)))
        (func (export "need_no_first") (if (call $read (i64.const 128) (i64.const 0)) (then unreachable)))
        (func (export "need_no_second") (if (call $read (i64.const 144) (i64.const 0)) (then unreachable))))
"#;

struct Fixture {
    harness: actor::Harness,
    store: Store,
    context_id: ContextId,
    module: calimero_runtime::Module,
}

async fn fixture() -> Fixture {
    global_runtime();
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let harness = actor::over(store.clone()).await;
    let wasm = wat::parse_str(MODULE).expect("parse the module");
    let module =
        calimero_runtime::Engine::with_limits(calimero_runtime::logic::VMLimits::default())
            .compile(&wasm)
            .expect("compile the module");
    Fixture {
        harness,
        store,
        context_id: ContextId::from([0xC7; 32]),
        module,
    }
}

impl Fixture {
    /// Run `method`, undelegated or as an authenticated session's account, and
    /// commit its private state. Commits unconditionally, unlike the execute
    /// path, which does so only when the run produced a root hash; that gating
    /// is not covered here.
    async fn run(&self, method: &'static str, delegated: bool) -> bool {
        let guard = ContextGuard::write(Arc::new(RwLock::new(self.context_id)).write_owned().await);
        let device = PublicKey::from([0x33; 32]);
        let account = account_for(&device);
        let (outcome, _storage, private_storage) = execute(
            &guard,
            self.module.clone(),
            Principal::new(account, device),
            method.into(),
            Vec::new().into(),
            ContextStorage::from(self.store.clone(), self.context_id),
            ContextPrivateStorage::for_run::<calimero_account::Delegation>(
                self.store.clone(),
                self.context_id,
                None,
                delegated.then_some(account),
            ),
            self.harness.node_client.clone(),
            false,
            None,
            false,
            calimero_runtime::logic::SealingContext::default(),
            None,
        )
        .await
        .expect("the run executes");
        let completed = outcome.returns.is_ok();
        private_storage.commit().expect("commit the private state");
        completed
    }
}

/// The control: a run for the node's own principal keeps what it writes, and a
/// later run reads it back.
#[actix::test]
async fn an_undelegated_run_persists_and_reads_private_state() {
    let fx = fixture().await;
    assert!(fx.run("need_no_first", false).await);
    assert!(fx.run("write_first", false).await);
    assert!(fx.run("need_first", false).await);
}

#[actix::test]
async fn a_delegated_run_does_not_read_the_nodes_private_state() {
    let fx = fixture().await;
    assert!(fx.run("write_first", false).await);
    assert!(
        fx.run("need_no_first", true).await,
        "a delegated run must find the node's private state empty"
    );
}

#[actix::test]
async fn a_delegated_run_that_writes_private_state_fails_and_persists_nothing() {
    let fx = fixture().await;
    assert!(
        !fx.run("write_second", true).await,
        "a write that would be discarded must fail the run, not report success"
    );
    assert!(
        fx.run("need_no_second", false).await,
        "what a delegated run tried to write must not reach the node's private state"
    );
}

#[actix::test]
async fn a_delegated_run_that_removes_private_state_fails_and_leaves_the_nodes_state() {
    let fx = fixture().await;
    assert!(fx.run("write_first", false).await);
    assert!(
        !fx.run("remove_first", true).await,
        "a removal that would be discarded must fail the run"
    );
    assert!(
        fx.run("need_first", false).await,
        "a delegated removal must not touch the node's private state"
    );
}
