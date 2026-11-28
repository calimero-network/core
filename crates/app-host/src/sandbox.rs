//! A sandbox run's Linux sandbox, as the host core sees it: the provider a job node gives a
//! run of a sandbox method (`HostCore::with_sandbox`), and what the WIT `sandbox` interface
//! passes through it.

use std::io::Read;

/// Most bytes of one `exec` request (arguments, environment and stdin together) a run may hand
/// the host; a larger one is refused before it reaches the sandbox.
pub const MAX_SANDBOX_REQUEST_BYTES: usize = 17 << 20;
/// Most bytes one `write` hands the host.
pub const MAX_SANDBOX_WRITE_BYTES: usize = 64 << 20;
/// What a sandbox call answers outside a sandbox run.
pub const NO_SANDBOX: &str = "no sandbox: only a sandbox run has one";
/// What the run answers when the sandbox could not serve it: the attempt is lost and retried.
pub const SANDBOX_UNAVAILABLE: &str = "the sandbox is not available";
/// A request over [`MAX_SANDBOX_REQUEST_BYTES`] or [`MAX_SANDBOX_WRITE_BYTES`].
pub const SANDBOX_REQUEST_TOO_LARGE: &str = "sandbox request too large";

/// One command, as the guest asked for it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SandboxExec {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<String>,
    pub stdin: Vec<u8>,
    pub timeout_ms: u32,
}

impl SandboxExec {
    /// The bytes the request hands the host.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.argv.iter().map(String::len).sum::<usize>()
            + self
                .env
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>()
            + self.cwd.as_ref().map_or(0, String::len)
            + self.stdin.len()
    }
}

/// How a command ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SandboxExecResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    pub timed_out: bool,
    pub wall_us: u64,
}

/// Why a sandbox call did not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxFailure {
    /// The request's fault (a bad path, a symlink, over a limit): the guest gets the text.
    Refused(String),
    /// The node's: the run ends `Unavailable` and its attempt is retried elsewhere.
    Unavailable,
}

/// The job node's side of one sandbox run.
pub trait SandboxHost: Send + Sync {
    /// Runs one command in a fresh sandbox over the run's workspace; `timeout_ms` is already
    /// held to the run's deadline.
    fn exec(&self, req: &SandboxExec) -> Result<SandboxExecResult, SandboxFailure>;
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), SandboxFailure>;
    fn read(&self, path: &str, max: u32) -> Result<Vec<u8>, SandboxFailure>;
    /// A regular file of the workspace, to seal as one of the run's outputs.
    fn open(&self, path: &str) -> Result<Box<dyn Read + Send>, SandboxFailure>;
}
