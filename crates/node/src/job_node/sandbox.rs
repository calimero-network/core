//! A sandbox run's Linux sandbox on a sandbox job node: a `calimero_sandbox::Run` the run's
//! component drives through the host core's [`SandboxHost`].
//!
//! Spec: [Compute actions](https://calimero-network.github.io/core/protocol/compute-actions/).

use std::io::Read;

use calimero_app_host::{SandboxCaps, SandboxExec, SandboxExecResult, SandboxFailure, SandboxHost};
use calimero_sandbox::{Config, ExecRequest, Limits, Run, SandboxError, Usage};

/// One run's sandbox, set up before its component starts and removed when the run ends.
pub(super) struct JobSandbox(Run);

impl JobSandbox {
    /// Sets up the sandbox of a run whose method declares `caps`; `id` names its workspace and
    /// cgroup. An image this node does not carry, or a sandbox it cannot set up, is the node's:
    /// the attempt is lost.
    pub(super) fn create(config: &Config, id: &str, caps: &SandboxCaps) -> eyre::Result<Self> {
        let image = calimero_sandbox::image(&caps.image)
            .ok_or_else(|| eyre::eyre!("this node carries no image {:?}", caps.image))?;
        let limits = Limits {
            vcpus: caps.vcpus,
            memory_bytes: u64::from(caps.memory_mib) << 20,
            workspace_bytes: u64::from(caps.workspace_mib) << 20,
            ..Limits::default()
        };
        Ok(Self(Run::create(config, id, image, &limits)?))
    }

    /// What the run's commands have used so far.
    pub(super) fn usage(&self) -> Usage {
        self.0.usage()
    }
}

fn failure(e: SandboxError) -> SandboxFailure {
    match e {
        SandboxError::Invalid(reason) => SandboxFailure::Refused(reason),
        SandboxError::Unavailable(_) | SandboxError::Unsupported => SandboxFailure::Unavailable,
    }
}

impl SandboxHost for JobSandbox {
    fn exec(&self, req: &SandboxExec) -> Result<SandboxExecResult, SandboxFailure> {
        let out = self
            .0
            .exec(&ExecRequest {
                argv: req.argv.clone(),
                env: req.env.clone(),
                cwd: req.cwd.clone(),
                stdin: req.stdin.clone(),
                timeout_ms: req.timeout_ms,
            })
            .map_err(failure)?;
        Ok(SandboxExecResult {
            exit_code: out.exit_code,
            stdout: out.stdout,
            stderr: out.stderr,
            truncated: out.truncated,
            timed_out: out.timed_out,
            wall_us: out.wall_us,
        })
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), SandboxFailure> {
        self.0.write(path, bytes).map_err(failure)
    }

    fn read(&self, path: &str, max: u32) -> Result<Vec<u8>, SandboxFailure> {
        self.0.read(path, max).map_err(failure)
    }

    fn open(&self, path: &str) -> Result<Box<dyn Read + Send>, SandboxFailure> {
        let (file, _len) = self.0.collect(path).map_err(failure)?;
        Ok(Box::new(file))
    }
}
