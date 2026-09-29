//! Guest-controlled memory, table and threading shapes must stay inside the
//! limits the runtime is configured with, whatever the module declares.

#![allow(clippy::unwrap_used)]

use calimero_account::AccountId;
use calimero_runtime::logic::{Outcome, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{Engine, Module};

fn compile(wat: &str, limits: VMLimits) -> Module {
    let wasm = wat::parse_str(wat).unwrap();

    Engine::with_limits(limits).compile(&wasm).unwrap()
}

fn run(module: &Module, method: &str) -> Outcome {
    let mut storage = InMemoryStorage::default();

    module
        .run(
            [0; 32].into(),
            AccountId::from([0; 32]),
            [0; 32].into(),
            method,
            &[],
            &mut storage,
            None,
            None,
        )
        .expect("run must return an Outcome")
}

/// `grow` returns `-1` for a refused growth; the guest traps when it is not.
fn grow_refused(pages: u32) -> String {
    format!(
        r#"(func (export "grow_refused")
               (if (i32.ne (memory.grow (i32.const {pages})) (i32.const -1))
                   (then unreachable)))"#
    )
}

#[test]
fn memory_growth_past_max_memory_pages_is_refused() {
    let limits = VMLimits {
        max_memory_pages: 4,
        ..VMLimits::default()
    };
    let wat = format!(
        r#"(module (memory (export "memory") 1) {})"#,
        grow_refused(8)
    );

    let outcome = run(&compile(&wat, limits), "grow_refused");

    assert!(outcome.returns.is_ok(), "{:?}", outcome.returns);
}

#[test]
fn memory_growth_past_max_memory_pages_is_refused_despite_a_larger_declared_maximum() {
    let limits = VMLimits {
        max_memory_pages: 4,
        ..VMLimits::default()
    };
    let wat = format!(
        r#"(module (memory (export "memory") 1 65536) {})"#,
        grow_refused(8)
    );

    let outcome = run(&compile(&wat, limits), "grow_refused");

    assert!(outcome.returns.is_ok(), "{:?}", outcome.returns);
}

#[test]
fn memory_growth_within_max_memory_pages_is_allowed() {
    let limits = VMLimits {
        max_memory_pages: 4,
        ..VMLimits::default()
    };
    let wat = r#"(module
        (memory (export "memory") 1)
        (func (export "grow_within")
            (if (i32.eq (memory.grow (i32.const 3)) (i32.const -1)) (then unreachable))))"#;

    let outcome = run(&compile(wat, limits), "grow_within");

    assert!(outcome.returns.is_ok(), "{:?}", outcome.returns);
}

#[test]
fn a_smaller_declared_memory_maximum_is_still_honoured() {
    let limits = VMLimits {
        max_memory_pages: 4,
        ..VMLimits::default()
    };
    let wat = format!(
        r#"(module (memory (export "memory") 1 2) {})"#,
        grow_refused(2)
    );

    let outcome = run(&compile(&wat, limits), "grow_refused");

    assert!(outcome.returns.is_ok(), "{:?}", outcome.returns);
}

#[test]
fn a_memory_minimum_above_max_memory_pages_is_not_instantiated() {
    let limits = VMLimits {
        max_memory_pages: 4,
        ..VMLimits::default()
    };
    let wat = r#"(module (memory (export "memory") 5) (func (export "noop")))"#;

    let outcome = run(&compile(wat, limits), "noop");

    assert!(outcome.returns.is_err(), "{:?}", outcome.returns);
}

/// `table.grow` returns `-1` for a refused growth; the guest traps when it is not.
fn table_grow_refused(delta: u32) -> String {
    format!(
        r#"(module
            (memory (export "memory") 1)
            (table 1 funcref)
            (func (export "grow_refused")
                (if (i32.ne (table.grow (ref.null func) (i32.const {delta})) (i32.const -1))
                    (then unreachable))))"#
    )
}

#[test]
fn table_growth_is_capped() {
    let wat = table_grow_refused(10_000_000);

    let outcome = run(&compile(&wat, VMLimits::default()), "grow_refused");

    assert!(outcome.returns.is_ok(), "{:?}", outcome.returns);
}

#[test]
fn table_growth_is_capped_at_the_configured_maximum() {
    let limits = VMLimits {
        max_table_elements: 8,
        ..VMLimits::default()
    };
    let wat = table_grow_refused(8);

    let outcome = run(&compile(&wat, limits), "grow_refused");

    assert!(outcome.returns.is_ok(), "{:?}", outcome.returns);
}

#[test]
fn table_growth_up_to_the_configured_maximum_is_allowed() {
    let limits = VMLimits {
        max_table_elements: 8,
        ..VMLimits::default()
    };
    let wat = r#"(module
        (memory (export "memory") 1)
        (table 1 funcref)
        (func (export "grow_within")
            (if (i32.eq (table.grow (ref.null func) (i32.const 7)) (i32.const -1))
                (then unreachable))))"#;

    let outcome = run(&compile(wat, limits), "grow_within");

    assert!(outcome.returns.is_ok(), "{:?}", outcome.returns);
}

#[test]
fn a_smaller_declared_table_maximum_is_still_honoured() {
    let limits = VMLimits {
        max_table_elements: 8,
        ..VMLimits::default()
    };
    let wat = r#"(module
        (memory (export "memory") 1)
        (table 1 2 funcref)
        (func (export "grow_refused")
            (if (i32.ne (table.grow (ref.null func) (i32.const 2)) (i32.const -1))
                (then unreachable))))"#;

    let outcome = run(&compile(wat, limits), "grow_refused");

    assert!(outcome.returns.is_ok(), "{:?}", outcome.returns);
}

#[test]
fn a_table_minimum_above_the_maximum_is_not_instantiated() {
    let limits = VMLimits {
        max_table_elements: 8,
        ..VMLimits::default()
    };
    let wat = r#"(module
        (memory (export "memory") 1)
        (table 9 funcref)
        (func (export "noop")))"#;

    let outcome = run(&compile(wat, limits), "noop");

    assert!(outcome.returns.is_err(), "{:?}", outcome.returns);
}

const SHARED_ATOMIC_WAIT_WAT: &str = r#"(module
    (memory (export "memory") 1 1 shared)
    (func (export "wait")
        (drop (memory.atomic.wait32 (i32.const 0) (i32.const 0) (i64.const -1)))))"#;

#[test]
fn a_guest_with_shared_memory_and_atomic_wait_is_rejected_at_compile() {
    let wasm = wat::parse_str(SHARED_ATOMIC_WAIT_WAT).unwrap();

    let result = Engine::default().compile(&wasm);

    assert!(result.is_err(), "{result:?}");
}

#[test]
fn a_shared_memory_is_not_instantiated_by_an_engine_that_compiled_it() {
    let wat = r#"(module (memory (export "memory") 1 1 shared) (func (export "noop")))"#;
    let wasm = wat::parse_str(wat).unwrap();
    let engine = Engine::new(wasmer::Engine::default(), VMLimits::default());
    let module = engine.compile(&wasm).unwrap();

    let outcome = run(&module, "noop");

    assert!(outcome.returns.is_err(), "{:?}", outcome.returns);
}

/// Bytes a threads-enabled engine produced, as a node's precompiled cache would
/// hold them. Such an engine is the only way a shared memory or atomic wait
/// reaches `from_precompiled`, since the runtime's own engines refuse to
/// compile them.
fn precompiled_with_threads(wat: &str) -> Vec<u8> {
    let wasm = wat::parse_str(wat).unwrap();
    let engine = wasmer::Engine::default();
    let module = wasmer::Module::new(&engine, wasm).unwrap();

    module.serialize().unwrap().to_vec()
}

/// Loads `bytes` through a headless engine and runs `method`, on a thread so a
/// guest that blocks fails the test instead of hanging it.
fn run_precompiled(bytes: Vec<u8>, method: &'static str) -> Result<Outcome, String> {
    let (tx, rx) = std::sync::mpsc::channel();

    let _detached = std::thread::spawn(move || {
        let engine = Engine::headless();
        // SAFETY: the bytes were produced by wasmer in this process.
        let result = unsafe { engine.from_precompiled(&bytes) }
            .map_err(|err| format!("{err:?}"))
            .map(|module| run(&module, method));

        drop(tx.send(result));
    });

    rx.recv_timeout(std::time::Duration::from_secs(30))
        .unwrap_or_else(|_| panic!("`{method}` did not finish: the guest is blocking"))
}

#[test]
fn a_precompiled_shared_memory_is_not_instantiated() {
    let bytes = precompiled_with_threads(
        r#"(module (memory (export "memory") 1 1 shared) (func (export "noop")))"#,
    );

    let result = run_precompiled(bytes, "noop");

    // Refusing at load is as good as refusing at run; running it is not.
    if let Ok(outcome) = result {
        assert!(outcome.returns.is_err(), "{:?}", outcome.returns);
    }
}

#[test]
fn a_precompiled_atomic_wait_does_not_block() {
    // A wait on a non-shared memory: wasmer traps it, which this pins (the run
    // must end, in a trap, rather than block).
    let bytes = precompiled_with_threads(
        r#"(module
            (memory (export "memory") 1 1)
            (func (export "wait")
                (drop (memory.atomic.wait32 (i32.const 0) (i32.const 0) (i64.const -1)))))"#,
    );

    let result = run_precompiled(bytes, "wait");

    if let Ok(outcome) = result {
        assert!(outcome.returns.is_err(), "{:?}", outcome.returns);
    }
}
