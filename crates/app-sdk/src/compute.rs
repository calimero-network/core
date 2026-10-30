//! Compute runs: what a `#[app::compute]` method sees of its run (its id, the files it may read
//! and a way to report progress), and how a call, timer or init starts and cancels runs.
//! Outside a compute run there is no run and no input; `start` and `cancel` work only in a
//! logged write.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{AppError, Result};
use crate::files::FileId;

/// The id of a compute run: opaque, safe to store and to send in JSON.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    /// The id as the host spells it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for RunId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(target_arch = "wasm32")]
mod backend {
    use crate::bindings::calimero::app::compute;

    pub fn run_id() -> Option<String> {
        compute::run_id()
    }

    pub fn inputs() -> Vec<String> {
        compute::inputs()
    }

    pub fn progress(percent: u8, step: &str) {
        compute::progress(percent, step);
    }

    pub fn start(method: &str, args: &[u8], inputs: &[String]) -> Result<String, String> {
        compute::start(method, args, inputs)
    }

    pub fn cancel(run: &str) -> Result<(), String> {
        compute::cancel(run)
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod backend {
    pub fn run_id() -> Option<String> {
        None
    }

    pub fn inputs() -> Vec<String> {
        Vec::new()
    }

    pub fn progress(_percent: u8, _step: &str) {}

    pub fn start(_method: &str, _args: &[u8], _inputs: &[String]) -> Result<String, String> {
        Err("compute runs need a node".to_owned())
    }

    pub fn cancel(_run: &str) -> Result<(), String> {
        Err("compute runs need a node".to_owned())
    }
}

/// The id of this run; none outside a compute run.
pub fn run_id() -> Option<String> {
    backend::run_id()
}

/// The files this run may open, in the order the app gave them.
pub fn inputs() -> Vec<FileId> {
    backend::inputs().into_iter().map(FileId::from).collect()
}

/// Reports progress: `percent` is capped at 100 and `step` cut at 128 bytes. The node shows the
/// last report; it is never logged.
pub fn progress(percent: u8, step: &str) {
    backend::progress(percent, step);
}

/// Starts the compute method `method` on `inputs`, and answers the run's id; the run exists once
/// the call that started it commits, and is queued until a job node takes it. A run with the same
/// method, arguments and inputs that is still active is answered instead of a second one.
pub fn start<A: Serialize + ?Sized>(method: &str, args: &A, inputs: &[FileId]) -> Result<RunId> {
    let args = match serde_json::to_value(args).map_err(|e| fail(e.to_string()))? {
        serde_json::Value::Null => Vec::new(),
        value => value.to_string().into_bytes(),
    };
    let inputs: Vec<String> = inputs.iter().map(|id| id.as_str().to_owned()).collect();
    backend::start(method, &args, &inputs)
        .map(RunId)
        .map_err(fail)
}

/// Asks for an active run to end: its result method then receives a cancelled outcome.
pub fn cancel(run: &RunId) -> Result<()> {
    backend::cancel(run.as_str()).map_err(fail)
}

/// How a compute run ended, as its result method is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputeStatus {
    Done,
    Failed,
    Cancelled,
}

/// A file a run wrote: its id, plaintext size and plaintext SHA-256 (hex).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ComputeOutput {
    pub id: FileId,
    #[serde(with = "crate::json::u64_str")]
    pub size: u64,
    pub sha256: String,
}

/// What a run cost.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputeUsage {
    pub cpu_seconds: f64,
    pub gib_seconds: f64,
    #[serde(with = "crate::json::u64_str")]
    pub fuel: u64,
}

/// The outcome the node hands a `#[app::compute_result]` method: it takes this as its one
/// parameter, whatever its name. `reason` is the failure's name (`fuel`, `memory`, `time`, `trap`,
/// `app-error`, `integrity`, `lost`, `timeout`, `refused`, `quota`) and `result` the compute
/// method's return bytes as standard base64, present only when the run is done.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputeOutcome {
    pub run: RunId,
    pub method: String,
    pub attempt: u32,
    pub status: ComputeStatus,
    pub reason: Option<String>,
    pub result: Option<String>,
    pub outputs: Vec<ComputeOutput>,
    pub usage: ComputeUsage,
}

fn fail(message: String) -> AppError {
    AppError::with_fields("Compute", json!({ "message": message }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outcome_reads_the_nodes_json() {
        let outcome: ComputeOutcome = serde_json::from_value(json!({
            "run": "r1", "method": "digest", "attempt": 2, "status": "failed", "reason": "time",
            "result": null,
            "outputs": [{ "id": "ab", "size": "12", "sha256": "cd" }],
            "usage": { "cpuSeconds": 1.5, "gibSeconds": 0.25, "fuel": "99" },
        }))
        .unwrap();
        assert_eq!(outcome.status, ComputeStatus::Failed);
        assert_eq!(outcome.reason.as_deref(), Some("time"));
        assert_eq!(outcome.outputs[0].size, 12);
        assert_eq!(outcome.usage.fuel, 99);
    }
}
