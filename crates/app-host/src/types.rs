//! What goes into an invocation and what comes out of it (spec section 3).

use std::time::Duration;

use thiserror::Error;

/// Fuel per call or view (D28); Task 13 measures the real ratio to wall time.
pub const DEFAULT_FUEL: u64 = 200_000_000;
/// Memory per instance: 1,024 WASM pages (64 MiB).
pub const DEFAULT_MEMORY_PAGES: u32 = 1_024;
/// Epoch deadline for a call or a view (spec section 5).
pub const CALL_WALL_CLOCK: Duration = Duration::from_secs(2);

/// Which entry point of `handle` runs, mirroring the WIT `invocation-kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationKind {
    Call,
    View,
    Action,
    Timer,
    XcallDeliver,
    Init,
    Migrate,
}

/// Who invoked the method (D13). Mirrors the WIT `caller` variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    Account(String),
    ApiKey { key_id: String, account: String },
    Public,
    System,
    App(String),
}

/// A timer tick's slot (D34).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerCtx {
    pub scheduled_ms: u64,
    pub previous_ms: Option<u64>,
}

/// One invocation of an app method.
///
/// `id_term` is the leader's Raft term while it runs the invocation and `id_seq` the
/// leader's per-term execution counter, assigned by the executor, incremented for every
/// execution (re-runs included) and never repeated within a term; together they make
/// `unique_id` unique without knowing the log index, which is assigned only at proposal
/// (D47, Ruling 12). `entry_ts_ms` is the entry timestamp
/// for calls, timers, init, migrate and xcall delivery, and the timestamp of the applied
/// entry being read for views (D19). `seed` is the leader's fresh seed for the run; views
/// and actions ignore it and draw from the node's CSPRNG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub kind: InvocationKind,
    pub method: String,
    pub args: Vec<u8>,
    pub caller: Caller,
    pub entry_ts_ms: u64,
    pub id_term: u64,
    pub id_seq: u64,
    pub seed: [u8; 32],
    pub timer: Option<TimerCtx>,
}

/// Per-invocation limits the engine enforces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub wall_clock: Duration,
    pub memory_pages: u32,
    pub fuel: u64,
}

impl Budget {
    /// The budget of a call, timer, init or xcall delivery.
    #[must_use]
    pub const fn call() -> Self {
        Self {
            wall_clock: CALL_WALL_CLOCK,
            memory_pages: DEFAULT_MEMORY_PAGES,
            fuel: DEFAULT_FUEL,
        }
    }

    /// The budget of a view; the same limits as a call.
    #[must_use]
    pub const fn view() -> Self {
        Self::call()
    }
}

/// Why an invocation trapped (spec section 5, error mapping).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrapKind {
    OutOfFuel,
    Timeout,
    OutOfMemory,
    StackOverflow,
    Fault,
    HostDenied,
}

/// How an invocation ended. Anything but `Ok` drops the diff and the events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    AppError { code: String, message: String },
    Trap { kind: TrapKind, detail: String },
}

/// One state change; `key` is the stored key, generation prefix included (D39).
/// `value: None` deletes the key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    pub key: Vec<u8>,
    pub value: Option<Vec<u8>>,
}

/// Who receives an event. Mirrors the WIT `audience` variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Audience {
    All,
    Caller,
    Accounts(Vec<String>),
    Team(String),
    OrgMembers(String),
    OrgAdmins(String),
}

/// An event emitted by a method; delivered only if the invocation ends `Ok`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub name: String,
    pub data: Vec<u8>,
    pub audience: Audience,
}

/// What an invocation cost. `writes` is counted by the host core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub fuel: u64,
    pub cpu: Duration,
    pub writes: u32,
}

/// Everything the executor sees of an invocation; exactly what the log stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub status: Status,
    pub result: Vec<u8>,
    pub diff: Vec<Write>,
    pub events: Vec<Event>,
    /// The owner-only app log (stdout, stderr, `log`); kept whatever the status.
    pub logs: Vec<String>,
    pub usage: Usage,
}

/// A role in an org, ordered from least to most privileged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrgRole {
    Guest,
    Member,
    Admin,
    Owner,
}

/// Identity facts the host core asks the node about; the node's replica answers.
pub trait Directory: Send + Sync {
    /// The org the app is installed in, if any.
    fn org(&self) -> Option<String>;
    /// The account that owns the app.
    fn app_owner(&self) -> String;
    /// The account's role in the app's org.
    fn org_role(&self, account: &str) -> Option<OrgRole>;
    /// Whether the account belongs to the team.
    fn in_team(&self, account: &str, team: &str) -> bool;
    /// Whether the team exists, for checking event audiences.
    fn team_exists(&self, team: &str) -> bool;
    /// Every org the account belongs to, with its role (requirement R1).
    fn orgs_of(&self, account: &str) -> Vec<(String, OrgRole)>;
}

/// A host call the policy refuses. The engine turns it into `Trap(HostDenied)`.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("host denied: {0}")]
pub struct HostDenied(pub String);

impl HostDenied {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

/// A blob's metadata. Mirrors the WIT `blob-info`; blobs are not enabled in this milestone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobInfo {
    pub size: u64,
    pub sha256: Vec<u8>,
    pub mime: String,
}

/// An invocation `HostCore::new` refuses.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum InvalidInvocation {
    /// A `Timer` invocation without its `TimerCtx` (D34).
    #[error("a timer invocation needs its timer slot")]
    TimerWithoutSlot,
    /// A `TimerCtx` on an invocation that is not a timer tick.
    #[error("only a timer invocation carries a timer slot")]
    SlotOutsideTimer,
}
