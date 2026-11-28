//! The cluster tests' sandbox fixture on the real engine: its sandbox component prepares only on
//! a sandbox engine, and `analyze` drives the run's sandbox through the WIT `sandbox` interface:
//! its inputs are written into the workspace, the script runs, and the collected file becomes
//! one of the run's sealed outputs. The sandbox here is a stand-in that records what it is asked;
//! the Linux sandbox itself is `calimero-sandbox`'s, with its own escape suite.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use calimero_app_build::{build_in_repo_cache, BuiltApp};
use calimero_app_engine_wasmtime::{ComputeRun, WasmtimeEngine};
use calimero_app_host::{
    Caller, ComputeCaps, Engine, MemoryFiles, MethodKind, PrepareError, Progress, SandboxExec,
    SandboxExecResult, SandboxFailure, SandboxHost, Status, TrapKind, SANDBOX_UNAVAILABLE,
};

fn fixture(name: &str) -> BuiltApp {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps")
        .join(name);
    build_in_repo_cache(&dir).unwrap_or_else(|e| panic!("build {name}: {e}"))
}

/// A sandbox whose workspace is a map and whose `exec` answers what the test set.
#[derive(Default)]
struct FakeSandbox {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    execs: Mutex<Vec<SandboxExec>>,
    /// `exec` writes `out.txt` with this and prints it.
    output: Vec<u8>,
    exit_code: i32,
    unavailable: bool,
}

impl SandboxHost for FakeSandbox {
    fn exec(&self, req: &SandboxExec) -> Result<SandboxExecResult, SandboxFailure> {
        if self.unavailable {
            return Err(SandboxFailure::Unavailable);
        }
        self.execs.lock().unwrap().push(req.clone());
        let stdout = if req.argv.first().is_some_and(|a| a == "/bin/sh") {
            let has_out = self.files.lock().unwrap().contains_key("out.txt");
            if has_out {
                b"yes\n".to_vec()
            } else {
                b"no\n".to_vec()
            }
        } else {
            let _ = self
                .files
                .lock()
                .unwrap()
                .insert("out.txt".to_owned(), self.output.clone());
            self.output.clone()
        };
        Ok(SandboxExecResult {
            exit_code: self.exit_code,
            stdout,
            stderr: b"err".to_vec(),
            ..SandboxExecResult::default()
        })
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), SandboxFailure> {
        if path.contains("..") {
            return Err(SandboxFailure::Refused(format!(
                "{path}: leaves the workspace"
            )));
        }
        let _ = self
            .files
            .lock()
            .unwrap()
            .insert(path.to_owned(), bytes.to_vec());
        Ok(())
    }

    fn read(&self, path: &str, max: u32) -> Result<Vec<u8>, SandboxFailure> {
        let files = self.files.lock().unwrap();
        let bytes = files
            .get(path)
            .ok_or_else(|| SandboxFailure::Refused(format!("{path}: no such file")))?;
        Ok(bytes.iter().copied().take(max as usize).collect())
    }

    fn open(&self, path: &str) -> Result<Box<dyn Read + Send>, SandboxFailure> {
        let bytes = self.read(path, u32::MAX)?;
        Ok(Box::new(Cursor::new(bytes)))
    }
}

fn caps(app: &BuiltApp) -> ComputeCaps {
    let abi = WasmtimeEngine::for_sandbox()
        .unwrap()
        .abi_of(&app.sandbox.as_ref().unwrap().component)
        .unwrap();
    let MethodKind::Compute(caps) = &abi.methods["analyze"].kind else {
        panic!("analyze is a sandbox method");
    };
    caps.clone()
}

fn run(
    app: &BuiltApp,
    files: &MemoryFiles,
    inputs: Vec<String>,
    sandbox: Arc<FakeSandbox>,
) -> calimero_app_host::Outcome {
    let engine = WasmtimeEngine::for_sandbox().unwrap();
    let prepared = engine
        .prepare("sandbox-hash", &app.sandbox.as_ref().unwrap().component)
        .unwrap();
    let progress: Progress = Arc::new(|_, _| {});
    let (outcome, _) = engine
        .run_compute(
            &prepared,
            ComputeRun {
                app: "app".to_owned(),
                run: "run-1".to_owned(),
                owner: "owner".to_owned(),
                method: "analyze".to_owned(),
                args: br#"{"script":"print(1)"}"#.to_vec(),
                caller: Caller::System,
                start_ts_ms: 1_700_000_000_000,
                seed: [3; 32],
                inputs,
                caps: caps(app),
                keep_logs: false,
                sandbox: Some(sandbox),
            },
            Arc::new(files.clone()),
            progress,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    outcome
}

#[test]
fn a_sandbox_component_prepares_on_a_sandbox_engine_only() {
    let sandbox_app = fixture("fixture-sandbox");
    let compute_app = fixture("fixture-compute");
    let sandbox = &sandbox_app
        .sandbox
        .as_ref()
        .expect("a sandbox component")
        .component;
    assert!(sandbox_app.compute.is_none());
    let on_sandbox = WasmtimeEngine::for_sandbox().unwrap();
    assert!(on_sandbox.prepare("s", sandbox).is_ok());
    let compute = &compute_app.compute.as_ref().unwrap().component;
    // Each component's imports and ABI world name one engine; every other refuses it.
    assert!(matches!(
        on_sandbox.prepare("c", compute),
        Err(PrepareError::BadAbi(_))
    ));
    assert!(matches!(
        WasmtimeEngine::for_compute().unwrap().prepare("s", sandbox),
        Err(PrepareError::ForbiddenImport(i)) if i.starts_with("calimero:app/sandbox@")
    ));
    let cache = tempfile::tempdir().unwrap();
    let replica = WasmtimeEngine::new(cache.path().to_path_buf(), &[1; 32]).unwrap();
    assert!(matches!(
        replica.prepare("s", sandbox),
        Err(PrepareError::ForbiddenImport(_))
    ));
}

#[test]
fn a_sandbox_run_writes_its_inputs_runs_the_script_and_seals_the_collected_file() {
    let app = fixture("fixture-sandbox");
    let files = MemoryFiles::default();
    let input = files.insert(b"alice,3\nbob,5\n");
    let sandbox = Arc::new(FakeSandbox {
        output: b"8\n".to_vec(),
        ..FakeSandbox::default()
    });
    let outcome = run(&app, &files, vec![input], Arc::clone(&sandbox));
    assert_eq!(outcome.status, Status::Ok, "{outcome:?}");
    assert_eq!(
        serde_json::from_slice::<String>(&outcome.result).unwrap(),
        "8\n"
    );
    let written = sandbox.files.lock().unwrap().clone();
    assert_eq!(written["input-0"], b"alice,3\nbob,5\n");
    assert_eq!(written["script.py"], b"print(1)");
    let execs = sandbox.execs.lock().unwrap();
    assert_eq!(execs[0].argv, ["python3", "script.py"]);
    assert_eq!(execs[0].timeout_ms, 60_000);
    assert_eq!(
        outcome.new_files.len(),
        1,
        "out.txt is the run's one output"
    );
    files.apply(&outcome.new_files);
    assert_eq!(files.get(&outcome.new_files[0].id).unwrap(), b"8\n");
    assert!(outcome.usage.fuel > 0);
}

#[test]
fn a_failing_script_is_the_apps_error_and_keeps_no_output() {
    let app = fixture("fixture-sandbox");
    let files = MemoryFiles::default();
    let sandbox = Arc::new(FakeSandbox {
        output: b"partial".to_vec(),
        exit_code: 1,
        ..FakeSandbox::default()
    });
    let outcome = run(&app, &files, Vec::new(), sandbox);
    assert!(
        matches!(&outcome.status, Status::AppError { code, .. } if code == "ScriptFailed"),
        "{:?}",
        outcome.status
    );
    assert!(outcome.new_files.is_empty());
}

#[test]
fn a_sandbox_the_node_cannot_serve_loses_the_attempt() {
    let app = fixture("fixture-sandbox");
    let files = MemoryFiles::default();
    let sandbox = Arc::new(FakeSandbox {
        unavailable: true,
        ..FakeSandbox::default()
    });
    let outcome = run(&app, &files, Vec::new(), sandbox);
    assert_eq!(
        outcome.status,
        Status::Trap {
            kind: TrapKind::Unavailable,
            detail: SANDBOX_UNAVAILABLE.to_owned(),
        }
    );
}
