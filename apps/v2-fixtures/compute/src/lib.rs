//! The cluster tests' compute fixture: `digest` streams every input and writes one output,
//! and `onDone` receives the run's outcome. The output is the input XOR the run's seeded
//! random stream, so two runs of one assignment write the same bytes. `spin`, `hog`, `burn`
//! and `peek` each break one limit of a run.

use std::io::{Read, Write};

use calimero_app_sdk::files::FileId;
use calimero_app_sdk::compute::{self, ComputeOutcome, ComputeStatus, RunId};
use calimero_app_sdk::prelude::*;
use sha2::{Digest, Sha256};

/// One read or write call's size.
const PIECE: usize = 64 * 1024;

#[app::state(version = 1)]
pub struct Compute {
    done: Counter,
    /// What each run's outcome said, by run id: JSON text.
    outcomes: UnorderedMap<String, String>,
}

#[app::logic]
impl Compute {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Streams every input into one output (input XOR a seeded keystream) and returns the
    /// SHA-256 of each input in order.
    #[app::compute(fuel = 20_000_000_000, memory_mib = 256, wall_secs = 60, result = "on_done")]
    pub fn digest() -> app::Result<Vec<String>> {
        let inputs = compute::inputs();
        let mut writer = calimero_app_sdk::files::create()?;
        let mut hashes = Vec::new();
        for (i, id) in inputs.iter().enumerate() {
            let mut reader = calimero_app_sdk::files::open(id)?;
            let mut hash = Sha256::new();
            let mut buf = vec![0u8; PIECE];
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hash.update(&buf[..n]);
                let key = env::random_bytes(n);
                let out: Vec<u8> = buf[..n].iter().zip(&key).map(|(b, k)| b ^ k).collect();
                writer.write_all(&out)?;
            }
            hashes.push(hex(&hash.finalize()));
            compute::progress(percent(i + 1, inputs.len()), "hashing");
        }
        let _: FileId = writer.finish()?;
        Ok(hashes)
    }

    /// Loops until the wall clock ends it.
    #[app::compute(fuel = 1_000_000_000_000, memory_mib = 64, wall_secs = 2, result = "on_done")]
    pub fn spin() -> app::Result<()> {
        let mut n = 0u64;
        loop {
            n = std::hint::black_box(n.wrapping_add(1));
        }
    }

    /// Grows memory until the run's memory cap refuses it.
    #[app::compute(fuel = 20_000_000_000, memory_mib = 64, wall_secs = 30, result = "on_done")]
    pub fn hog() -> app::Result<u64> {
        let mut held: Vec<Vec<u8>> = Vec::new();
        loop {
            held.push(std::hint::black_box(vec![1u8; 1 << 20]));
        }
    }

    /// Loops until its small fuel cap ends it.
    #[app::compute(fuel = 1_000_000, memory_mib = 64, wall_secs = 30, result = "on_done")]
    pub fn burn() -> app::Result<()> {
        let mut n = 0u64;
        loop {
            n = std::hint::black_box(n.wrapping_add(1));
        }
    }

    /// Returns more bytes than a run may hand back.
    #[app::compute(fuel = 20_000_000_000, memory_mib = 64, wall_secs = 30, result = "on_done")]
    pub fn big() -> app::Result<Vec<u8>> {
        Ok(vec![7u8; 70_000])
    }

    /// Opens the file `id`, which a test names outside the run's inputs.
    #[app::compute(fuel = 20_000_000_000, memory_mib = 64, wall_secs = 30, result = "on_done")]
    pub fn peek(id: FileId) -> app::Result<u64> {
        let _reader = calimero_app_sdk::files::open(&id)?;
        Ok(0)
    }

    /// Starts a `digest` run over `inputs`; the same inputs while it is active answer its id.
    pub fn start_digest(&mut self, inputs: Vec<FileId>) -> app::Result<RunId> {
        compute::start("digest", &(), &inputs)
    }

    /// Starts the argument-less compute method `method` with no input.
    pub fn start_run(&mut self, method: String) -> app::Result<RunId> {
        compute::start(&method, &(), &[])
    }

    /// Asks for the active run `run` to end.
    pub fn cancel(&mut self, run: RunId) -> app::Result<()> {
        compute::cancel(&run)
    }

    /// Receives the outcome of a run and records what it said.
    #[app::compute_result]
    pub fn on_done(&mut self, outcome: ComputeOutcome) -> app::Result<()> {
        self.done.add(1)?;
        let status = match outcome.status {
            ComputeStatus::Done => "done",
            ComputeStatus::Failed => "failed",
            ComputeStatus::Cancelled => "cancelled",
        };
        let text = serde_json::json!({
            "run": outcome.run,
            "status": status,
            "reason": outcome.reason,
            "attempt": outcome.attempt,
            "sha256": outcome.outputs.first().map(|o| o.sha256.clone()),
            "outputs": outcome.outputs.len(),
            "fuel": outcome.usage.fuel.to_string(),
            "cpuSeconds": outcome.usage.cpu_seconds,
        });
        self.outcomes.insert(&outcome.run.to_string(), &text.to_string())?;
        Ok(())
    }

    /// The outcome recorded for `run`, as JSON text, if its result method ran.
    #[app::view]
    pub fn outcome(&self, run: RunId) -> app::Result<Option<String>> {
        Ok(self.outcomes.get(&run.to_string())?)
    }

    /// How many outcomes `onDone` has received.
    #[app::view]
    pub fn done(&self) -> app::Result<u64> {
        Ok(self.done.get()?)
    }
}

fn percent(done: usize, total: usize) -> u8 {
    u8::try_from(done * 100 / total.max(1)).unwrap_or(100)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
