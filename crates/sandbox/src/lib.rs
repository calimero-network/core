//! `calimero-sandbox`: the job node's Linux sandbox for sandbox runs.
//!
//! A [`Run`] is one sandbox run's isolation: a size-capped workspace, the cgroup that caps
//! memory, processes and CPU for everything the run starts, and `exec` of a command in fresh
//! namespaces under an unprivileged, seccomp-filtered init. The host reads and writes the
//! workspace for the run's driver with `openat2` resolve flags, so a planted symlink, `..` or
//! FIFO cannot make it touch anything outside.
//!
//! Each `exec` re-runs the node's own binary as init; a binary that creates runs must call
//! [`init_if_requested`] first thing in `main`.
//!
//! Spec: [Compute actions](https://calimero-network.github.io/core/protocol/compute-actions/)
//! (a sandbox run is a compute run whose component also drives this sandbox).
//! Guide: `crates/sandbox/AGENTS.md`.

#[cfg(target_os = "linux")]
mod cgroup;
#[cfg(target_os = "linux")]
mod init;
#[cfg(target_os = "linux")]
mod run;
#[cfg(target_os = "linux")]
mod seccomp;

use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub use run::Run;

/// Elsewhere than Linux there is no sandbox: [`Run::create`] answers `Unsupported`.
#[cfg(not(target_os = "linux"))]
pub struct Run(std::convert::Infallible);

#[cfg(not(target_os = "linux"))]
impl Run {
    pub fn create(
        _config: &Config,
        _id: &str,
        _image: Image,
        _limits: &Limits,
    ) -> Result<Self, SandboxError> {
        Err(SandboxError::Unsupported)
    }

    pub fn exec(&self, _req: &ExecRequest) -> Result<ExecOutcome, SandboxError> {
        match self.0 {}
    }

    pub fn usage(&self) -> Usage {
        match self.0 {}
    }

    pub fn write(&self, _path: &str, _bytes: &[u8]) -> Result<(), SandboxError> {
        match self.0 {}
    }

    pub fn read(&self, _path: &str, _max: u32) -> Result<Vec<u8>, SandboxError> {
        match self.0 {}
    }

    pub fn collect(&self, _path: &str) -> Result<(std::fs::File, u64), SandboxError> {
        match self.0 {}
    }
}

/// The first argument that makes the node's binary act as a sandbox's init.
pub const INIT_ARG: &str = "__calimero_sandbox_init";

/// The uid and gid every sandboxed process runs as.
pub const NOBODY: u32 = 65_534;

/// Most a single `exec` may run, whatever its request asks: a run's wall clock is the node's.
pub const MAX_EXEC_MS: u32 = 900_000;
/// Most of stdout, and of stderr, kept per `exec`; the rest is drained and `truncated` set.
pub const MAX_OUTPUT_BYTES: usize = 1 << 20;
/// Most a single `read` returns.
pub const MAX_READ_BYTES: u32 = 16 << 20;
/// Most arguments, environment variables and their bytes in one `exec` request.
pub const MAX_ARGV: usize = 1_024;
pub const MAX_ENV: usize = 256;
pub const MAX_REQUEST_BYTES: usize = 1 << 20;
/// Most stdin handed to one `exec`.
pub const MAX_STDIN_BYTES: usize = 16 << 20;

/// Runs the sandbox init and exits if this process was started as one (its first argument is
/// [`INIT_ARG`]); otherwise returns. Call it before parsing arguments or starting threads.
pub fn init_if_requested() {
    #[cfg(target_os = "linux")]
    {
        let mut args = std::env::args();
        let _exe = args.next();
        if args.next().as_deref() == Some(INIT_ARG) {
            let rest: Vec<String> = args.collect();
            init::main(&rest);
        }
    }
}

/// Where a run's processes and files live on the node.
#[derive(Clone, Debug)]
pub struct Config {
    /// Holds each run's workspace and root mount point, one directory per run.
    pub base_dir: PathBuf,
    /// The cgroup (under each v1 controller, or under the v2 root) that holds the runs.
    pub cgroup_name: String,
    /// The binary re-run as init: the node's own, which calls [`init_if_requested`].
    pub init_exe: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from("/run/calimero-sandbox"),
            cgroup_name: "calimero-sandbox".to_owned(),
            init_exe: PathBuf::from("/proc/self/exe"),
        }
    }
}

/// A root filesystem the sandbox runs in, read-only.
///
/// The sandbox sees the image's `usr` as `/usr` (with the usual `/bin`, `/lib` links) and, of
/// its `etc`, only the loader cache, alternatives and CA certificates; `/etc/passwd`, `group`
/// and `hosts` are the sandbox's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub root: PathBuf,
}

impl Image {
    /// The node's own root: its `/usr`, read-only. The development image, until curated images
    /// ship in the node's disk image.
    pub fn host() -> Self {
        Self {
            root: PathBuf::from("/"),
        }
    }
}

/// The images this release's sandbox job nodes carry, by the name a sandbox method's ABI gives
/// (`sandbox.image`). Install refuses a release naming any other.
pub const IMAGES: &[&str] = &["host"];

/// The image named `name`, if this release carries it.
#[must_use]
pub fn image(name: &str) -> Option<Image> {
    match name {
        "host" => Some(Image::host()),
        _ => None,
    }
}

/// What a run may use, for everything it starts together.
#[derive(Clone, Debug)]
pub struct Limits {
    pub vcpus: u32,
    pub memory_bytes: u64,
    pub pids: u32,
    pub workspace_bytes: u64,
    /// Layers the escape suite's controls turn off; every layer is on otherwise.
    #[cfg(feature = "controls")]
    pub control: Control,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            vcpus: 1,
            memory_bytes: 256 << 20,
            pids: 64,
            workspace_bytes: 64 << 20,
            #[cfg(feature = "controls")]
            control: Control::Full,
        }
    }
}

/// The isolation layers a run keeps, for the escape suite's controls only.
#[cfg(feature = "controls")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Full,
    /// No seccomp filter.
    NoSeccomp,
    /// No seccomp filter and no privilege drop: the command runs as root.
    Root,
}

#[cfg(feature = "controls")]
impl Control {
    pub(crate) fn as_arg(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::NoSeccomp => "no-seccomp",
            Self::Root => "root",
        }
    }
}

/// One command to run in the sandbox.
#[derive(Clone, Debug, Default)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Relative to `/workspace`.
    pub cwd: Option<String>,
    pub stdin: Vec<u8>,
    pub timeout_ms: u32,
}

/// How a command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecOutcome {
    /// The exit code, or 128 + the signal that killed it.
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    pub timed_out: bool,
    pub wall_us: u64,
}

/// What a run has used so far, for everything it started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub cpu_us: u64,
    pub peak_memory_bytes: u64,
}

/// Why the sandbox refused or failed a request.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The request itself is wrong: an empty argv, a bad path or env var, too large.
    #[error("{0}")]
    Invalid(String),
    /// The node could not set up or serve the run; nothing the request did.
    #[error("sandbox unavailable: {0}")]
    Unavailable(String),
    /// Sandboxes need Linux.
    #[error("sandbox runs need Linux")]
    Unsupported,
}
