//! Files: encrypted, replicated byte streams the node keeps for the app.
//!
//! A [`Writer`] makes a file in a call, timer, init or xcall delivery; [`Reader`] opens one
//! in any invocation but a migration. The app keeps the [`FileId`] in its own state: a file
//! exists only once the run that finished it is committed, and is deleted with `delete`.
//! In the guest every function reaches the host through the `calimero:app/files` import; on
//! the host target they use a plain in-memory store that shows a finished file at once.
//! Only `testing::TestApp` shows the host's commit semantics (a file appears when its call
//! commits and a delete takes effect then) and its soft errors.

use std::fmt;
use std::io;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{AppError, Result};

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
use wasm as backend;

#[cfg(not(target_arch = "wasm32"))]
mod mock;
#[cfg(not(target_arch = "wasm32"))]
use mock as backend;

/// The id of a file: opaque, safe to store and to send in JSON.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileId(String);

impl FileId {
    /// The id as the host spells it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for FileId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

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
pub struct Writer(backend::Writer);

/// A file being read from the start.
pub struct Reader(backend::Reader);

/// Starts a new file.
pub fn create() -> Result<Writer> {
    backend::create().map(Writer).map_err(fail)
}

/// Opens a file this app holds.
pub fn open(id: &FileId) -> Result<Reader> {
    backend::open(id.as_str()).map(Reader).map_err(fail)
}

/// A file's size and hash; none when the app holds no such file.
pub fn info(id: &FileId) -> Option<FileInfo> {
    backend::info(id.as_str())
}

/// Deletes a file the app held before this run, when the run is committed.
pub fn delete(id: &FileId) -> Result<()> {
    backend::delete(id.as_str()).map_err(fail)
}

impl Writer {
    /// Seals the file and returns its id.
    pub fn finish(self) -> Result<FileId> {
        self.0.finish().map(FileId).map_err(fail)
    }
}

impl io::Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .write(buf)
            .map_err(|m| io::Error::other(FileIoError(m)))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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
