//! The compute runs an app can start and cancel from a logged run, behind the node's replica of
//! the app's run records. The host core talks only to [`Runs`]; the node provides it.

use sha2::{Digest, Sha256};

use crate::abi::ComputeCaps;

/// Input files one run may name.
pub const MAX_RUN_INPUTS: usize = 64;
/// Declared plaintext bytes of all the inputs of one run.
pub const MAX_RUN_INPUT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Bytes of a compute run's arguments; they live in the run's record.
pub const MAX_COMPUTE_ARGS_BYTES: usize = 64 * 1024;
/// Runs one logged run may start.
pub const MAX_STARTS_PER_RUN: usize = 16;
/// Runs an app may have active (queued or running) at once.
pub const MAX_ACTIVE_RUNS: usize = 64;

/// The soft errors of `compute.start` and `compute.cancel` (the WIT `result<_, string>`),
/// fixed so every engine and SDK returns and matches the same strings.
pub const NOT_A_COMPUTE_METHOD: &str = "not a compute method";
pub const RUN_ARGS_TOO_LARGE: &str = "arguments too large";
pub const RUN_TOO_MANY_INPUTS: &str = "too many inputs";
pub const RUN_INPUTS_TOO_LARGE: &str = "inputs too large";
pub const RUN_DUPLICATE_INPUT: &str = "input named twice";
pub const RUN_INPUT_MISSING: &str = "input is not a file of this app";
pub const RUN_TOO_MANY_STARTS: &str = "too many runs started";
pub const RUN_TOO_MANY_ACTIVE: &str = "too many active runs";
pub const RUN_RECORDS_UNREADABLE: &str = "run records are unreadable";
pub const NO_ACTIVE_RUN: &str = "no such active run";

/// An active run as the app's records hold it: what makes a second start the same run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveRun {
    pub run: String,
    pub method: String,
    /// SHA-256 of the arguments.
    pub args_sha256: Vec<u8>,
    pub inputs: Vec<String>,
    /// A cancel is recorded: the run is ending, so it is no one's answer to a start, but it still
    /// counts toward the cap and its inputs stay protected.
    pub cancel_requested: bool,
}

impl ActiveRun {
    /// Whether `other` asks for the same job: the same method, arguments and inputs in order.
    #[must_use]
    pub fn same_job(&self, other: &Self) -> bool {
        self.method == other.method
            && self.args_sha256 == other.args_sha256
            && self.inputs == other.inputs
    }
}

/// An input file of a started run, as its mark records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunInput {
    pub id: String,
    pub size: u64,
    pub sha256: Vec<u8>,
}

/// A run a logged run started; it reaches the outcome only if the run ends `Ok`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedRun {
    pub run: String,
    pub method: String,
    pub args: Vec<u8>,
    pub inputs: Vec<RunInput>,
    /// The method's caps, as the running bundle's ABI declares them.
    pub caps: ComputeCaps,
}

impl StartedRun {
    /// The run as the records will hold it, once applied.
    #[must_use]
    pub fn to_active(&self) -> ActiveRun {
        ActiveRun {
            run: self.run.clone(),
            method: self.method.clone(),
            args_sha256: Sha256::digest(&self.args).to_vec(),
            inputs: self.inputs.iter().map(|i| i.id.clone()).collect(),
            cancel_requested: false,
        }
    }
}

/// The app's active runs, as one invocation sees them.
pub trait Runs: Send + Sync {
    /// The active runs in a fixed order; asked at most once per invocation. A record that cannot
    /// be read is an error, never a run left out.
    fn active(&self) -> Result<Vec<ActiveRun>, String>;
}

/// The provider of an app with no run records.
pub struct NoRuns;

impl Runs for NoRuns {
    fn active(&self) -> Result<Vec<ActiveRun>, String> {
        Ok(Vec::new())
    }
}
