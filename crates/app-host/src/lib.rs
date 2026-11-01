//! The engine-neutral host core of the `calimero:app` world.
//!
//! [`HostCore`] owns every state access and policy decision of one invocation; an
//! [`Engine`] only maps the WIT world onto it and enforces the [`Budget`].

mod abi;
mod blobs;
mod core;
mod engine;
mod overlay;
mod search;
mod state;
mod time;
mod types;

pub use abi::{Abi, MethodAbi, MethodKind, SUPPORTED_ABI_VERSIONS};
pub use blobs::{blob_refs, parse_blob_id, BlobHost, NoBlobs, BLOB_REF_TAG, MAX_GRANT_TTL_SECS};
pub use core::{
    CursorId, HostCore, EMIT_BAD_AUDIENCE, EMIT_DATA_TOO_LARGE, EMIT_FOREIGN_ORG,
    EMIT_NAME_TOO_LONG, EMIT_TOO_MANY, EMIT_TOO_MANY_ACCOUNTS, EMIT_UNKNOWN_TEAM,
    MAX_AUDIENCE_ACCOUNTS, MAX_EVENTS, MAX_EVENT_DATA_BYTES, MAX_EVENT_NAME_BYTES, MAX_KEY_BYTES,
    MAX_LOG_BYTES, MAX_OPEN_CURSORS, MAX_RANGE_BATCH, MAX_RUN_BYTES, MAX_WRITES, MAX_XCALLS,
    MAX_XCALL_NAME_BYTES, XCALL_BAD_TARGET, XCALL_TOO_MANY,
};
pub use engine::{Engine, PrepareError, Prepared};
pub use search::{
    NoSearch, SearchHit, SearchHost, SearchMode, SearchPage, SearchQuery, MAX_SEARCH_LIMIT,
    MAX_SEARCH_QUERY_BYTES,
};
pub use state::{stored_key, KeyValue, StateView, GENERATION};
pub use types::{
    Audience, BlobInfo, Budget, Caller, Directory, Event, HostDenied, InvalidInvocation,
    Invocation, InvocationKind, OrgRole, Outcome, Status, TimerCtx, TrapKind, Usage, Write, Xcall,
    CALL_WALL_CLOCK, DEFAULT_FUEL, DEFAULT_MEMORY_PAGES,
};
