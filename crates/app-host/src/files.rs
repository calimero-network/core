//! The files a run can read, create and delete, behind the node's encrypted store.
//! The host core talks only to [`Files`]; the node provides it.

#[cfg(feature = "testing")]
mod memory;

#[cfg(feature = "testing")]
pub use memory::MemoryFiles;

/// Open files (readers and writers) one run may hold at once.
pub const MAX_OPEN_FILES: usize = 16;
/// Plaintext bytes of one file (the node's blob cap).
pub const MAX_FILE_BYTES: u64 = 47 * 1024 * 1024;
/// Plaintext bytes one run may write across all its files; the node holds a logged run's new
/// ciphertext in memory until its entry is proposed.
pub const MAX_RUN_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Plaintext bytes a compute run may write across all its files; a job node keeps them sealed
/// on disk, not in memory.
pub const MAX_COMPUTE_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Bytes one `read` returns at most; a larger `max` is clamped.
pub const MAX_READ_CHUNK: u32 = 1024 * 1024;
/// Files one run may delete.
pub const MAX_DELETES: usize = 1_000;
/// Files one run may finish.
pub const MAX_NEW_FILES: usize = 1_000;

/// The soft errors of the `files` interface (the WIT `result<_, string>`), fixed so every
/// engine and SDK returns and matches the same strings.
pub const NO_SUCH_FILE: &str = "no such file";
pub const FILES_DISABLED: &str = "files are not enabled";
pub const FILE_TOO_MANY_OPEN: &str = "too many open files";
pub const FILE_TOO_LARGE: &str = "file too large";
pub const RUN_WROTE_TOO_MUCH: &str = "run wrote too much";
pub const FILE_FINISHED: &str = "file already finished";
pub const FILE_FAILED: &str = "file write failed";
pub const FILE_DELETED_TWICE: &str = "file already deleted";
pub const FILE_CREATED_IN_RUN: &str = "file created in this run";
pub const FILE_IS_RUN_INPUT: &str = "file is a run input";
pub const FILE_TOO_MANY_DELETES: &str = "too many deletes";
pub const FILE_TOO_MANY_NEW: &str = "too many new files";
/// The bytes of a file cannot be had now (a view's soft error; a logged run is aborted).
pub const FILE_UNAVAILABLE: &str = "file bytes are not available";
/// The reason of the trap a corrupt or truncated file ends the run with; never a soft error.
pub const FILE_CORRUPT: &str = "file failed its integrity check";

/// A file's plaintext size and SHA-256, as the app's mark records them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub size: u64,
    pub sha256: Vec<u8>,
}

/// A file a run finished: its id is the hash of the ciphertext, which the node keeps in
/// memory until the entry that marks it is proposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFile {
    pub id: String,
    pub size: u64,
    pub sha256: Vec<u8>,
    pub wrapped_key: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// Why a file could not be opened or created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileError {
    /// The guest is told (the WIT `result<_, string>`).
    Failed(String),
    /// The node cannot serve the file now. A logged run is aborted and retried (never a
    /// choice the guest makes, which would be logged); a view gets `FILE_UNAVAILABLE`.
    Unavailable,
}

/// Why a read failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// The guest is told (the WIT `result<_, string>`).
    Failed(String),
    /// The stored bytes are altered or cut short: the run traps, so no guest can ignore it.
    Corrupt,
}

/// Plaintext of one file, read front to back.
pub trait FileRead: Send {
    /// Up to `max` bytes (the host core truncates a longer answer); an empty vector is the
    /// end of the file, and only after the whole file proved authentic.
    fn read(&mut self, max: usize) -> Result<Vec<u8>, ReadError>;
}

/// A file being written.
pub trait FileWrite: Send {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn finish(self: Box<Self>) -> Result<NewFile, String>;
}

/// The node's file store, as one run sees it.
pub trait Files: Send + Sync {
    /// Opens a file the app marks; `NO_SUCH_FILE` for any other id.
    fn open(&self, id: &str) -> Result<Box<dyn FileRead>, FileError>;
    fn info(&self, id: &str) -> Option<FileInfo>;
    fn create(&self) -> Result<Box<dyn FileWrite>, FileError>;
}

/// The provider of a node that has no file store: nothing exists and nothing can be made.
pub struct NoFiles;

impl Files for NoFiles {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, FileError> {
        Err(FileError::Failed(NO_SUCH_FILE.to_owned()))
    }

    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }

    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        Err(FileError::Failed(FILES_DISABLED.to_owned()))
    }
}
