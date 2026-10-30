//! The `files` host calls of [`HostCore`]: handles, limits and the run's new and deleted files.

use std::collections::BTreeMap;

use super::HostCore;
use crate::files::{
    FileError, FileInfo, FileRead, FileWrite, NewFile, ReadError, FILE_CORRUPT,
    FILE_CREATED_IN_RUN, FILE_DELETED_TWICE, FILE_FAILED, FILE_FINISHED, FILE_IS_RUN_INPUT,
    FILE_TOO_LARGE, FILE_TOO_MANY_DELETES, FILE_TOO_MANY_NEW, FILE_TOO_MANY_OPEN, FILE_UNAVAILABLE,
    MAX_COMPUTE_FILE_BYTES, MAX_DELETES, MAX_FILE_BYTES, MAX_NEW_FILES, MAX_OPEN_FILES,
    MAX_READ_CHUNK, MAX_RUN_FILE_BYTES, NO_SUCH_FILE, RUN_WROTE_TOO_MUCH,
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
    /// The provider answered an empty read: the file ended and proved authentic.
    ended: bool,
    /// A provider failure met after some bytes were gathered; answered by the next read.
    failed: Option<ReadError>,
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
    /// A logged run lost a file to node load: it ends as a timeout, not logged.
    unavailable: bool,
}

impl FileTable {
    pub(crate) fn unavailable(&self) -> bool {
        self.unavailable
    }

    pub(crate) fn is_deleted(&self, id: &str) -> bool {
        self.deleted.iter().any(|d| d == id)
    }

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
            Err(FileError::Failed(_)) if self.inv.compute.is_some() => {
                return Err(self.unavailable())
            }
            Err(FileError::Failed(reason)) => return Ok(Err(reason)),
            Err(FileError::Unavailable) => return Err(self.unavailable()),
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
        let run_limit = if self.inv.compute.is_some() {
            MAX_COMPUTE_FILE_BYTES
        } else {
            MAX_RUN_FILE_BYTES
        };
        if written.saturating_add(len) > run_limit {
            return Ok(Err(RUN_WROTE_TOO_MUCH.to_owned()));
        }
        if let Err(reason) = sink.write(bytes) {
            writer.sink = Sink::Failed;
            if self.inv.compute.is_some() {
                return Err(self.unavailable());
            }
            return Ok(Err(reason));
        }
        writer.bytes += len;
        self.files.written += len;
        Ok(Ok(()))
    }

    /// Seals the file and returns its id; the file reaches the outcome only if the run ends `Ok`.
    pub fn writer_finish(&mut self, id: WriterId) -> Result<Result<String, String>, HostDenied> {
        self.may_write_files("files.finish")?;
        if self.files.created.len() >= MAX_NEW_FILES {
            return Ok(Err(FILE_TOO_MANY_NEW.to_owned()));
        }
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
                if self.inv.compute.is_some() {
                    return Err(self.unavailable());
                }
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
        self.may_read_files("files.open", Some(id))?;
        if self.files.open_count() >= MAX_OPEN_FILES {
            return Ok(Err(FILE_TOO_MANY_OPEN.to_owned()));
        }
        if self.files.deleted.iter().any(|d| d == id) {
            return Ok(Err(NO_SUCH_FILE.to_owned()));
        }
        let source = match self.provider.open(id) {
            Ok(source) => source,
            Err(FileError::Failed(_)) if self.inv.compute.is_some() => {
                return Err(self.unavailable())
            }
            Err(FileError::Failed(reason)) => return Ok(Err(reason)),
            Err(FileError::Unavailable) if self.is_logged_write() || self.inv.compute.is_some() => {
                return Err(self.unavailable());
            }
            Err(FileError::Unavailable) => return Ok(Err(FILE_UNAVAILABLE.to_owned())),
        };
        let id = self.files.next_id()?;
        let _ = self.files.readers.insert(
            id,
            Reader {
                source,
                ended: false,
                failed: None,
                pending: Vec::new(),
            },
        );
        Ok(Ok(ReaderId(id)))
    }

    /// `max` bytes (clamped to `1..=MAX_READ_CHUNK`) or what is left of the file, however the
    /// provider chunks it; an empty vector is always the end of the file.
    pub fn reader_read(
        &mut self,
        id: ReaderId,
        max: u32,
    ) -> Result<Result<Vec<u8>, String>, HostDenied> {
        self.may_read_files("files.read", None)?;
        let compute = self.inv.compute.is_some();
        let Some(reader) = self.files.readers.get_mut(&id.0) else {
            return Err(HostDenied::new("unknown reader"));
        };
        let max = max.clamp(1, MAX_READ_CHUNK) as usize;
        let take = max.min(reader.pending.len());
        let rest = reader.pending.split_off(take);
        let mut out = std::mem::replace(&mut reader.pending, rest);
        while out.len() < max && !reader.ended {
            let failure = match reader.failed.take() {
                Some(failure) => failure,
                None => match reader.source.read(max - out.len()) {
                    Ok(mut chunk) => {
                        if chunk.is_empty() {
                            reader.ended = true;
                        }
                        let room = max - out.len();
                        if chunk.len() > room {
                            reader.pending = chunk.split_off(room);
                        }
                        out.append(&mut chunk);
                        continue;
                    }
                    Err(failure) => failure,
                },
            };
            if !out.is_empty() {
                reader.failed = Some(failure);
                break;
            }
            return match failure {
                ReadError::Failed(_) if compute => {
                    self.files.unavailable = true;
                    Err(HostDenied::new(FILE_UNAVAILABLE))
                }
                ReadError::Failed(reason) => Ok(Err(reason)),
                ReadError::Corrupt => Err(HostDenied::new(FILE_CORRUPT)),
            };
        }
        Ok(Ok(out))
    }

    /// Drops a reader (the WIT resource's destructor).
    pub fn reader_close(&mut self, id: ReaderId) {
        let _ = self.files.readers.remove(&id.0);
    }

    pub fn files_info(&self, id: &str) -> Result<Option<FileInfo>, HostDenied> {
        self.may_read_files("files.info", Some(id))?;
        if self.files.deleted.iter().any(|d| d == id) {
            return Ok(None);
        }
        Ok(self.provider.info(id))
    }

    /// Records the delete for the entry to apply; the file is gone for the rest of the run.
    /// Logged writes only; the file must exist and not be created, or already deleted, in
    /// this run.
    pub fn files_delete(&mut self, id: &str) -> Result<Result<(), String>, HostDenied> {
        if self.inv.compute.is_some() || !self.is_logged_write() {
            return Err(self.denied("files.delete"));
        }
        if self.files.created.iter().any(|f| f.id == id) {
            return Ok(Err(FILE_CREATED_IN_RUN.to_owned()));
        }
        if self.files.deleted.iter().any(|d| d == id) {
            return Ok(Err(FILE_DELETED_TWICE.to_owned()));
        }
        if self.provider.info(id).is_none() {
            return Ok(Err(NO_SUCH_FILE.to_owned()));
        }
        if self.is_run_input(id)? {
            return Ok(Err(FILE_IS_RUN_INPUT.to_owned()));
        }
        if self.files.deleted.len() >= MAX_DELETES {
            return Ok(Err(FILE_TOO_MANY_DELETES.to_owned()));
        }
        self.files.deleted.push(id.to_owned());
        Ok(Ok(()))
    }

    /// Whether `id` is an input of an active run, or of a run this invocation started.
    fn is_run_input(&mut self, id: &str) -> Result<bool, HostDenied> {
        let started = self
            .starts
            .iter()
            .any(|r| r.inputs.iter().any(|i| i.id == id));
        Ok(started
            || self
                .active_runs()?
                .iter()
                .any(|r| r.inputs.iter().any(|i| i == id)))
    }

    /// The run could not be served: it ends `Unavailable` and is retried, never the app's fault.
    fn unavailable(&mut self) -> HostDenied {
        self.files.unavailable = true;
        HostDenied::new(FILE_UNAVAILABLE)
    }

    fn may_write_files(&self, what: &str) -> Result<(), HostDenied> {
        if !self.is_logged_write() && self.inv.compute.is_none() {
            return Err(self.denied(what));
        }
        Ok(())
    }

    /// Every kind but migrate reads; a compute run only the files it was given. `id` is none
    /// for a read, which works on a handle that was already checked at `open`.
    fn may_read_files(&self, what: &str, id: Option<&str>) -> Result<(), HostDenied> {
        if self.inv.kind == InvocationKind::Migrate {
            return Err(self.denied(what));
        }
        if let Some(run) = &self.inv.compute {
            if id.is_some_and(|id| !run.inputs.iter().any(|input| input == id)) {
                return Err(HostDenied::new(format!("{what}: not an input of this run")));
            }
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
