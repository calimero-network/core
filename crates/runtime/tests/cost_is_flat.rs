//! Cost regressions are invisible to correctness tests. This one sees them.
//!
//! # Why this exists
//!
//! A storage collection shipped with an append whose cost grew with the
//! collection's size, until a single write exhausted the gas limit and the
//! collection became permanently unwritable (core#3602). The full storage suite
//! — 715 tests — was green throughout, because every one of them asserts what
//! the code *computes*, and none asserts what it *costs*.
//!
//! Three separate O(n) regressions were then introduced while fixing the first,
//! each hidden behind a call that reads as free at the call site: `len()`, a
//! cache populate, and a log line asking a trie to enumerate rather than count.
//!
//! # What it asserts, and why in these units
//!
//! Gas is charged per executed wasm operator and NOTHING else — a host storage
//! read costs the ~4 operators of the caller, independent of value size, and
//! there is no read counter or read limit in the VM. So an O(n) read pattern
//! never registers as "reads are expensive"; it registers as gas spent
//! borsh-decoding whatever those reads returned.
//!
//! That makes `storage_reads` the leading indicator and `gas_used` the
//! confirming one, so this asserts on both: reads must not grow with n, and
//! neither must gas.
//!
//! # The shape of the guest
//!
//! Deliberately not a real contract. A contract drags in the whole collections
//! stack and would make a failure ambiguous between "the app regressed" and
//! "storage regressed". This guest performs a fixed number of host reads per
//! call regardless of how much data exists, so ANY growth in the measured
//! counters is a defect in the layer under test, not in the fixture.

use calimero_account::AccountId;
use calimero_runtime::logic::{Outcome, VMLimits};
use calimero_runtime::store::{InMemoryStorage, Storage};
use calimero_runtime::Engine;

/// A guest that reads one key `n` times, where `n` arrives as the register
/// contents of key `"n"`. Fixed work per call: the loop count comes from the
/// input, so the fixture can hold work constant while the STORE grows.
const FIXED_WORK_WAT: &str = r#"
(module
  (import "env" "storage_read" (func $storage_read (param i64 i64) (result i32)))
  (import "env" "read_register" (func $read_register (param i64 i64) (result i32)))
  (import "env" "register_len"  (func $register_len (param i64) (result i64)))
  (memory (export "memory") 2)

  ;; key "k" at offset 64, described by a Buffer{ptr,len} at offset 0
  (data (i32.const 0)  "\40\00\00\00\00\00\00\00\01\00\00\00\00\00\00\00")
  (data (i32.const 64) "k")

  (func (export "fixed_reads")
    (local $i i32)
    (local.set $i (i32.const 0))
    (block $done
      (loop $again
        (br_if $done (i32.ge_u (local.get $i) (i32.const 64)))
        (drop (call $storage_read (i64.const 0) (i64.const 0)))
        (drop (call $read_register (i64.const 0) (i64.const 1024)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $again))))
)
"#;

fn run_with_store_size(entries: usize, value_len: usize) -> Outcome {
    let wasm = wat::parse_str(FIXED_WORK_WAT).expect("parse WAT");
    let module = Engine::with_limits(VMLimits::default())
        .compile(&wasm)
        .expect("compile metered module");

    let mut storage = InMemoryStorage::default();
    // The key the guest reads.
    storage.set(b"k".to_vec(), vec![7_u8; value_len]);
    // Bulk the store out around it. The guest never touches these — they exist
    // so that a lookup which secretly scans, rather than seeking, shows up.
    for i in 0..entries {
        storage.set(format!("pad{i:08}").into_bytes(), vec![1_u8; value_len]);
    }

    module
        .run(
            [0_u8; 32].into(),
            AccountId::from([0_u8; 32]),
            [0_u8; 32].into(),
            "fixed_reads",
            &[],
            &mut storage,
            None,
            None,
        )
        .expect("run must return an Outcome")
}

#[test]
fn fixed_guest_work_costs_the_same_however_much_data_exists() {
    let small = run_with_store_size(10, 64);
    let large = run_with_store_size(10_000, 64);

    println!(
        "  10 entries: gas={:?} reads={} read_bytes={}",
        small.gas_used, small.storage_reads, small.storage_read_bytes
    );
    println!(
        "  10k entries: gas={:?} reads={} read_bytes={}",
        large.gas_used, large.storage_reads, large.storage_read_bytes
    );

    assert!(
        small.returns.is_ok(),
        "small run failed: {:?}",
        small.returns
    );
    assert!(
        large.returns.is_ok(),
        "large run failed: {:?}",
        large.returns
    );

    assert_eq!(
        small.storage_reads, large.storage_reads,
        "a guest doing fixed work must not read more because the store grew"
    );
    assert_eq!(
        small.gas_used, large.gas_used,
        "…and must not burn more gas either"
    );
    assert_eq!(
        small.storage_reads, 64,
        "the fixture should do exactly 64 reads"
    );
}

#[test]
fn read_bytes_scale_with_value_size_but_gas_does_not() {
    // The counterintuitive property worth pinning: gas is per wasm operator, so
    // a read of a 16 KiB value costs the same gas as a read of 64 B. Only
    // `storage_read_bytes` reflects the difference.
    //
    // This is why a redesign toward fewer, larger rows would look free in gas
    // while costing real wall-clock and real guest decode work. Anyone reading
    // gas alone would draw the wrong conclusion.
    let small = run_with_store_size(10, 64);
    let big = run_with_store_size(10, 16_384);

    println!(
        "  64B values:  gas={:?} read_bytes={}",
        small.gas_used, small.storage_read_bytes
    );
    println!(
        "  16KiB values: gas={:?} read_bytes={}",
        big.gas_used, big.storage_read_bytes
    );

    assert!(
        big.storage_read_bytes > small.storage_read_bytes * 100,
        "read_bytes must track value size: {} vs {}",
        small.storage_read_bytes,
        big.storage_read_bytes
    );
    assert_eq!(
        small.gas_used, big.gas_used,
        "gas must NOT track value size — it prices wasm operators, not bytes. \
         If this ever fails, the metering model changed and every cost \
         assumption in the storage design needs revisiting."
    );
}
