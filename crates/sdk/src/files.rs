//! Files: encrypted, replicated byte streams the node keeps for the app.
//!
//! A [`Writer`] makes a file in a call, timer, init or xcall delivery; [`Reader`] opens one
//! only inside a compute run (the host refuses `open` anywhere else). The app keeps the [`BlobRef`] in its own state: a file
//! exists only once the run that finished it is committed, and goes when no state refers to it.
//! In the guest every function reaches the host through the `calimero:app/files` import; on
//! the host target they use a plain in-memory store that shows a finished file at once.
//! Only `testing::TestApp` shows the host's commit semantics (a file appears when its call
//! commits) and its soft errors.

use std::fmt;
use std::io;

use serde_json::json;

use crate::blob::BlobRef;
use crate::error::{AppError, Result};

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
use wasm as backend;

#[cfg(not(target_arch = "wasm32"))]
mod mock;
#[cfg(not(target_arch = "wasm32"))]
use mock as backend;

/// A file's size and SHA-256, both of the plaintext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    /// Bytes of content.
    pub size: u64,
    /// SHA-256 of the content.
    pub sha256: Vec<u8>,
}

/// The io error the reader and writer return, so `AppError` can tell a file failure from any
/// other io error.
#[derive(Debug)]
pub(crate) struct FileIoError(pub(crate) String);

impl fmt::Display for FileIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::error::Error for FileIoError {}

/// A file being written; [`Writer::finish`] seals it. Dropped unfinished, it leaves no file.
///
/// Small writes are gathered in the guest and sent to the host [`WRITE_BUFFER`] bytes at a
/// time, so `write!` or `serde_json::to_writer` costs a host call per 64 KiB, not per piece. A
/// write the host refuses is reported by the `write` or `flush` that sends it, or by `finish`.
pub struct Writer {
    file: backend::Writer,
    buffer: Vec<u8>,
}

/// How many bytes a [`Writer`] gathers before it sends them to the host.
pub const WRITE_BUFFER: usize = 64 * 1024;

/// A file being read from the start.
pub struct Reader(backend::Reader);

/// Starts a new file.
pub fn create() -> Result<Writer> {
    backend::create()
        .map(|file| Writer {
            file,
            buffer: Vec::new(),
        })
        .map_err(fail)
}

/// Opens a file this app holds.
pub fn open(id: &BlobRef) -> Result<Reader> {
    backend::open(&id.id()).map(Reader).map_err(fail)
}

/// A file's size and hash; none when the app holds no such file.
pub fn info(id: &BlobRef) -> Option<FileInfo> {
    backend::info(&id.id())
}

/// The account that uploaded the file, fixed at its upload; none for a file a call or a run
/// made, and for one the app does not hold. Not available in a compute run.
pub fn uploader(id: &BlobRef) -> Option<String> {
    crate::compute::uploader(id)
}

impl Writer {
    /// Seals the file and returns the reference to store in state.
    pub fn finish(mut self) -> Result<BlobRef> {
        self.send().map_err(fail)?;
        let id = self.file.finish().map_err(fail)?;
        BlobRef::parse(&id)
            .ok_or_else(|| fail(format!("the host returned a malformed file id: {id}")))
    }
}

impl Writer {
    /// Send what is gathered to the host.
    fn send(&mut self) -> std::result::Result<(), String> {
        if !self.buffer.is_empty() {
            self.file.write(&self.buffer)?;
            self.buffer.clear();
        }
        Ok(())
    }
}

impl io::Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let to_io = |m| io::Error::other(FileIoError(m));
        if self.buffer.len() + buf.len() > WRITE_BUFFER {
            self.send().map_err(to_io)?;
        }
        if buf.len() >= WRITE_BUFFER {
            // A large write goes as it is, after what was gathered before it.
            self.file.write(buf).map_err(to_io)?;
        } else {
            self.buffer.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send().map_err(|m| io::Error::other(FileIoError(m)))
    }
}

impl io::Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // The host clamps max 0 to 1, so an empty buffer would swallow a byte.
        if buf.is_empty() {
            return Ok(0);
        }
        let max = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let chunk = self
            .0
            .read(max)
            .map_err(|m| io::Error::other(FileIoError(m)))?;
        let n = chunk.len().min(buf.len());
        buf[..n].copy_from_slice(&chunk[..n]);
        Ok(n)
    }
}

fn fail(message: String) -> AppError {
    AppError::with_fields("Files", json!({ "message": message }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_writes_reach_the_host_in_64_kib_pieces() {
        use std::io::Write as _;
        let before = backend::write_calls();
        let mut w = create().unwrap();
        for i in 0..20_000 {
            writeln!(w, "line {i}").unwrap();
        }
        let big = vec![7u8; 100_000];
        w.write_all(&big).unwrap();
        let id = w.finish().unwrap();
        // ~208 KB of lines in 64 KiB pieces, then the 100 KB write as it is.
        let calls = backend::write_calls() - before;
        assert!(calls <= 6, "{calls} host writes");
        let mut expected: Vec<u8> = (0..20_000)
            .flat_map(|i| format!("line {i}\n").into_bytes())
            .collect();
        expected.extend_from_slice(&big);
        let got = info(&id).unwrap();
        assert_eq!(got.size, expected.len() as u64);
        let mut r = backend::open(&id.id()).unwrap();
        let mut read = Vec::new();
        loop {
            let chunk = r.read(1 << 20).unwrap();
            if chunk.is_empty() {
                break;
            }
            read.extend(chunk);
        }
        assert_eq!(read, expected, "the bytes and their order are kept");
    }

    #[test]
    fn only_file_io_errors_are_labelled_files() {
        let files = AppError::from(io::Error::other(FileIoError("gone".into())));
        assert_eq!(
            (files.code.as_str(), files.fields["message"].as_str()),
            ("Files", Some("gone"))
        );
        let other = AppError::from(io::Error::other("boom"));
        assert_eq!(other.code, "Io");
        assert_eq!(other.fields["message"], "boom");
    }
}
