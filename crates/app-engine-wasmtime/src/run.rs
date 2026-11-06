//! One compute run on a compute engine, and its measurement: the setup a job node and the
//! local runner share, so the two never drift. Only the file provider differs.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_app_host::{
    Budget, Caller, ComputeCaps, ComputeCtx, Directory, Files, HostCore, Invocation,
    InvocationKind, OrgRole, Outcome, Prepared, Progress, Status, TrapKind,
};

use crate::WasmtimeEngine;

/// Whether [`Measured::cpu`] is the thread's CPU time on this platform (else wall time).
pub const THREAD_CPU: bool = cfg!(unix);

/// What a run is: the assignment's own fields, whoever made it.
pub struct ComputeRun {
    pub app: String,
    pub run: String,
    pub owner: String,
    pub method: String,
    pub args: Vec<u8>,
    pub caller: Caller,
    pub start_ts_ms: u64,
    pub seed: [u8; 32],
    /// Input file ids, in the order the guest sees them.
    pub inputs: Vec<String>,
    pub caps: ComputeCaps,
}

/// Why a run failed, by what happened and never by its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Fuel,
    Memory,
    Trap,
    AppError,
    Cancelled,
}

impl Reason {
    /// The name a report and the local runner give.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fuel => "fuel",
            Self::Memory => "memory",
            Self::Trap => "trap",
            Self::AppError => "app-error",
            Self::Cancelled => "cancelled",
        }
    }
}

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ending {
    Done,
    Failed(Reason),
    /// The host could not serve the run, or its wall clock ran out: it runs elsewhere.
    Lost,
}

/// Maps an outcome's status to how the run ended.
#[must_use]
pub fn ending_of(status: &Status) -> Ending {
    match status {
        Status::Ok => Ending::Done,
        Status::AppError { .. } => Ending::Failed(Reason::AppError),
        Status::Trap { kind, .. } => match kind {
            TrapKind::OutOfFuel => Ending::Failed(Reason::Fuel),
            TrapKind::OutOfMemory => Ending::Failed(Reason::Memory),
            TrapKind::Cancelled => Ending::Failed(Reason::Cancelled),
            TrapKind::Timeout | TrapKind::Unavailable => Ending::Lost,
            TrapKind::StackOverflow | TrapKind::Fault | TrapKind::HostDenied => {
                Ending::Failed(Reason::Trap)
            }
        },
    }
}

/// What one run took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    /// Thread CPU time ([`THREAD_CPU`]), else wall time.
    pub cpu: Duration,
    pub wall: Duration,
}

/// What a compute run knows of its surroundings: nothing but its app's owner.
struct ComputeDirectory {
    owner: String,
}

impl Directory for ComputeDirectory {
    fn org(&self) -> Option<String> {
        None
    }
    fn app_owner(&self) -> String {
        self.owner.clone()
    }
    fn org_role(&self, _account: &str) -> Option<OrgRole> {
        None
    }
    fn in_team(&self, _account: &str, _team: &str) -> bool {
        false
    }
    fn team_exists(&self, _team: &str) -> bool {
        false
    }
    fn orgs_of(&self, _account: &str) -> Vec<(String, OrgRole)> {
        Vec::new()
    }
}

impl WasmtimeEngine {
    /// Runs `run` on `prepared` over `files`, measured on the calling thread; `cancel` set from
    /// another thread ends it. `Err` only when the invocation is malformed.
    pub fn run_compute(
        &self,
        prepared: &Prepared,
        run: ComputeRun,
        files: Arc<dyn Files>,
        progress: Progress,
        cancel: Arc<AtomicBool>,
    ) -> eyre::Result<(Outcome, Measured)> {
        let invocation = Invocation {
            kind: InvocationKind::Action,
            method: run.method,
            args: run.args,
            caller: run.caller,
            entry_ts_ms: run.start_ts_ms,
            id_term: 0,
            id_seq: 0,
            seed: run.seed,
            timer: None,
            compute: Some(ComputeCtx {
                run_id: run.run,
                owner: run.owner.clone(),
                inputs: run.inputs,
            }),
        };
        let host = HostCore::new(
            &run.app,
            Arc::new(BTreeMap::<Vec<u8>, Vec<u8>>::new()),
            0,
            invocation,
            Arc::new(ComputeDirectory { owner: run.owner }),
        )
        .map_err(|e| eyre::eyre!("{e}"))?
        .with_files(files)
        .with_progress(progress);
        let budget = Budget::compute(&run.caps);
        Ok(measure(|| {
            self.invoke_cancellable(prepared, host, budget, cancel)
        }))
    }
}

/// Runs `f` and measures it on the thread it runs on.
pub fn measure<R>(f: impl FnOnce() -> R) -> (R, Measured) {
    let (cpu_at, wall_at) = (thread_cpu(), Instant::now());
    let out = f();
    let wall = wall_at.elapsed();
    let cpu = match (cpu_at, thread_cpu()) {
        (Some(at), Some(now)) => now.saturating_sub(at),
        _ => wall,
    };
    (out, Measured { cpu, wall })
}

#[cfg(unix)]
fn thread_cpu() -> Option<Duration> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `clock_gettime` writes only the `timespec` it is handed, which
    // lives on this frame for the whole call.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut ts) };
    if rc != 0 {
        return None;
    }
    Some(Duration::new(
        u64::try_from(ts.tv_sec).ok()?,
        u32::try_from(ts.tv_nsec).ok()?,
    ))
}

#[cfg(not(unix))]
const fn thread_cpu() -> Option<Duration> {
    None
}
