//! The cluster tests' compute fixture: `digest` streams every input and writes one output,
//! and `onDone` receives the run's outcome and keeps its outputs by `BlobRef`, which is what
//! retains them. The output is the input XOR the run's seeded random stream, so two runs of
//! one assignment write the same bytes. `spin`, `hog`, `burn` and `peek` each break one
//! limit of a run.

use std::io::{Read, Write};

use calimero_sdk::blob::BlobRef;
use calimero_sdk::compute::{self, ComputeOutcome, ComputeStatus, RunId};
use calimero_sdk::prelude::*;
use sha2::{Digest, Sha256};

/// One read or write call's size.
const PIECE: usize = 64 * 1024;

#[app::state(version = 1)]
pub struct Compute {
    done: Counter,
    pings: Counter,
    /// The outputs kept, by id: a reference in state retains one.
    kept: UnorderedMap<String, BlobRef>,
    /// What each run's outcome said, by run id: JSON text.
    outcomes: UnorderedMap<String, String>,
    /// What `onThen` got from starting the next run, by the finished run's id: the new run's
    /// id or the error text.
    chained: UnorderedMap<String, String>,
    /// The plain blobs `publish` made, by id: a reference in state retains one.
    published: UnorderedMap<String, BlobRef>,
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
        digest_inputs()
    }

    /// [`Compute::digest`] whose result method keeps no output.
    #[app::compute(fuel = 20_000_000_000, memory_mib = 256, wall_secs = 60, result = "on_drop")]
    pub fn digest_unkept() -> app::Result<Vec<String>> {
        digest_inputs()
    }

    /// [`Compute::digest`] whose result method starts another `digest` run.
    #[app::compute(fuel = 20_000_000_000, memory_mib = 256, wall_secs = 60, result = "on_then")]
    pub fn digest_then() -> app::Result<Vec<String>> {
        digest_inputs()
    }

    /// Returns its argument, so a pipeline test sees what a step was given.
    #[app::compute(fuel = 20_000_000_000, memory_mib = 64, wall_secs = 30, result = "on_done")]
    pub fn echo(value: Vec<String>) -> app::Result<Vec<String>> {
        Ok(value)
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
    pub fn peek(id: BlobRef) -> app::Result<u64> {
        let _reader = calimero_sdk::files::open(&id)?;
        Ok(0)
    }

    /// Starts a `digest` run over `inputs`; the same inputs while it is active answer its id.
    pub fn start_digest(&mut self, inputs: Vec<BlobRef>) -> app::Result<RunId> {
        compute::start("digest", &(), &inputs)
    }

    /// Starts a `digestUnkept` run over `inputs`.
    pub fn start_unkept(&mut self, inputs: Vec<BlobRef>) -> app::Result<RunId> {
        compute::start("digestUnkept", &(), &inputs)
    }

    /// Starts a `digestThen` run over `inputs`.
    pub fn start_digest_then(&mut self, inputs: Vec<BlobRef>) -> app::Result<RunId> {
        compute::start("digestThen", &(), &inputs)
    }

    /// Starts the argument-less compute method `method` with no input.
    pub fn start_run(&mut self, method: String) -> app::Result<RunId> {
        compute::start(&method, &(), &[])
    }

    /// Asks for the active run `run` to end.
    pub fn cancel(&mut self, run: RunId) -> app::Result<()> {
        compute::cancel(&run)
    }

    /// Receives the outcome of a run, keeps its outputs and records what it said; an output a
    /// result method does not store is collected.
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
            "sha256": outcome.outputs.first().map(|o| o.sha256.clone()),
            "outputs": outcome.outputs.len(),
            "fuel": outcome.usage.fuel.to_string(),
            "cpuSeconds": outcome.usage.cpu_seconds,
        });
        self.outcomes
            .insert(&outcome.run.to_string(), &text.to_string())?;
        self.done.add(1)?;
        Ok(())
    }

    /// Receives the outcome of `digestUnkept` and stores no output.
    #[app::compute_result]
    pub fn on_drop(&mut self, _outcome: ComputeOutcome) -> app::Result<()> {
        self.done.add(1)?;
        Ok(())
    }

    /// Starts a `digest` run with no input and records its id, or why it was refused.
    #[app::compute_result]
    pub fn on_then(&mut self, outcome: ComputeOutcome) -> app::Result<()> {
        let started = match compute::start("digest", &(), &[]) {
            Ok(run) => run.to_string(),
            Err(e) => format!("error: {e:?}"),
        };
        self.chained.insert(&outcome.run.to_string(), &started)?;
        Ok(())
    }

    /// Publishes the compute output `output` as a plain blob and keeps its reference.
    pub fn publish(&mut self, output: BlobRef) -> app::Result<BlobRef> {
        let blob = compute::publish(&output)?;
        self.published.insert(&blob.id(), &blob)?;
        Ok(blob)
    }

    /// A download grant for the blob `id`, valid for `ttl` seconds.
    pub fn share(&mut self, id: BlobRef, ttl: u32) -> app::Result<String> {
        Ok(id.grant(ttl))
    }

    /// [`Self::share`] in a view, which the host refuses for a published blob.
    #[app::view]
    pub fn share_view(&self, id: BlobRef, ttl: u32) -> app::Result<String> {
        Ok(id.grant(ttl))
    }

    /// Logs an entry that changes nothing else.
    pub fn ping(&mut self) -> app::Result<()> {
        self.pings.add(1)?;
        Ok(())
    }

    /// The outcome recorded for `run`, as JSON text, if its result method ran.
    #[app::view]
    pub fn outcome(&self, run: RunId) -> app::Result<Option<String>> {
        Ok(self.outcomes.get(&run.to_string())?)
    }

    /// What `onThen` recorded for the finished run `run`.
    #[app::view]
    pub fn chained(&self, run: RunId) -> app::Result<Option<String>> {
        Ok(self.chained.get(&run.to_string())?)
    }

    /// How many outcomes `onDone` has received.
    #[app::view]
    pub fn done(&self) -> app::Result<u64> {
        Ok(self.done.get()?)
    }
}

fn digest_inputs() -> app::Result<Vec<String>> {
    let inputs = compute::inputs();
    let mut writer = calimero_sdk::files::create()?;
    let mut hashes = Vec::new();
    for (i, id) in inputs.iter().enumerate() {
        let mut reader = calimero_sdk::files::open(id)?;
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
    writer.finish()?;
    Ok(hashes)
}

fn percent(done: usize, total: usize) -> u8 {
    u8::try_from(done * 100 / total.max(1)).unwrap_or(100)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
