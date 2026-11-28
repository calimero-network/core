//! The cluster tests' sandbox fixture: `analyze` copies its inputs into the sandbox's
//! workspace, runs a Python script over them, and takes `out.txt` out as the run's output;
//! `onDone` keeps the output and records what the run said.

use std::io::Read;

use calimero_sdk::blob::BlobRef;
use calimero_sdk::compute::{self, ComputeOutcome, ComputeStatus, RunId};
use calimero_sdk::prelude::*;
use calimero_sdk::sandbox::{self, Command};

#[app::error]
pub enum Error {
    ScriptFailed { exit: i32, stderr: String },
}

#[app::state(version = 1)]
pub struct Sandbox {
    /// The outputs kept, by id: a reference in state retains one.
    kept: UnorderedMap<String, BlobRef>,
    /// What each run's outcome said, by run id: JSON text.
    outcomes: UnorderedMap<String, String>,
}

#[app::logic]
impl Sandbox {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Writes each input to the workspace as `input-<i>`, runs `script` with `python3`, and
    /// collects `out.txt` when the script wrote one. Returns the script's stdout.
    #[app::sandbox(
        fuel = 20_000_000_000,
        memory_mib = 256,
        wall_secs = 120,
        result = "on_done",
        vcpus = 1,
        sandbox_memory_mib = 512,
        workspace_mib = 64,
        image = "host"
    )]
    pub fn analyze(script: String) -> app::Result<String> {
        for (i, id) in compute::inputs().iter().enumerate() {
            let mut bytes = Vec::new();
            let _ = calimero_sdk::files::open(id)?.read_to_end(&mut bytes)?;
            sandbox::write(&format!("input-{i}"), &bytes)?;
        }
        sandbox::write("script.py", script.as_bytes())?;
        compute::progress(50, "running the script");
        let run = sandbox::exec(&Command::new(["python3", "script.py"]).timeout_ms(60_000))?;
        if !run.success() {
            app::bail!(Error::ScriptFailed {
                exit: run.exit_code,
                stderr: String::from_utf8_lossy(&run.stderr).into_owned(),
            });
        }
        let listing = sandbox::exec(&Command::sh("test -f out.txt && echo yes || echo no"))?;
        if listing.stdout_text().trim() == "yes" {
            let _ = sandbox::collect("out.txt")?;
        }
        Ok(run.stdout_text())
    }

    /// Starts an `analyze` run of `script` over `inputs`.
    pub fn start_analyze(&mut self, script: String, inputs: Vec<BlobRef>) -> app::Result<RunId> {
        compute::start("analyze", &serde_json::json!({ "script": script }), &inputs)
    }

    /// Receives the outcome of a run, keeps its outputs and records what it said.
    #[app::compute_result]
    pub fn on_done(&mut self, outcome: ComputeOutcome) -> app::Result<()> {
        let status = match outcome.status {
            ComputeStatus::Done => "done",
            ComputeStatus::Failed => "failed",
            ComputeStatus::Cancelled => "cancelled",
        };
        if outcome.status == ComputeStatus::Done {
            for output in &outcome.outputs {
                self.kept.insert(&output.id.id(), &output.id)?;
            }
        }
        let text = serde_json::json!({
            "run": outcome.run,
            "status": status,
            "reason": outcome.reason,
            "attempt": outcome.attempt,
            "result": outcome.result,
            "outputs": outcome.outputs.iter().map(|o| o.id.id()).collect::<Vec<_>>(),
        });
        self.outcomes.insert(&outcome.run.to_string(), &text.to_string())?;
        Ok(())
    }

    /// What run `run`'s outcome said, as JSON text, once it is in.
    #[app::view]
    pub fn outcome(&self, run: RunId) -> app::Result<Option<String>> {
        Ok(self.outcomes.get(&run.to_string())?)
    }
}
