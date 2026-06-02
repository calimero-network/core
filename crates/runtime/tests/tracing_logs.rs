//! End-to-end test for the host-backed `tracing` subscriber (SDK `tracing`
//! feature).
//!
//! `tracing` is a facade: with no subscriber installed in the WASM guest, its
//! macros (`info!`/`debug!`/…) — including those inside crates the app imports,
//! notably `calimero_storage` — are dropped before formatting. The SDK now
//! installs a subscriber whose writer forwards each formatted line through the
//! host `log_utf8` function, so the output lands in the execution `Outcome`.
//!
//! This drives a REAL compiled app (`apps/scaffolding-e2e`, built with the
//! `tracing` feature) through the actual `Module::run` path and asserts:
//!   * app-level `tracing` events reach `outcome.logs` with their level
//!     rendered;
//!   * the level filter drops events below the active level (DEBUG hidden at
//!     the default INFO);
//!   * raising the level to DEBUG surfaces `calimero_storage`'s OWN `tracing`
//!     output — proving logs from a crate the app merely *imports* (not just
//!     the app's own) reach the host.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use calimero_runtime::logic::{Outcome, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{Engine, Module};
use serde_json::{json, to_vec as to_json_vec, Value};

const CTX: [u8; 32] = [9u8; 32];
const EXEC: [u8; 32] = [3u8; 32];

fn workspace_root() -> PathBuf {
    // crates/runtime/ -> ../../
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Build the scaffolding-e2e app once per test-binary run and return its wasm.
/// Rebuilds only when the binary is missing or older than `src/lib.rs` (a
/// coarse but adequate freshness check for a fixture).
fn scaffolding_wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        let app_dir = workspace_root().join("apps/scaffolding-e2e");
        let wasm_path = app_dir.join("res/scaffolding_e2e.wasm");

        let wasm_mtime = std::fs::metadata(&wasm_path)
            .and_then(|m| m.modified())
            .ok();
        let src_mtime = std::fs::metadata(app_dir.join("src/lib.rs"))
            .and_then(|m| m.modified())
            .ok();
        let needs_build = match (wasm_mtime, src_mtime) {
            (Some(w), Some(s)) => w < s,
            _ => true,
        };
        if needs_build {
            let output = Command::new("bash")
                .arg(app_dir.join("build.sh"))
                .output()
                .expect("failed to spawn build.sh — is bash on PATH?");
            assert!(
                output.status.success(),
                "building scaffolding-e2e wasm failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        std::fs::read(&wasm_path).expect("scaffolding_e2e.wasm not found after build")
    })
}

fn engine_module() -> &'static (Engine, Module) {
    static EM: OnceLock<(Engine, Module)> = OnceLock::new();
    EM.get_or_init(|| {
        // DEBUG-level storage logging on the scaffolding root (many collections)
        // easily exceeds the 100-log default, which would trap with
        // `LogsOverflow`. Raise the cap well past anything a single probe emits.
        let limits = VMLimits {
            max_logs: 100_000,
            ..VMLimits::default()
        };
        let engine = Engine::new(wasmer::Engine::default(), limits);
        let module = engine.compile(scaffolding_wasm()).expect("compile wasm");
        (engine, module)
    })
}

/// `init` on a fresh store, then run `method`; returns the method's `Outcome`.
fn run(method: &str, params: Value) -> Outcome {
    let (_, module) = engine_module();
    let mut store = InMemoryStorage::default();

    module
        .run(
            CTX.into(),
            EXEC.into(),
            "init",
            &to_json_vec(&json!({})).unwrap(),
            &mut store,
            None,
            None,
        )
        .expect("init failed");

    module
        .run(
            CTX.into(),
            EXEC.into(),
            method,
            &to_json_vec(&params).unwrap(),
            &mut store,
            None,
            None,
        )
        .expect("method run failed")
}

#[test]
fn app_tracing_reaches_outcome_and_filters_by_level() {
    // Default level is INFO: info + warn pass, debug is filtered out.
    let outcome = run("tracing_probe", json!({ "debug": false }));
    let logs = outcome.logs;

    let info = logs
        .iter()
        .find(|l| l.contains("tracing_probe: info line"))
        .unwrap_or_else(|| panic!("missing info line; logs: {logs:#?}"));
    assert!(info.contains("INFO"), "level rendered into line: {info:?}");

    assert!(
        logs.iter().any(|l| l.contains("tracing_probe: warn line")),
        "warn line present; logs: {logs:#?}"
    );
    assert!(
        !logs.iter().any(|l| l.contains("tracing_probe: debug line")),
        "debug line must be filtered out at INFO; logs: {logs:#?}"
    );
}

#[test]
fn debug_level_surfaces_storage_crate_tracing() {
    // Raising to DEBUG surfaces both the app's debug line AND the storage
    // crate's own `tracing` output — the imported-dependency case.
    let outcome = run("tracing_probe", json!({ "debug": true }));
    let logs = outcome.logs;

    assert!(
        logs.iter().any(|l| l.contains("tracing_probe: debug line")),
        "app debug line present at DEBUG; logs: {logs:#?}"
    );
    assert!(
        logs.iter().any(|l| l.contains("calimero_storage")),
        "expected at least one log from the storage crate's own tracing \
         (target `calimero_storage::…`); logs: {logs:#?}"
    );
}
