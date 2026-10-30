//! The cluster tests' compute fixture: `digest` streams every input and writes one output,
//! and `onDone` receives the run's outcome. The output is the input XOR the run's seeded
//! random stream, so two runs of one assignment write the same bytes.

use std::io::{Read, Write};

use calimero_app_sdk::files::FileId;
use calimero_app_sdk::compute;
use calimero_app_sdk::prelude::*;
use sha2::{Digest, Sha256};

/// One read or write call's size.
const PIECE: usize = 64 * 1024;

#[app::state(version = 1)]
pub struct Compute {
    done: Counter,
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

    /// Receives the outcome of `digest`.
    #[app::compute_result]
    pub fn on_done(&mut self) -> app::Result<()> {
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
