//! The cluster tests' files fixture: writes deterministic files, keeps them by `BlobRef` and
//! streams them back. Method names and arguments are the ABI's camelCase ones; the content of
//! `putFile(len, seed)` is [`stream`], which the tests recompute.

use std::io::{Read, Write};

use calimero_app_sdk::blob::BlobRef;
use calimero_app_sdk::prelude::*;
use sha2::{Digest, Sha256};

/// One write call's size: the file reaches the host in pieces, as a real app's would.
const WRITE_PIECE: usize = 64 * 1024;

#[derive(serde::Serialize)]
pub struct PutOut {
    pub id: String,
    pub sha256: String,
}

#[derive(serde::Serialize)]
pub struct ReadOut {
    /// Decimal string (D20).
    pub size: String,
    pub sha256: String,
}

#[app::state(version = 1)]
pub struct Files {
    puts: Counter,
    /// The files kept, by id: a reference in state is what retains one.
    kept: UnorderedMap<String, BlobRef>,
    /// Padding that pushes an entry past the log's size cap.
    pad: UnorderedMap<String, String>,
}

#[app::error]
pub enum Error {
    Failed,
}

/// The `len` bytes of `seed`'s stream: splitmix64 words, little endian.
fn stream(len: u64, seed: u64) -> impl Iterator<Item = Vec<u8>> {
    let mut state = seed;
    let mut left = len;
    std::iter::from_fn(move || {
        if left == 0 {
            return None;
        }
        let piece = left.min(WRITE_PIECE as u64) as usize;
        let mut bytes = Vec::with_capacity(piece + 8);
        while bytes.len() < piece {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            bytes.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
        }
        bytes.truncate(piece);
        left -= piece as u64;
        Some(bytes)
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_all(id: &BlobRef) -> app::Result<ReadOut> {
    read_reader(calimero_app_sdk::files::open(id)?)
}

fn read_reader(mut reader: calimero_app_sdk::files::Reader) -> app::Result<ReadOut> {
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buf = vec![0u8; WRITE_PIECE];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        size += n as u64;
    }
    Ok(ReadOut {
        size: size.to_string(),
        sha256: hex(&hash.finalize()),
    })
}

fn write_file(len: u64, seed: u64) -> app::Result<(PutOut, BlobRef)> {
    let mut hash = Sha256::new();
    let mut writer = calimero_app_sdk::files::create()?;
    for piece in stream(len, seed) {
        hash.update(&piece);
        writer.write_all(&piece)?;
    }
    let id = writer.finish()?;
    let out = PutOut {
        id: id.to_string(),
        sha256: hex(&hash.finalize()),
    };
    Ok((out, id))
}

#[app::logic]
impl Files {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Writes `len` bytes of `seed`'s stream as a file, in 64 KiB writes, and keeps it.
    pub fn put_file(&mut self, len: u64, seed: u64) -> app::Result<PutOut> {
        self.puts.add(1)?;
        let (out, id) = write_file(len, seed)?;
        self.kept.insert(&out.id, &id)?;
        Ok(out)
    }

    /// Writes a file and keeps no reference to it: nothing retains it.
    pub fn put_unkept(&mut self, len: u64, seed: u64) -> app::Result<PutOut> {
        self.puts.add(1)?;
        Ok(write_file(len, seed)?.0)
    }

    /// Drops the reference to a file; it goes once nothing else refers to it.
    pub fn forget_file(&mut self, id: String) -> app::Result<bool> {
        Ok(self.kept.remove(&id)?.is_some())
    }

    /// Writes a file, then fails: the file must not outlive the call.
    pub fn put_and_fail(&mut self, len: u64) -> app::Result<()> {
        let _put = write_file(len, 1)?;
        app::bail!(Error::Failed)
    }

    /// Streams the file back; its size and SHA-256 as the guest computed them.
    #[app::view]
    pub fn read_file(&self, id: BlobRef) -> app::Result<ReadOut> {
        read_all(&id)
    }

    /// `readFile` as a call, which is logged.
    pub fn touch_file(&mut self, id: BlobRef) -> app::Result<ReadOut> {
        self.puts.add(1)?;
        read_all(&id)
    }

    /// Holds the file open twice at once, reads one of them.
    pub fn touch_twice(&mut self, id: BlobRef) -> app::Result<ReadOut> {
        let first = calimero_app_sdk::files::open(&id)?;
        let _second = calimero_app_sdk::files::open(&id)?;
        read_reader(first)
    }

    /// Writes a file and `pad` bytes of state: past the log's entry cap the entry is replaced.
    pub fn put_padded(&mut self, len: u64, seed: u64, pad: u32) -> app::Result<PutOut> {
        self.pad
            .insert(&"pad".to_owned(), &"x".repeat(pad as usize))?;
        Ok(write_file(len, seed)?.0)
    }

    /// Drops a file's reference and writes `pad` bytes of state.
    pub fn forget_padded(&mut self, id: String, pad: u32) -> app::Result<bool> {
        self.pad
            .insert(&"pad".to_owned(), &"x".repeat(pad as usize))?;
        Ok(self.kept.remove(&id)?.is_some())
    }
}
