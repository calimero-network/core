//! `mero-compute run`: one compute method on a compute component alone, with the engine, caps
//! and determinism of a job node and plain files in place of sealed ones.

mod files;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, PoisonError};

use base64::Engine as _;
use calimero_app_engine_wasmtime::{ending_of, ComputeRun, Ending, Measured, WasmtimeEngine};
use calimero_app_host::{
    Caller, ComputeCaps, Engine, MethodKind, Outcome, PrepareError, Prepared, Progress, Status,
};
use clap::{Parser, Subcommand};
use eyre::{bail, eyre, Result};

use files::{Input, LocalFiles, Produced};

const START_TS_MS: u64 = 1_800_000_000_000; // the fixed clock of every run
const DEFAULT_SEED: [u8; 32] = [0; 32]; // the randomness seed without `--seed`
const IDENTITY: &str = "local"; // the app, run and owner of a local run
const SECOND_DIR: &str = ".second"; // where `--twice` writes its second run, removed after
const BYTES_PER_GIB: f64 = 1_073_741_824.0;

#[derive(Parser)]
#[command(name = "mero-compute", version)]
#[command(about = "Test a compute component alone, as a job node would run it")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run one compute method
    Run {
        /// The compute component (`compute.component.wasm` from `mero-app-build`)
        component: PathBuf,
        /// The compute method, by its ABI name
        #[arg(long)]
        method: String,
        /// The method's arguments: JSON text, or `@file`
        #[arg(long)]
        args: Option<String>,
        /// An input file; repeat for several, in the order the run sees them
        #[arg(long = "input")]
        inputs: Vec<PathBuf>,
        /// Where outputs go, as `output-<i>` in the order the run finished them
        #[arg(long)]
        out: PathBuf,
        /// The randomness seed, 64 hex digits
        #[arg(long)]
        seed: Option<String>,
        /// Run twice and fail if the outputs or the result differ
        #[arg(long)]
        twice: bool,
    },
}

/// What one run left.
struct Ran {
    outcome: Outcome,
    measured: Measured,
    outputs: Vec<Produced>,
}

/// Everything the two runs of `--twice` share.
struct Setup {
    engine: WasmtimeEngine,
    prepared: Prepared,
    method: String,
    caps: ComputeCaps,
    args: Vec<u8>,
    seed: [u8; 32],
    inputs: Vec<Input>,
}

fn main() -> ExitCode {
    let Command::Run {
        component,
        method,
        args,
        inputs,
        out,
        seed,
        twice,
    } = Cli::parse().command;
    match run(
        &component,
        method,
        args,
        &inputs,
        &out,
        seed.as_deref(),
        twice,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(
    component: &Path,
    method: String,
    args: Option<String>,
    inputs: &[PathBuf],
    out: &Path,
    seed: Option<&str>,
    twice: bool,
) -> Result<()> {
    let engine = WasmtimeEngine::for_compute()?;
    let prepared = prepare(&engine, component)?;
    let caps = caps_of(&prepared, &method)?;
    let setup = Setup {
        engine,
        prepared,
        method,
        caps,
        args: read_args(args)?,
        seed: seed.map_or(Ok(DEFAULT_SEED), parse_seed)?,
        inputs: inputs
            .iter()
            .map(|p| Input::read(p))
            .collect::<Result<_>>()?,
    };

    clear_outputs(out)?;
    let first = setup.run(out, true)?;
    if let Err(reason) = settle(&first) {
        for o in &first.outputs {
            let _ = std::fs::remove_file(&o.path);
        }
        bail!("{reason}");
    }
    report(&first);
    if twice {
        let scratch = out.join(SECOND_DIR);
        let second = setup.run(&scratch, false);
        let _ = std::fs::remove_dir_all(&scratch);
        let second = second?;
        settle(&second).map_err(|reason| eyre!("the second run {reason}"))?;
        let differences = differences(
            (&first.outcome.result, &first.outputs),
            (&second.outcome.result, &second.outputs),
        );
        if !differences.is_empty() {
            bail!("the two runs differ: {}", differences.join("; "));
        }
        println!("both runs are identical");
    }
    Ok(())
}

impl Setup {
    /// One run writing its outputs into `dir`, its progress printed as it is reported if `live`.
    fn run(&self, dir: &Path, live: bool) -> Result<Ran> {
        std::fs::create_dir_all(dir)?;
        let files = Arc::new(LocalFiles::new(self.inputs.clone(), dir.to_owned()));
        let produced = files.produced();
        let progress: Progress = Arc::new(move |percent, step| {
            if live {
                println!("progress: {percent}% {step}");
                let _ = std::io::stdout().flush();
            }
        });
        let (outcome, measured) = self.engine.run_compute(
            &self.prepared,
            ComputeRun {
                app: IDENTITY.to_owned(),
                run: IDENTITY.to_owned(),
                owner: IDENTITY.to_owned(),
                method: self.method.clone(),
                args: self.args.clone(),
                caller: Caller::System,
                start_ts_ms: START_TS_MS,
                seed: self.seed,
                inputs: self.inputs.iter().map(|i| i.id.clone()).collect(),
                caps: self.caps.clone(),
            },
            files,
            progress,
            Arc::new(AtomicBool::new(false)),
        )?;
        let outputs = std::mem::take(&mut *produced.lock().unwrap_or_else(PoisonError::into_inner));
        Ok(Ran {
            outcome,
            measured,
            outputs,
        })
    }
}

/// Removes the `output-*` files an earlier run left in `out`, so only this run's outputs remain.
fn clear_outputs(out: &Path) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(out) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("output-") {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// `--args` as bytes: JSON text, or the content of the file after an `@`.
fn read_args(args: Option<String>) -> Result<Vec<u8>> {
    Ok(match args {
        None => Vec::new(),
        Some(a) => match a.strip_prefix('@') {
            Some(file) => std::fs::read(file).map_err(|e| eyre!("cannot read {file}: {e}"))?,
            None => a.into_bytes(),
        },
    })
}

/// Prepares the component on the compute engine, which refuses an app component.
fn prepare(engine: &WasmtimeEngine, component: &Path) -> Result<Prepared> {
    let bytes =
        std::fs::read(component).map_err(|e| eyre!("cannot read {}: {e}", component.display()))?;
    engine.prepare("local", &bytes).map_err(|e| match e {
        PrepareError::ForbiddenImport(import) => eyre!(
            "{} is not a compute component: it imports {import}, which a compute run cannot use \
             (an app component? mero-app-build writes the compute one as compute.component.wasm \
             beside --out)",
            component.display()
        ),
        other => eyre!("{} cannot be prepared: {other}", component.display()),
    })
}

/// The caps the component's ABI declares for `method`.
fn caps_of(prepared: &Prepared, method: &str) -> Result<ComputeCaps> {
    let methods = &prepared.abi.methods;
    match methods.get(method).map(|m| &m.kind) {
        Some(MethodKind::Compute(caps)) => Ok(caps.clone()),
        _ => {
            let known: Vec<&str> = methods
                .iter()
                .filter(|(_, m)| matches!(m.kind, MethodKind::Compute(_)))
                .map(|(name, _)| name.as_str())
                .collect();
            bail!(
                "the component has no compute method {method:?}; it has: {}",
                known.join(", ")
            )
        }
    }
}

fn parse_seed(text: &str) -> Result<[u8; 32]> {
    hex::decode(text)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| eyre!("--seed is 64 hex digits (32 bytes)"))
}

/// Why a run did not end `Ok`, named as a job node's report names it.
fn settle(ran: &Ran) -> std::result::Result<(), String> {
    let status = &ran.outcome.status;
    let reason = match ending_of(status) {
        Ending::Done => return Ok(()),
        Ending::Failed(reason) => reason.as_str(),
        Ending::Lost => "lost",
    };
    let detail = match status {
        Status::Trap { detail, .. } => format!(" ({detail})"),
        Status::AppError { code, message } => format!(" ({code}: {message})"),
        Status::Ok => String::new(),
    };
    Err(format!("failed: {reason}{detail}"))
}

/// What differs between two runs of one assignment: their result and their outputs.
fn differences(a: (&[u8], &[Produced]), b: (&[u8], &[Produced])) -> Vec<String> {
    let mut out = Vec::new();
    if a.0 != b.0 {
        out.push("the result differs".to_owned());
    }
    if a.1.len() != b.1.len() {
        out.push(format!("{} outputs against {}", a.1.len(), b.1.len()));
    }
    for (i, (x, y)) in a.1.iter().zip(b.1).enumerate() {
        if (x.size, &x.sha256) != (y.size, &y.sha256) {
            out.push(format!("output-{i} differs"));
        }
    }
    out
}

fn report(ran: &Ran) {
    let result = &ran.outcome.result;
    println!(
        "result (base64): {}",
        base64::engine::general_purpose::STANDARD.encode(result)
    );
    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(result) {
        println!("result: {json:#}");
    }
    for o in &ran.outputs {
        println!("output: {} ({} bytes)", o.path.display(), o.size);
    }
    let usage = &ran.outcome.usage;
    let gib_seconds = usage.peak_memory as f64 * ran.measured.wall.as_secs_f64() / BYTES_PER_GIB;
    println!(
        "usage: {:.3} cpu seconds, {gib_seconds:.6} GiB-seconds, {} fuel",
        ran.measured.cpu.as_secs_f64(),
        usage.fuel
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(size: u64, hash: u8) -> Produced {
        Produced {
            path: PathBuf::new(),
            size,
            sha256: vec![hash; 32],
        }
    }

    #[test]
    fn identical_runs_differ_in_nothing() {
        let outputs = [output(3, 1), output(4, 2)];
        assert!(differences((b"r", &outputs), (b"r", &outputs)).is_empty());
    }

    #[test]
    fn differing_runs_are_told_apart_by_what_differs() {
        let (a, b) = ([output(3, 1), output(4, 2)], [output(3, 1), output(4, 9)]);
        assert_eq!(differences((b"r", &a), (b"r", &b)), ["output-1 differs"]);
        assert_eq!(
            differences((b"r", &a), (b"x", &a[..1])),
            ["the result differs", "2 outputs against 1"]
        );
    }
}
