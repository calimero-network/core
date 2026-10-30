//! The engine-neutral host core of the `calimero:app` world.
//!
//! [`HostCore`] owns every state access and policy decision of one invocation; an
//! [`Engine`] only maps the WIT world onto it and enforces the [`Budget`].

mod abi;
mod core;
mod engine;
mod files;
mod overlay;
mod state;
mod time;
mod types;

pub use abi::{
    Abi, ComputeCaps, MethodAbi, MethodKind, MAX_COMPUTE_FUEL, MAX_COMPUTE_MEMORY_MIB,
    MAX_COMPUTE_WALL_SECS, SUPPORTED_ABI_VERSIONS,
};
pub use core::{
    CursorId, HostCore, Progress, ReaderId, WriterId, EMIT_BAD_AUDIENCE, EMIT_DATA_TOO_LARGE,
    EMIT_FOREIGN_ORG, EMIT_NAME_TOO_LONG, EMIT_TOO_MANY, EMIT_TOO_MANY_ACCOUNTS, EMIT_UNKNOWN_TEAM,
    MAX_AUDIENCE_ACCOUNTS, MAX_COMPUTE_RESULT_BYTES, MAX_EVENTS, MAX_EVENT_DATA_BYTES,
    MAX_EVENT_NAME_BYTES, MAX_KEY_BYTES, MAX_LOG_BYTES, MAX_OPEN_CURSORS, MAX_PROGRESS_STEP_BYTES,
    MAX_RANGE_BATCH, MAX_RUN_BYTES, MAX_WRITES,
};
pub use engine::{Engine, PrepareError, Prepared};
#[cfg(feature = "testing")]
pub use files::MemoryFiles;
pub use files::{
    FileError, FileInfo, FileRead, FileWrite, Files, NewFile, NoFiles, ReadError, FILES_DISABLED,
    FILE_CORRUPT, FILE_CREATED_IN_RUN, FILE_DELETED_TWICE, FILE_FAILED, FILE_FINISHED,
    FILE_TOO_LARGE, FILE_TOO_MANY_DELETES, FILE_TOO_MANY_NEW, FILE_TOO_MANY_OPEN, FILE_UNAVAILABLE,
    MAX_COMPUTE_FILE_BYTES, MAX_DELETES, MAX_FILE_BYTES, MAX_NEW_FILES, MAX_OPEN_FILES,
    MAX_READ_CHUNK, MAX_RUN_FILE_BYTES, NO_SUCH_FILE, RUN_WROTE_TOO_MUCH,
};
pub use state::{stored_key, KeyValue, StateView, GENERATION};
pub use types::{
    Audience, BlobInfo, Budget, Caller, ComputeCtx, Directory, Event, HostDenied,
    InvalidInvocation, Invocation, InvocationKind, OrgRole, Outcome, Status, TimerCtx, TrapKind,
    Usage, Write, CALL_WALL_CLOCK, DEFAULT_FUEL, DEFAULT_MEMORY_PAGES,
};
