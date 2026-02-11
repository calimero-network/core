//! Integration tests for the panic hook. Run the merod binary in a subprocess with
//! MEROD_TEST_PANIC=1 or MEROD_TEST_PANIC=string so it panics after installing color_eyre.
//! We assert on the subprocess stderr.
//!
//! **Why subprocess?** An in-process test that calls `panic!("...")` is flaky because:
//! - The test process actually panics; without `catch_unwind` the test fails.
//! - With `catch_unwind`, the panic hook runs in the same process and may interact
//!   with the test harness or stderr in non-deterministic ways.
//! Running the real binary in a subprocess and using `Command::output()` gives
//! deterministic, full stderr capture and no in-process panic.

#[test]
fn test_panic_hook_logs_structured_info() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_merod"))
        .env("MEROD_TEST_PANIC", "1")
        .output()
        .expect("failed to run merod");

    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("test panic message"),
        "stderr should contain panic message; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("main.rs"),
        "stderr should contain panic location (main.rs); stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("panic.line="),
        "stderr should contain structured field panic.line; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("panic.backtrace="),
        "stderr should contain structured field panic.backtrace; stderr:\n{stderr}"
    );
}

#[test]
fn test_panic_hook_handles_string_payload() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_merod"))
        .env("MEROD_TEST_PANIC", "string")
        .output()
        .expect("failed to run merod");

    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("string payload panic"),
        "stderr should contain String panic message; stderr:\n{stderr}"
    );
}
