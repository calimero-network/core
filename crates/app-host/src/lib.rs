//! The engine-neutral host core of the `calimero:app` world.
//!
//! [`HostCore`] owns every state access and policy decision of one invocation; an
//! [`Engine`] only maps the WIT world onto it and enforces the [`Budget`].

mod abi;
mod core;
mod engine;
mod overlay;
mod state;
mod time;
mod types;

pub use abi::{Abi, MethodAbi, MethodKind};
pub use core::{CursorId, HostCore, MAX_LOG_BYTES, MAX_OPEN_CURSORS, MAX_RANGE_BATCH, MAX_WRITES};
pub use engine::{Engine, PrepareError, Prepared};
pub use state::{app_key, KeyValue, StateView, GENERATION};
pub use types::{
    Audience, BlobInfo, Budget, Caller, Directory, Event, HostDenied, Invocation, InvocationKind,
    OrgRole, Outcome, Status, TimerCtx, TrapKind, Usage, Write, CALL_WALL_CLOCK, DEFAULT_FUEL,
    DEFAULT_MEMORY_PAGES,
};
