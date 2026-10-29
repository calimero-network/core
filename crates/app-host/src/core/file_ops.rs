//! The `files` host calls of [`HostCore`]: handles, limits and the run's new and deleted files.

use std::collections::BTreeMap;

use super::HostCore;
use crate::files::{
    FileInfo, FileRead, FileWrite, NewFile, FILE_CREATED_IN_RUN, FILE_DELETED_TWICE, FILE_FAILED,
    FILE_FINISHED, FILE_TOO_LARGE, FILE_TOO_MANY_DELETES, FILE_TOO_MANY_OPEN, MAX_DELETES,
    MAX_FILE_BYTES, MAX_OPEN_FILES, MAX_READ_CHUNK, MAX_RUN_FILE_BYTES, NO_SUCH_FILE,
    RUN_WROTE_TOO_MUCH,
};
use crate::types::{HostDenied, InvocationKind};

/// A file writer handed to the guest; the WIT `writer` resource maps onto it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WriterId(pub u32);

/// A file reader handed to the guest; the WIT `reader` resource maps onto it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReaderId(pub u32);

enum Sink {
    Open(Box<dyn FileWrite>),
    Finished,
    /// The provider failed a write or the finish; nothing more is accepted.
    Failed,
}

struct Writer {
    sink: Sink,
    bytes: u64,
}

struct Reader {
    source: Box<dyn FileRead>,
    /// What the provider returned beyond the last `max`, served first on the next read.
    pending: Vec<u8>,
}

/// What a run holds open and has done to files.
#[derive(Default)]
pub(crate) struct FileTable {
    readers: BTreeMap<u32, Reader>,
    writers: BTreeMap<u32, Writer>,
    next: u32,
    written: u64,
    created: Vec<NewFile>,
    deleted: Vec<String>,
}

impl FileTable {
    pub(crate) fn into_changes(self) -> (Vec<NewFile>, Vec<String>) {
        (self.created, self.deleted)
    }

    fn open_count(&self) -> usize {
        self.readers.len() + self.writers.len()
    }

    fn next_id(&mut self) -> Result<u32, HostDenied> {
        let id = self.next;
        self.next = id
            .checked_add(1)
            .ok_or_else(|| HostDenied::new("file ids exhausted"))?;
        Ok(id)
    }
}

impl HostCore {
    /// Opens a writer. Logged writes only.
    pub fn files_create(&mut self) -> Result<Result<WriterId, String>, HostDenied> {
        self.may_write_files("files.create")?;
        if self.files.open_count() >= MAX_OPEN_FILES {
            return Ok(Err(FILE_TOO_MANY_OPEN.to_owned()));
        }
        let sink = match self.provider.create() {
            Ok(sink) => sink,
            Err(reason) => return Ok(Err(reason)),
        };
        let id = self.files.next_id()?;
        let _ = self.files.writers.insert(
            id,
            Writer {
                sink: Sink::Open(sink),
                bytes: 0,
            },
        );
        Ok(Ok(WriterId(id)))
    }

    pub fn writer_write(
        &mut self,
        id: WriterId,
        bytes: &[u8],
    ) -> Result<Result<(), String>, HostDenied> {
        self.may_write_files("files.write")?;
        let len = bytes.len() as u64;
        let written = self.files.written;
        let Some(writer) = self.files.writers.get_mut(&id.0) else {
            return Err(HostDenied::new("unknown writer"));
        };
        let sink = match &mut writer.sink {
            Sink::Open(sink) => sink,
            Sink::Finished => return Ok(Err(FILE_FINISHED.to_owned())),
            Sink::Failed => return Ok(Err(FILE_FAILED.to_owned())),
        };
        if writer.bytes.saturating_add(len) > MAX_FILE_BYTES {
            return Ok(Err(FILE_TOO_LARGE.to_owned()));
        }
        if written.saturating_add(len) > MAX_RUN_FILE_BYTES {
            return Ok(Err(RUN_WROTE_TOO_MUCH.to_owned()));
        }
        if let Err(reason) = sink.write(bytes) {
            writer.sink = Sink::Failed;
            return Ok(Err(reason));
        }
        writer.bytes += len;
        self.files.written += len;
        Ok(Ok(()))
    }

    /// Seals the file and returns its id; the file reaches the outcome only if the run ends `Ok`.
    pub fn writer_finish(&mut self, id: WriterId) -> Result<Result<String, String>, HostDenied> {
        self.may_write_files("files.finish")?;
        let Some(writer) = self.files.writers.get_mut(&id.0) else {
            return Err(HostDenied::new("unknown writer"));
        };
        let sink = match std::mem::replace(&mut writer.sink, Sink::Finished) {
            Sink::Open(sink) => sink,
            Sink::Finished => return Ok(Err(FILE_FINISHED.to_owned())),
            Sink::Failed => {
                writer.sink = Sink::Failed;
                return Ok(Err(FILE_FAILED.to_owned()));
            }
        };
        match sink.finish() {
            Ok(file) => {
                let id = file.id.clone();
                self.files.created.push(file);
                Ok(Ok(id))
            }
            Err(reason) => {
                writer.sink = Sink::Failed;
                Ok(Err(reason))
            }
        }
    }

    /// Drops a writer (the WIT resource's destructor); an unfinished one leaves no file.
    pub fn writer_close(&mut self, id: WriterId) {
        let _ = self.files.writers.remove(&id.0);
    }

    /// Opens a file the app marks. Every kind but migrate.
    pub fn files_open(&mut self, id: &str) -> Result<Result<ReaderId, String>, HostDenied> {
        self.may_read_files("files.open")?;
        if self.files.open_count() >= MAX_OPEN_FILES {
            return Ok(Err(FILE_TOO_MANY_OPEN.to_owned()));
        }
        if self.files.deleted.iter().any(|d| d == id) {
            return Ok(Err(NO_SUCH_FILE.to_owned()));
        }
        let source = match self.provider.open(id) {
            Ok(source) => source,
            Err(reason) => return Ok(Err(reason)),
        };
        let id = self.files.next_id()?;
        let _ = self.files.readers.insert(
            id,
            Reader {
                source,
                pending: Vec::new(),
            },
        );
        Ok(Ok(ReaderId(id)))
    }

    /// Up to `max` bytes, clamped to `1..=MAX_READ_CHUNK` so an empty vector always means the
    /// end of the file.
    pub fn reader_read(
        &mut self,
        id: ReaderId,
        max: u32,
    ) -> Result<Result<Vec<u8>, String>, HostDenied> {
        self.may_read_files("files.read")?;
        let Some(reader) = self.files.readers.get_mut(&id.0) else {
            return Err(HostDenied::new("unknown reader"));
        };
        let max = max.clamp(1, MAX_READ_CHUNK) as usize;
        if !reader.pending.is_empty() {
            let rest = reader.pending.split_off(max.min(reader.pending.len()));
            return Ok(Ok(std::mem::replace(&mut reader.pending, rest)));
        }
        let mut chunk = match reader.source.read(max) {
            Ok(chunk) => chunk,
            Err(reason) => return Ok(Err(reason)),
        };
        if chunk.len() > max {
            reader.pending = chunk.split_off(max);
        }
        Ok(Ok(chunk))
    }

    /// Drops a reader (the WIT resource's destructor).
    pub fn reader_close(&mut self, id: ReaderId) {
        let _ = self.files.readers.remove(&id.0);
    }

    pub fn files_info(&self, id: &str) -> Result<Option<FileInfo>, HostDenied> {
        self.may_read_files("files.info")?;
        if self.files.deleted.iter().any(|d| d == id) {
            return Ok(None);
        }
        Ok(self.provider.info(id))
    }

    /// Records the delete for the entry to apply; the file is gone for the rest of the run.
    /// Logged writes only; the file must exist and not be created, or already deleted, in
    /// this run.
    pub fn files_delete(&mut self, id: &str) -> Result<Result<(), String>, HostDenied> {
        self.may_write_files("files.delete")?;
        if self.files.created.iter().any(|f| f.id == id) {
            return Ok(Err(FILE_CREATED_IN_RUN.to_owned()));
        }
        if self.files.deleted.iter().any(|d| d == id) {
            return Ok(Err(FILE_DELETED_TWICE.to_owned()));
        }
        if self.provider.info(id).is_none() {
            return Ok(Err(NO_SUCH_FILE.to_owned()));
        }
        if self.files.deleted.len() >= MAX_DELETES {
            return Ok(Err(FILE_TOO_MANY_DELETES.to_owned()));
        }
        self.files.deleted.push(id.to_owned());
        Ok(Ok(()))
    }

    fn may_write_files(&self, what: &str) -> Result<(), HostDenied> {
        if !self.is_logged_write() {
            return Err(self.denied(what));
        }
        Ok(())
    }

    fn may_read_files(&self, what: &str) -> Result<(), HostDenied> {
        if self.inv.kind == InvocationKind::Migrate {
            return Err(self.denied(what));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_ids_are_never_reused_and_run_out_with_a_denial() {
        let mut table = FileTable {
            next: u32::MAX - 1,
            ..FileTable::default()
        };
        assert_eq!(table.next_id().unwrap(), u32::MAX - 1);
        assert_eq!(
            table.next_id().unwrap_err(),
            HostDenied::new("file ids exhausted")
        );
    }
}
