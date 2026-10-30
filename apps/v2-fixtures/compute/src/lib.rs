//! The cluster tests' compute fixture: `digest` streams every input and writes one output,
//! and `onDone` receives the run's outcome and keeps its outputs by `BlobRef`, which is what
//! retains them. The output is the input XOR the run's seeded random stream, so two runs of
//! one assignment write the same bytes. `spin`, `hog`, `burn` and `peek` each break one
//! limit of a run.

use std::io::{Read, Write};

use calimero_sdk::blob::BlobRef;
use calimero_sdk::compute;
use calimero_sdk::prelude::*;
use sha2::{Digest, Sha256};

/// One read or write call's size.
const PIECE: usize = 64 * 1024;

/// The part of the run's outcome `onDone` reads: its status and the output ids.
#[derive(serde::Deserialize)]
pub struct Outcome {
    pub status: String,
    pub outputs: Vec<BlobRef>,
}

#[app::state(version = 1)]
pub struct Compute {
    done: Counter,
    /// The outputs kept, by id: a reference in state retains one.
    kept: UnorderedMap<String, BlobRef>,
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

    /// Opens the file `id`, which a test names outside the run's inputs.
    #[app::compute(fuel = 20_000_000_000, memory_mib = 64, wall_secs = 30, result = "on_done")]
    pub fn peek(id: BlobRef) -> app::Result<u64> {
        let _reader = calimero_app_sdk::files::open(&id)?;
        Ok(0)
    }

    /// Receives the outcome of `digest` and keeps its outputs; an output a result method does
    /// not store is collected.
    #[app::compute_result]
    pub fn on_done(&mut self, outcome: Outcome) -> app::Result<()> {
        if outcome.status == "done" {
            for output in outcome.outputs {
                self.kept.insert(&output.id(), &output)?;
            }
        }
        self.done.add(1)?;
        Ok(())
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
