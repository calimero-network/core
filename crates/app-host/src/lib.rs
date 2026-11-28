//! The engine-neutral host core of the `calimero:app` world.
//!
//! [`HostCore`] owns every state access and policy decision of one invocation; an
//! [`Engine`] only maps the WIT world onto it and enforces the [`Budget`].
//!
//! Spec: [App execution](https://calimero-network.github.io/core/protocol/execution/),
//! [The env API](https://calimero-network.github.io/core/build/host-functions/).
//! Crate guide: [`crates/app-host/AGENTS.md`](https://github.com/calimero-network/core/blob/master/crates/app-host/AGENTS.md).

mod abi;
mod action;
mod blobs;
mod core;
mod engine;
mod entry;
mod files;
mod fuel;
mod overlay;
mod runs;
mod sandbox;
mod search;
mod state;
mod time;
mod types;
mod xcall;

pub use abi::{
    Abi, ComputeCaps, MethodAbi, MethodKind, SandboxCaps, MAX_COMPUTE_FUEL, MAX_COMPUTE_MEMORY_MIB,
    MAX_COMPUTE_WALL_SECS, MAX_SANDBOX_IMAGE_BYTES, MAX_SANDBOX_MEMORY_MIB, MAX_SANDBOX_VCPUS,
    MAX_SANDBOX_WORKSPACE_MIB, SUPPORTED_ABI_VERSIONS,
};
pub use action::{
    subcall_key, ActionHost, ActionStart, HttpExchange, HttpHost, HttpRequest, HttpResponse,
    NoHttp, MAX_ACTION_SUBCALLS, MAX_HTTP_BODY_BYTES, MAX_HTTP_REQUESTS, MAX_RECORDED_BODY_BYTES,
    MAX_STARTED_ACTIONS,
};
pub use blobs::{blob_refs, parse_blob_id, BlobHost, NoBlobs, BLOB_REF_TAG, MAX_GRANT_TTL_SECS};
pub use core::{
    CursorId, HostCore, Progress, ReaderId, RunLimit, RunLimitKind, WriterId,
    ACTION_RESERVED_METHOD, EMIT_BAD_AUDIENCE, EMIT_DATA_TOO_LARGE, EMIT_FOREIGN_ORG,
    EMIT_NAME_TOO_LONG, EMIT_TOO_MANY, EMIT_TOO_MANY_ACCOUNTS, EMIT_UNKNOWN_TEAM, ENTRY_TOO_LARGE,
    MAX_AUDIENCE_ACCOUNTS, MAX_COMPUTE_RESULT_BYTES, MAX_EVENTS, MAX_EVENT_DATA_BYTES,
    MAX_EVENT_NAME_BYTES, MAX_KEY_BYTES, MAX_LOG_BYTES, MAX_OPEN_CURSORS, MAX_PROGRESS_STEP_BYTES,
    MAX_RANGE_BATCH, MAX_RANGE_BATCH_BYTES, MAX_RUN_BYTES, MAX_WRITES, MAX_XCALLS,
    MAX_XCALL_NAME_BYTES, TOO_MANY_WRITES, XCALL_BAD_TARGET, XCALL_TOO_MANY,
};
pub use engine::{Engine, PrepareError, Prepared};
pub use entry::{entry_bytes, EntryIds, EntryOf, MAX_ENTRY_BYTES};
#[cfg(feature = "testing")]
pub use files::MemoryFiles;
pub use files::{
    FileError, FileInfo, FileRead, FileWrite, Files, NewFile, NewSegment, NoFiles, PublishedFile,
    ReadError, FILES_DISABLED, FILE_CORRUPT, FILE_FAILED, FILE_FINISHED, FILE_TOO_LARGE,
    FILE_TOO_MANY_NEW, FILE_TOO_MANY_OPEN, FILE_UNAVAILABLE, MAX_COMPUTE_FILE_BYTES,
    MAX_FILE_BYTES, MAX_NEW_FILES, MAX_OPEN_FILES, MAX_PUBLISHED_BYTES, MAX_PUBLISHED_PER_RUN,
    MAX_READ_CHUNK, MAX_RUN_FILE_BYTES, NOT_A_RUN_OUTPUT, NO_SUCH_FILE, RUN_PUBLISHED_TOO_MUCH,
    RUN_WROTE_TOO_MUCH,
};
pub use fuel::{
    SearchBudget, FUEL_HOST_CALL, FUEL_PER_BYTE, FUEL_PER_RANGE_ENTRY, FUEL_PER_SEARCH_BYTE,
    FUEL_PER_SEARCH_HIT, FUEL_PER_SEARCH_MATCH, FUEL_SEARCH_QUERY, FUEL_STATE_ACCESS,
    MAX_SEARCHES_PER_RUN, SEARCH_TOO_MANY,
};
pub use runs::{
    ActiveRun, NoRuns, RunInput, Runs, StartedRun, MAX_ACTIVE_RUNS, MAX_COMPUTE_ARGS_BYTES,
    MAX_PIPELINE_RUN_INPUTS, MAX_RUN_INPUTS, MAX_RUN_INPUT_BYTES, MAX_STARTS_PER_RUN,
    NOT_A_COMPUTE_METHOD, NO_ACTIVE_RUN, NO_COMPUTE_COMPONENT, RUN_ARGS_TOO_LARGE,
    RUN_DUPLICATE_INPUT, RUN_INPUTS_TOO_LARGE, RUN_INPUT_MISSING, RUN_RECORDS_UNREADABLE,
    RUN_TOO_MANY_ACTIVE, RUN_TOO_MANY_INPUTS, RUN_TOO_MANY_STARTS,
};
pub use sandbox::{
    SandboxExec, SandboxExecResult, SandboxFailure, SandboxHost, MAX_SANDBOX_REQUEST_BYTES,
    MAX_SANDBOX_WRITE_BYTES, NO_SANDBOX, SANDBOX_REQUEST_TOO_LARGE, SANDBOX_UNAVAILABLE,
};
pub use search::{
    NoSearch, SearchHit, SearchHost, SearchMode, SearchPage, SearchQuery, MAX_SEARCH_LIMIT,
    MAX_SEARCH_QUERY_BYTES,
};
pub use state::{stored_key, KeyValue, StateView, GENERATION};
pub use types::{
    Audience, BlobInfo, Budget, Caller, ComputeCtx, Directory, Event, HostDenied,
    InvalidInvocation, Invocation, InvocationKind, OrgRole, Outcome, Status, TimerCtx, TrapKind,
    Usage, Write, Xcall, ACTION_WALL_CLOCK, CALL_WALL_CLOCK, DEFAULT_FUEL, DEFAULT_MEMORY_PAGES,
    WALL_CLOCK_EXCEEDED,
};
pub use xcall::{
    sync_xcall_key, SyncXcall, XcallHost, XcallKind, XcallRequest, MAX_SYNC_XCALLS,
    MAX_SYNC_XCALL_ARGS_BYTES, MAX_SYNC_XCALL_RESULT_BYTES, XCALL_SYNC_SELF, XCALL_SYNC_TOO_MANY,
    XCALL_SYNC_UNAVAILABLE,
};
