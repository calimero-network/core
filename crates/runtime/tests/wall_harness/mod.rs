//! The harness the gas-wall probes share: build a guest, call it, and tell a
//! measured wall apart from a broken call site. A directory module so cargo
//! does not compile it as a test target of its own.

// Each probe uses a different subset of this module.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use calimero_account::AccountId;
use calimero_runtime::errors::{FunctionCallError, MethodResolutionError};
use calimero_runtime::logic::{Outcome, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{Engine, Module};

pub const PROBE_STRIDE: usize = 100; // characters between two progress lines of a sweep

/// Build `apps/<app>` and return the wasm bytes. The build runs every time:
/// cargo already knows the whole dependency graph these probes measure, and a
/// local mtime check over the app's own sources does not.
pub fn guest_wasm(app: &str) -> Vec<u8> {
    let app_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("workspace root")
        .join("apps")
        .join(app);
    let wasm_path = app_dir.join(format!("res/{}.wasm", app.replace('-', "_")));

    let output = Command::new(env!("CARGO"))
        .args([
            "run",
            "-q",
            "-p",
            "cargo-mero",
            "--",
            "mero",
            "build",
            "--manifest-path",
        ])
        .arg(app_dir.join("Cargo.toml"))
        .output()
        .expect("failed to spawn cargo mero build");
    assert!(
        output.status.success(),
        "building {app} wasm failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    std::fs::read(&wasm_path).unwrap_or_else(|e| panic!("{}: {e}", wasm_path.display()))
}

pub fn call(
    module: &Module,
    storage: &mut InMemoryStorage,
    method: &str,
    args: &serde_json::Value,
) -> Outcome {
    module
        .run(
            [0_u8; 32].into(),
            AccountId::from([0_u8; 32]),
            [0_u8; 32].into(),
            method,
            &serde_json::to_vec(args).expect("encode args"),
            storage,
            None,
            None,
        )
        .expect("run must return an Outcome")
}

pub enum Verdict {
    Wall { limit: u64 },
    Drift(String),
}

/// The guest a probe drives, and the gate text its drift message points at.
pub struct Probe {
    pub app: &'static str,
    pub gate: &'static str,
}

impl Probe {
    /// In these probes `GasExhausted` is the only failure that counts as a wall.
    pub fn classify(&self, method: &str, error: &FunctionCallError) -> Verdict {
        match error {
            FunctionCallError::GasExhausted { limit } => Verdict::Wall { limit: *limit },
            FunctionCallError::ExecutionError(payload) => Verdict::Drift(format!(
                "{method} returned an application error: {}\n\
                 The usual cause is an argument this probe passes that the app no longer \
                 takes. Compare the call site below against {}'s current \
                 #[app::logic] signature.",
                String::from_utf8_lossy(payload),
                self.app,
            )),
            other => Verdict::Drift(format!("{method} failed: {other}")),
        }
    }

    pub fn drift(&self, detail: &str) -> ! {
        panic!(
            "\n\
             ==================== CONTRACT DRIFT - NOT A STORAGE RESULT ====================\n\
             {detail}\n\
             \n\
             Nothing has been measured and no wall has been found; fix the call site in \
             this file, then rerun.\n\
             {}\
             ==============================================================================\n",
            self.gate
        )
    }

    pub fn expect_ok(&self, outcome: &Outcome, method: &str) {
        if let Err(error) = &outcome.returns {
            match self.classify(method, error) {
                Verdict::Drift(detail) => self.drift(&detail),
                Verdict::Wall { limit } => self.drift(&format!(
                    "{method} exhausted its {limit}-point gas budget on the FIRST call, \
                     against an empty store. That is not a wall - a wall needs data \
                     behind it. Either max_gas has been lowered dramatically or the app \
                     now does unbounded work at n=0."
                )),
            }
        }
    }

    pub fn decode<T: serde::de::DeserializeOwned>(&self, outcome: &Outcome, method: &str) -> T {
        let body = outcome
            .returns
            .as_ref()
            .unwrap_or_else(|_| self.drift(&format!("{method} failed")))
            .as_ref()
            .unwrap_or_else(|| self.drift(&format!("{method} returned no value at all")));
        serde_json::from_slice(body)
            .unwrap_or_else(|e| self.drift(&format!("{method} returned unexpected JSON: {e}")))
    }
}

/// Requires a guest exposing `init` and `insert_text`.
pub fn mid_document_typing_wall(
    probe: &Probe,
    wasm: &[u8],
    ceiling: usize,
    preflight: impl FnOnce(&Module),
) {
    let limits = VMLimits::default();
    println!("guest:   real {} app, built from this tree", probe.app);
    println!("max_gas: {:?}", limits.max_gas);
    println!("position: landed / 2 (mid-document), NOT an append");

    let module = Engine::with_limits(limits)
        .compile(wasm)
        .expect("compile metered module");

    preflight(&module);

    let mut storage = InMemoryStorage::default();
    probe.expect_ok(
        &call(&module, &mut storage, "init", &serde_json::json!({})),
        "init",
    );

    let mut landed = 0_usize;
    let mut write_wall: Option<usize> = None;

    println!("\n  n        insert_gas       i_reads   ms");

    for _ in 0..ceiling {
        let started = Instant::now();
        let outcome = call(
            &module,
            &mut storage,
            "insert_text",
            &serde_json::json!({"position": landed / 2, "text": "x"}),
        );
        let write_ms = started.elapsed().as_secs_f64() * 1000.0;

        if let Err(error) = &outcome.returns {
            match probe.classify("insert_text", error) {
                Verdict::Wall { limit } => {
                    println!("\nMID-DOCUMENT WRITE WALL at {landed} (gas limit {limit})");
                    write_wall = Some(landed);
                    break;
                }
                Verdict::Drift(detail) => probe.drift(&format!(
                    "{detail}\nThis appeared only after {landed} characters, so it is \
                     state-dependent rather than a stale call signature."
                )),
            }
        }
        landed += 1;

        if landed.is_multiple_of(PROBE_STRIDE) {
            println!(
                "  {landed:<6}  {:>12?}  {:>7}  {write_ms:>5.1}",
                outcome.gas_used, outcome.storage_reads,
            );
        }
    }

    println!("\n--- result ---");
    println!("characters landed:  {landed}");
    match write_wall {
        Some(n) => println!("mid-document write wall (insert_text at landed/2): {n}"),
        None => println!("mid-document write wall: none below {ceiling}"),
    }

    assert!(
        landed > 0,
        "no character was inserted even though preflight succeeded - the failure \
         classification in this file is broken"
    );
    if let Some(n) = write_wall {
        assert!(
            n >= 50,
            "walled after only {n} mid-document characters. Preflight passed, so the \
             calls are well-formed, but a ceiling this low is a change in the app's work \
             per call, not the cost curve this probe exists to measure."
        );
    }
}

#[test]
fn only_gas_exhaustion_counts_as_a_wall() {
    let probe = Probe {
        app: "fugue-editor",
        gate: "",
    };

    assert!(
        matches!(
            probe.classify(
                "insert_text",
                &FunctionCallError::GasExhausted { limit: 1_000 }
            ),
            Verdict::Wall { limit: 1_000 }
        ),
        "gas exhaustion is the one failure that is a measurement"
    );

    let Verdict::Drift(detail) = probe.classify(
        "insert_text",
        &FunctionCallError::ExecutionError(b"missing field `text`".to_vec()),
    ) else {
        panic!(
            "an application error was classified as a wall - the probe would report \
             a fictional number"
        );
    };
    assert!(
        detail.contains("text"),
        "the drift message must carry the app's own complaint, got: {detail}"
    );

    let Verdict::Drift(detail) = probe.classify(
        "char_at",
        &FunctionCallError::MethodResolutionError(MethodResolutionError::MethodNotFound {
            name: "char_at".to_owned(),
        }),
    ) else {
        panic!("a renamed method was classified as a wall");
    };
    assert!(
        detail.contains("char_at"),
        "the drift message must name the method, got: {detail}"
    );
}
