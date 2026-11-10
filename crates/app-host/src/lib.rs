//! The engine-neutral host core of the `calimero:app` world.
//!
//! [`HostCore`] owns every state access and policy decision of one invocation; an
//! [`Engine`] only maps the WIT world onto it and enforces the [`Budget`].

mod abi;
mod action;
mod blobs;
mod collections;
mod core;
mod engine;
mod files;
mod overlay;
mod read_set;
mod runs;
mod search;
mod state;
mod time;
mod types;
mod xcall;

pub use abi::{
    Abi, ComputeCaps, MethodAbi, MethodKind, MAX_COMPUTE_FUEL, MAX_COMPUTE_MEMORY_MIB,
    MAX_COMPUTE_WALL_SECS, SUPPORTED_ABI_VERSIONS,
};
pub use action::{
    subcall_key, ActionHost, ActionStart, HttpExchange, HttpHost, HttpRequest, HttpResponse,
    NoHttp, MAX_ACTION_SUBCALLS, MAX_HTTP_BODY_BYTES, MAX_HTTP_REQUESTS, MAX_RECORDED_BODY_BYTES,
    MAX_STARTED_ACTIONS,
};
pub use blobs::{blob_refs, parse_blob_id, BlobHost, NoBlobs, BLOB_REF_TAG, MAX_GRANT_TTL_SECS};
pub use collections::{
    collection_fuel, CollEntry, CollError, CollPage, CollectionSchema, KeyPart, TableSchema,
    ValueType, COLLECTION_BASE_FUEL, COLLECTION_BYTE_FUEL, COLLECTION_ENTRY_FUEL,
    MAX_COLLECTION_KEY_BYTES, MAX_COLLECTION_LIMIT, MAX_JSON_DEPTH, MAX_JSON_ITEMS,
};
pub use core::{
    CursorId, HostCore, Progress, ReaderId, WriterId, EMIT_BAD_AUDIENCE, EMIT_DATA_TOO_LARGE,
    EMIT_FOREIGN_ORG, EMIT_NAME_TOO_LONG, EMIT_TOO_MANY, EMIT_TOO_MANY_ACCOUNTS, EMIT_UNKNOWN_TEAM,
    MAX_AUDIENCE_ACCOUNTS, MAX_COMPUTE_RESULT_BYTES, MAX_EVENTS, MAX_EVENT_DATA_BYTES,
    MAX_EVENT_NAME_BYTES, MAX_KEY_BYTES, MAX_LOG_BYTES, MAX_OPEN_CURSORS, MAX_PROGRESS_STEP_BYTES,
    MAX_RANGE_BATCH, MAX_RUN_BYTES, MAX_WRITES, MAX_XCALLS, MAX_XCALL_NAME_BYTES, XCALL_BAD_TARGET,
    XCALL_TOO_MANY,
};
pub use engine::{Engine, PrepareError, Prepared};
#[cfg(feature = "testing")]
pub use files::MemoryFiles;
pub use files::{
    FileError, FileInfo, FileRead, FileWrite, Files, NewFile, NoFiles, PublishedFile, ReadError,
    FILES_DISABLED, FILE_CORRUPT, FILE_FAILED, FILE_FINISHED, FILE_TOO_LARGE, FILE_TOO_MANY_NEW,
    FILE_TOO_MANY_OPEN, FILE_UNAVAILABLE, MAX_COMPUTE_FILE_BYTES, MAX_FILE_BYTES, MAX_NEW_FILES,
    MAX_OPEN_FILES, MAX_PUBLISHED_BYTES, MAX_PUBLISHED_PER_RUN, MAX_READ_CHUNK, MAX_RUN_FILE_BYTES,
    NOT_A_RUN_OUTPUT, NO_SUCH_FILE, RUN_PUBLISHED_TOO_MUCH, RUN_WROTE_TOO_MUCH,
};
pub use read_set::{ReadSet, MAX_READ_SET_BYTES, MAX_READ_SET_KEYS, MAX_READ_SET_RANGES};
pub use runs::{
    ActiveRun, NoRuns, RunInput, Runs, StartedRun, MAX_ACTIVE_RUNS, MAX_COMPUTE_ARGS_BYTES,
    MAX_RUN_INPUTS, MAX_RUN_INPUT_BYTES, MAX_STARTS_PER_RUN, NOT_A_COMPUTE_METHOD, NO_ACTIVE_RUN,
    NO_COMPUTE_COMPONENT, RUN_ARGS_TOO_LARGE, RUN_DUPLICATE_INPUT, RUN_INPUTS_TOO_LARGE,
    RUN_INPUT_MISSING, RUN_RECORDS_UNREADABLE, RUN_TOO_MANY_ACTIVE, RUN_TOO_MANY_INPUTS,
    RUN_TOO_MANY_STARTS,
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
};
pub use xcall::{
    sync_xcall_key, XcallHost, MAX_SYNC_XCALLS, MAX_SYNC_XCALL_ARGS_BYTES,
    MAX_SYNC_XCALL_RESULT_BYTES, XCALL_SYNC_SELF, XCALL_SYNC_TOO_MANY, XCALL_SYNC_UNAVAILABLE,
};
