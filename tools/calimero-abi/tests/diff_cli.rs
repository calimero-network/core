//! End-to-end tests for the `calimero-abi diff` CLI: they invoke the built
//! binary against real files and assert the printed output + exit code, covering
//! the `run_diff` path (file I/O, findings printing, exit-code mapping) that the
//! library-level unit tests in `diff.rs` do not exercise.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Write `content` to a unique temp file and return its path.
fn temp_schema(content: &str) -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "mero_abi_diff_{}_{}.json",
        std::process::id(),
        unique
    ));
    std::fs::write(&path, content).expect("write temp schema");
    path
}

/// Run `calimero-abi diff <current> <baseline> [extra]` and return (exit_code, stdout, stderr).
fn run_diff(current: &str, baseline: &str, extra: &[&str]) -> (i32, String, String) {
    let cur = temp_schema(current);
    let base = temp_schema(baseline);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mero-abi"));
    cmd.arg("diff").arg(&cur).arg(&base).args(extra);
    let out = cmd.output().expect("run mero-abi diff");
    let _ = std::fs::remove_file(&cur);
    let _ = std::fs::remove_file(&base);
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn root(fields: &str) -> String {
    format!(
        r#"{{"schema_version":"wasm-abi/1","types":{{"Root":{{"kind":"record","fields":{fields}}}}},"methods":[],"events":[],"state_root":"Root"}}"#
    )
}

const COUNTER_FIELD: &str = r#"{"name":"counter","type":{"kind":"record","fields":[],"crdt_type":"lww_register","inner_type":{"kind":"u64"}}}"#;
const AUTHORED_MAP: &str = r#"{"name":"wiki","type":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"authored_map"}}"#;
const UNORDERED_MAP: &str = r#"{"name":"wiki","type":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"unordered_map"}}"#;
const NOTES_FIELD: &str = r#"{"name":"notes","type":{"kind":"record","fields":[],"crdt_type":"lww_register","inner_type":{"kind":"string"}}}"#;

#[test]
fn cli_additive_exits_zero() {
    let baseline = root(&format!("[{COUNTER_FIELD}]"));
    let current = root(&format!("[{COUNTER_FIELD},{NOTES_FIELD}]"));
    let (code, stdout, _) = run_diff(&current, &baseline, &[]);
    assert_eq!(code, 0, "additive change must exit 0; stdout: {stdout}");
    assert!(stdout.contains("ADDITIVE"), "stdout: {stdout}");
}

#[test]
fn cli_identity_downgrade_exits_one() {
    let baseline = root(&format!("[{AUTHORED_MAP}]"));
    let current = root(&format!("[{UNORDERED_MAP}]"));
    let (code, stdout, _) = run_diff(&current, &baseline, &[]);
    assert_eq!(code, 1, "identity downgrade must fail CI; stdout: {stdout}");
    assert!(
        stdout.contains("UNSAFE_IDENTITY_DOWNGRADE"),
        "stdout: {stdout}"
    );
}

#[test]
fn cli_exit_zero_flag_reports_but_does_not_fail() {
    let baseline = root(&format!("[{AUTHORED_MAP}]"));
    let current = root(&format!("[{UNORDERED_MAP}]"));
    let (code, stdout, _) = run_diff(&current, &baseline, &["--exit-zero"]);
    assert_eq!(code, 0, "--exit-zero must not fail; stdout: {stdout}");
    assert!(
        stdout.contains("UNSAFE_IDENTITY_DOWNGRADE"),
        "still reports the finding; stdout: {stdout}"
    );
}

#[test]
fn cli_broken_baseline_exits_nonzero_with_clear_error() {
    let baseline =
        r#"{"schema_version":"wasm-abi/1","types":{},"methods":[],"events":[]}"#.to_owned();
    let current = root(&format!("[{AUTHORED_MAP}]"));
    let (code, _, stderr) = run_diff(&current, &baseline, &[]);
    assert_ne!(code, 0, "broken baseline must fail closed");
    assert!(
        stderr.contains("state_root"),
        "error should mention state_root; stderr: {stderr}"
    );
}
