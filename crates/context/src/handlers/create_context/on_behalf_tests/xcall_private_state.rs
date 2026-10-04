//! Whether a run made for someone else reaches this node's private state
//! through an xcall it queues. The xcall is dispatched as the node's own run,
//! so its target runs over the node's `PrivateState` column, not the empty
//! store the delegated run itself was given.

use std::cell::Cell;
use std::future::Future;
use std::time::Duration;

use calimero_context_client::messages::{ExecuteError, ExecuteResponse};
use calimero_wasm_abi::schema::{Manifest, Method, MethodIntent};

use super::*;

/// What `stash` writes to private state, and so what `publish` commits as the root.
const SECRET: [u8; 32] = [0xA5; 32];

/// `stash` and `publish` are the app's xcall entry points: `stash` writes
/// [`SECRET`] to private state, `publish` commits the private value as the
/// context's root and traps when there is none. `kick_stash` and `kick_publish`
/// queue an xcall to them on this same context, and `peek_publish` is a view
/// that does what `kick_publish` does. `has_stash` traps unless the private
/// value is present.
///
/// Layout: a 32-byte buffer at 0, the artifact byte at 32, `{ptr, len}`
/// descriptors from 64 (the buffer, an empty artifact, a one-byte artifact, the
/// private key, the private value), the two xcall descriptors at 160 and 208,
/// then the key, the value and the two method names.
fn module() -> Vec<u8> {
    let bytes = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|b| format!("\\{b:02x}"))
            .collect::<String>()
    };
    let desc = |ptr: u64, len: u64| bytes(&[ptr.to_le_bytes(), len.to_le_bytes()].concat());
    let xcall = |name: u64, len: u64| [desc(0, 32), desc(name, len), desc(0, 0)].concat();
    let wat = format!(
        r#"
    (module
        (import "env" "account_id" (func $account_id (param i64)))
        (import "env" "context_id" (func $context_id (param i64)))
        (import "env" "read_register" (func $read_register (param i64 i64) (result i32)))
        (import "env" "commit" (func $commit (param i64 i64)))
        (import "env" "xcall" (func $xcall (param i64)))
        (import "env" "private_storage_read" (func $read (param i64 i64) (result i32)))
        (import "env" "private_storage_write" (func $write (param i64 i64) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 32) "\01")
        (data (i32.const 64) "{buffer}{no_artifact}{artifact}{key_desc}{value_desc}")
        (data (i32.const 160) "{publish}{stash}")
        (data (i32.const 512) "{key}")
        (data (i32.const 576) "{value}")
        (data (i32.const 640) "publish")
        (data (i32.const 656) "stash")
        (func $queue (param $call i64)
            (call $context_id (i64.const 0))
            (drop (call $read_register (i64.const 0) (i64.const 64)))
            (call $xcall (local.get $call)))
        (func (export "init")
            (call $account_id (i64.const 0))
            (drop (call $read_register (i64.const 0) (i64.const 64)))
            (call $commit (i64.const 64) (i64.const 80)))
        (func (export "kick_publish") (call $queue (i64.const 160)))
        (func (export "peek_publish") (call $queue (i64.const 160)))
        (func (export "kick_stash") (call $queue (i64.const 208)))
        (func (export "stash")
            (drop (call $write (i64.const 112) (i64.const 128)))
            (call $account_id (i64.const 0))
            (drop (call $read_register (i64.const 0) (i64.const 64)))
            (call $commit (i64.const 64) (i64.const 96)))
        (func (export "publish")
            (if (i32.eqz (call $read (i64.const 112) (i64.const 0))) (then unreachable))
            (drop (call $read_register (i64.const 0) (i64.const 64)))
            (call $commit (i64.const 64) (i64.const 96)))
        (func (export "has_stash")
            (if (i32.eqz (call $read (i64.const 112) (i64.const 0))) (then unreachable))))
"#,
        buffer = desc(0, 32),
        no_artifact = desc(32, 0),
        artifact = desc(32, 1),
        key_desc = desc(512, 33),
        value_desc = desc(576, 32),
        publish = xcall(640, 7),
        stash = xcall(656, 5),
        key = bytes(&[0x11; 33]),
        value = bytes(&SECRET),
    );
    let entry_point = |name: &str| Method {
        name: name.to_owned(),
        xcall_callable: true,
        ..Method::default()
    };
    let manifest = Manifest {
        // Sorted by name, as the manifest's validation requires.
        methods: vec![
            Method {
                name: "peek_publish".to_owned(),
                intent: MethodIntent::ReadOnly,
                ..Method::default()
            },
            entry_point("publish"),
            entry_point("stash"),
        ],
        ..Manifest::default()
    };
    let wasm = wat::parse_str(wat).expect("parse the module");
    calimero_wasm_abi::embed::write_embedded_state_schema(&wasm, &manifest).expect("embed the abi")
}

struct Relay {
    fx: Fixture,
    context: ContextId,
    nonce: Cell<u64>,
}

/// A context created for the author, on a relay that is a `Member` holding
/// `CAN_AUTHOR_ON_BEHALF` (a self-hosted `--delegated-access` node).
async fn relay() -> Relay {
    let fx = fixture_of(Standing::RelayTee, true, module()).await;
    let created = fx.create(fx.delegation()).await.expect("create");
    MembershipRepository::new(&fx.store)
        .set_role(&fx.group, &fx.relay, GroupMemberRole::Member)
        .expect("the relay is now a plain member");
    CapabilitiesRepository::new(&fx.store)
        .set_member_capability(
            &fx.group,
            &fx.relay,
            MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
        )
        .expect("the relay may author on behalf");
    Relay {
        fx,
        context: created.context_id,
        nonce: Cell::new(1),
    }
}

fn completed(result: Result<ExecuteResponse, ExecuteError>) -> bool {
    result.is_ok_and(|response| response.returns.is_ok())
}

/// Whether `condition` came to hold while the detached xcall batch had time to run.
async fn came_to_hold<F: Future<Output = bool>>(condition: impl Fn() -> F) -> bool {
    for _ in 0..50 {
        if condition().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

impl Relay {
    /// Run `method` as this node, for itself.
    async fn own(&self, method: &str) -> bool {
        completed(
            self.fx
                .harness
                .context_client
                .execute(
                    &self.context,
                    &self.fx.relay_pk,
                    method.to_owned(),
                    b"{}".to_vec(),
                    None,
                )
                .await,
        )
    }

    /// Run `method` under a warrant `author_sk` signed for this relay.
    async fn delegated(&self, author_sk: &PrivateKey, method: &str) -> bool {
        let args = b"{}".to_vec();
        self.nonce.set(self.nonce.get() + 1);
        let author = author_sk.public_key();
        let warrant = calimero_account::Warrant::sign(
            author_sk,
            calimero_account::WarrantTerms {
                context: self.context,
                author_account: crate::test_support::account_for(&author),
                executor: self.fx.relay,
                app_version: self.fx.application_id,
                method: method.to_owned(),
                intent_hash: calimero_account::Warrant::intent_hash(method, &args),
                account_heads: vec![],
                governance_floor: vec![],
                nonce: self.nonce.get(),
                not_after: u64::MAX,
            },
        )
        .expect("sign");
        let delegation = calimero_account::Delegation {
            warrant: Box::new(warrant),
            author_proof: credential(&author),
            executor_proof: crate::join_credential::build(
                &self.fx.store,
                &self.fx.group,
                &self.fx.relay_pk,
            )
            .expect("this node's credential"),
            executor_key: self.fx.relay_pk,
        };
        completed(
            self.fx
                .harness
                .context_client
                .execute_with_origin(
                    &self.context,
                    &self.fx.relay_pk,
                    method.to_owned(),
                    args,
                    None,
                    None,
                    0,
                    Some(Box::new(delegation)),
                )
                .await,
        )
    }

    /// Run the view `method` as `account`, as an authenticated session's read is.
    async fn read_as(&self, account: calimero_account::AccountId, method: &str) -> bool {
        completed(
            self.fx
                .harness
                .context_client
                .query_as(
                    &self.context,
                    account,
                    &self.fx.relay_pk,
                    method.to_owned(),
                    b"{}".to_vec(),
                )
                .await,
        )
    }

    /// A second member with no node here, as the author is.
    fn another_member(&self) -> PrivateKey {
        let sk = PrivateKey::from([0x55; 32]);
        let account = enrol(&self.fx.store, &self.fx.group, &sk.public_key());
        MembershipRepository::new(&self.fx.store)
            .add_member(&self.fx.group, &account, GroupMemberRole::Member)
            .expect("seat the second member");
        sk
    }
}

#[actix::test]
async fn an_xcall_a_delegated_run_queues_does_not_write_the_nodes_private_state() {
    let relay = relay().await;
    let author = &relay.fx.author_sk;

    assert!(
        !relay.delegated(author, "stash").await,
        "control: the same write, called directly under the warrant, fails the run"
    );
    assert!(!relay.own("has_stash").await, "control: nothing stored yet");

    assert!(relay.delegated(author, "kick_stash").await);
    assert!(
        !came_to_hold(|| relay.own("has_stash")).await,
        "the xcall a delegated run queued wrote the node's private state"
    );
}

#[actix::test]
async fn an_xcall_a_delegated_run_queues_does_not_read_the_nodes_private_state() {
    let relay = relay().await;
    let author = &relay.fx.author_sk;

    assert!(relay.own("stash").await, "the node stores its own secret");
    assert_ne!(relay.fx.root(), SECRET);
    assert!(
        !relay.delegated(author, "publish").await,
        "control: the same read, called directly under the warrant, finds nothing"
    );

    assert!(relay.delegated(author, "kick_publish").await);
    assert!(
        !came_to_hold(|| async { relay.fx.root() == SECRET }).await,
        "the xcall a delegated run queued copied the node's private value into the context's root"
    );
}

#[actix::test]
async fn one_delegated_account_does_not_reach_another_through_xcalls() {
    let relay = relay().await;
    let author = &relay.fx.author_sk;
    let other = relay.another_member();

    assert!(relay.delegated(author, "kick_stash").await);
    let _stored = came_to_hold(|| relay.own("has_stash")).await;

    assert!(relay.delegated(&other, "kick_publish").await);
    assert!(
        !came_to_hold(|| async { relay.fx.root() == SECRET }).await,
        "what one account's xcall stored privately, another account's xcall published"
    );
}

#[actix::test]
async fn an_xcall_a_session_read_queues_does_not_read_the_nodes_private_state() {
    let relay = relay().await;

    assert!(relay.own("stash").await, "the node stores its own secret");
    assert_ne!(relay.fx.root(), SECRET);

    assert!(relay.read_as(relay.fx.author, "peek_publish").await);
    assert!(
        !came_to_hold(|| async { relay.fx.root() == SECRET }).await,
        "the xcall a session's read queued wrote the node's private value into the context's root"
    );
}

/// The harness itself: an xcall the node queues for itself runs its target.
#[actix::test]
async fn an_xcall_the_node_queues_for_itself_writes_its_private_state() {
    let relay = relay().await;
    assert!(relay.own("kick_stash").await);
    assert!(came_to_hold(|| relay.own("has_stash")).await);
}
