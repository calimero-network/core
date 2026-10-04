//! A guest cannot do unbounded host work inside a small instruction budget:
//! host calls and the bytes they move are capped per execution.

#![allow(clippy::unwrap_used)]

use calimero_account::AccountId;
use calimero_runtime::errors::{FunctionCallError, HostError};
use calimero_runtime::logic::{Outcome, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;

const CHUNK_LEN: u64 = 32 * 1024; // bytes each host call in the byte loops moves

fn run(wat: &str, limits: VMLimits) -> Outcome {
    let wasm = wat::parse_str(wat).unwrap();
    let module = Engine::with_limits(limits).compile(&wasm).unwrap();
    let mut storage = InMemoryStorage::default();

    module
        .run(
            [0; 32].into(),
            AccountId::from([0; 32]),
            [0; 32].into(),
            "main",
            &[],
            &mut storage,
            None,
            None,
        )
        .expect("run must return an Outcome")
}

/// A host import and the guest statement that calls it once.
type HostCall = (&'static str, &'static str);

/// `register_len`, the cheapest host function.
const REGISTER_LEN: HostCall = (
    r#"(import "env" "register_len" (func $host (param i64) (result i64)))"#,
    "(drop (call $host (i64.const 0)))",
);
/// The host copies the 32-byte context id into a register.
const CONTEXT_ID: HostCall = (
    r#"(import "env" "context_id" (func $host (param i64)))"#,
    "(call $host (i64.const 0))",
);
/// The host writes `CHUNK_LEN` random bytes into guest memory.
const RANDOM_BYTES: HostCall = (
    r#"(import "env" "random_bytes" (func $host (param i64)))"#,
    "(call $host (i64.const 0))",
);
/// The host reads a `CHUNK_LEN` log line out of guest memory.
const LOG_UTF8: HostCall = (
    r#"(import "env" "log_utf8" (func $host (param i64)))"#,
    "(call $host (i64.const 0))",
);
/// The host reads a `CHUNK_LEN` QuickJS debug print, given as a raw pointer and length.
const DEBUG_PRINT: HostCall = (
    r#"(import "env" "js_std_d_print" (func $host (param i64 i64 i64) (result i32)))"#,
    "(drop (call $host (i64.const 0) (i64.const 16) (i64.load (i32.const 8))))",
);

/// Makes `host` `calls` times, with a buffer descriptor `{ptr: 16, len: CHUNK_LEN}` at 0.
fn host_loop((import, call): HostCall, calls: u64) -> String {
    format!(
        r#"(module
            {import}
            (memory (export "memory") 1)
            (func (export "main") (local $i i64)
                (i64.store (i32.const 0) (i64.const 16))
                (i64.store (i32.const 8) (i64.const {CHUNK_LEN}))
                (loop $again
                    {call}
                    (local.set $i (i64.add (local.get $i) (i64.const 1)))
                    (br_if $again (i64.lt_u (local.get $i) (i64.const {calls}))))))"#
    )
}

fn assert_call_cap_refused(outcome: &Outcome, max: u64) {
    assert!(
        matches!(
            &outcome.returns,
            Err(FunctionCallError::HostError(HostError::HostCallLimitExceeded { max: m })) if *m == max
        ),
        "{:?}",
        outcome.returns
    );
}

fn assert_byte_cap_refused(outcome: &Outcome) {
    assert!(
        matches!(
            outcome.returns,
            Err(FunctionCallError::HostError(
                HostError::HostBytesLimitExceeded { .. }
            ))
        ),
        "{:?}",
        outcome.returns
    );
}

#[test]
fn a_cheap_host_call_looped_past_the_cap_is_refused() {
    let limits = VMLimits {
        max_host_calls: 100,
        ..VMLimits::default()
    };

    let outcome = run(&host_loop(REGISTER_LEN, 1_000), limits);

    assert_call_cap_refused(&outcome, 100);
}

#[test]
fn host_calls_up_to_the_cap_are_allowed() {
    let limits = VMLimits {
        max_host_calls: 100,
        ..VMLimits::default()
    };

    let outcome = run(&host_loop(REGISTER_LEN, 100), limits);

    assert!(outcome.returns.is_ok(), "{:?}", outcome.returns);
}

#[test]
fn the_default_call_cap_refuses_a_cheap_loop_well_inside_the_gas_budget() {
    let max = VMLimits::default().max_host_calls;

    let outcome = run(&host_loop(REGISTER_LEN, max + 1), VMLimits::default());

    assert_call_cap_refused(&outcome, max);
}

#[test]
fn bytes_written_into_the_guest_past_the_byte_cap_are_refused() {
    let limits = VMLimits {
        max_host_bytes: CHUNK_LEN + CHUNK_LEN / 2,
        ..VMLimits::default()
    };

    let outcome = run(&host_loop(RANDOM_BYTES, 2), limits);

    assert_byte_cap_refused(&outcome);
}

#[test]
fn bytes_up_to_the_byte_cap_are_allowed() {
    let limits = VMLimits {
        max_host_bytes: 2 * CHUNK_LEN,
        ..VMLimits::default()
    };

    let outcome = run(&host_loop(RANDOM_BYTES, 2), limits);

    assert!(outcome.returns.is_ok(), "{:?}", outcome.returns);
}

/// Two `CHUNK_LEN` reads out of guest memory under a cap of one and a half.
fn read_past_the_byte_cap(host: HostCall) -> Outcome {
    let limits = VMLimits {
        max_host_bytes: CHUNK_LEN + CHUNK_LEN / 2,
        max_log_size: CHUNK_LEN,
        ..VMLimits::default()
    };

    run(&host_loop(host, 2), limits)
}

#[test]
fn bytes_read_from_a_guest_buffer_past_the_byte_cap_are_refused() {
    assert_byte_cap_refused(&read_past_the_byte_cap(LOG_UTF8));
}

#[test]
fn bytes_read_from_a_raw_guest_pointer_past_the_byte_cap_are_refused() {
    assert_byte_cap_refused(&read_past_the_byte_cap(DEBUG_PRINT));
}

#[test]
fn bytes_copied_into_registers_past_the_byte_cap_are_refused() {
    let limits = VMLimits {
        max_host_bytes: 48,
        ..VMLimits::default()
    };

    let outcome = run(&host_loop(CONTEXT_ID, 2), limits);

    assert_byte_cap_refused(&outcome);
}
