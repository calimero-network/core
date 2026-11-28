//! The `sandbox` host calls of [`HostCore`]: a sandbox run's commands and workspace files,
//! through the job node's [`SandboxHost`].

use std::io::Read;
use std::time::Instant;

use super::HostCore;
use crate::files::MAX_READ_CHUNK;
use crate::fuel::per_byte;
use crate::sandbox::{
    SandboxExec, SandboxExecResult, SandboxFailure, SandboxHost, MAX_SANDBOX_REQUEST_BYTES,
    MAX_SANDBOX_WRITE_BYTES, NO_SANDBOX, SANDBOX_REQUEST_TOO_LARGE, SANDBOX_UNAVAILABLE,
};
use crate::types::HostDenied;

impl HostCore {
    /// The run's sandbox: a compute run given one, else `HostDenied(NO_SANDBOX)`.
    fn sandbox(&self) -> Result<std::sync::Arc<dyn SandboxHost>, HostDenied> {
        match &self.sandbox {
            Some(sandbox) if self.is_compute() => Ok(std::sync::Arc::clone(sandbox)),
            _ => Err(HostDenied::new(NO_SANDBOX)),
        }
    }

    /// Ends the run `Unavailable`, as a file the node cannot serve does: the attempt is lost.
    fn sandbox_unavailable(&mut self) -> HostDenied {
        self.sandbox_unavailable = true;
        HostDenied::new(SANDBOX_UNAVAILABLE)
    }

    fn sandbox_answer<T>(
        &mut self,
        answer: Result<T, SandboxFailure>,
    ) -> Result<Result<T, String>, HostDenied> {
        match answer {
            Ok(value) => Ok(Ok(value)),
            Err(SandboxFailure::Refused(reason)) => Ok(Err(reason)),
            Err(SandboxFailure::Unavailable) => Err(self.sandbox_unavailable()),
        }
    }

    /// Runs one command; its timeout is held to the run's deadline. Owes the bytes it was
    /// handed and the output it answers.
    pub fn sandbox_exec(
        &mut self,
        mut req: SandboxExec,
    ) -> Result<Result<SandboxExecResult, String>, HostDenied> {
        let bytes = req.bytes();
        self.burn(per_byte(bytes));
        let sandbox = self.sandbox()?;
        if bytes > MAX_SANDBOX_REQUEST_BYTES {
            return Ok(Err(SANDBOX_REQUEST_TOO_LARGE.to_owned()));
        }
        if let Some(deadline) = self.deadline {
            let left = deadline
                .saturating_duration_since(Instant::now())
                .as_millis();
            req.timeout_ms = req.timeout_ms.min(u32::try_from(left).unwrap_or(u32::MAX));
        }
        let answer = self.blocking(|_| sandbox.exec(&req))?;
        if let Ok(out) = &answer {
            self.burn(per_byte(out.stdout.len() + out.stderr.len()));
        }
        self.sandbox_answer(answer)
    }

    pub fn sandbox_write(
        &mut self,
        path: &str,
        bytes: &[u8],
    ) -> Result<Result<(), String>, HostDenied> {
        self.burn(per_byte(path.len() + bytes.len()));
        let sandbox = self.sandbox()?;
        if bytes.len() > MAX_SANDBOX_WRITE_BYTES {
            return Ok(Err(SANDBOX_REQUEST_TOO_LARGE.to_owned()));
        }
        let answer = sandbox.write(path, bytes);
        self.sandbox_answer(answer)
    }

    pub fn sandbox_read(
        &mut self,
        path: &str,
        max: u32,
    ) -> Result<Result<Vec<u8>, String>, HostDenied> {
        self.burn(per_byte(path.len()));
        let sandbox = self.sandbox()?;
        let answer = sandbox.read(path, max);
        if let Ok(bytes) = &answer {
            self.burn(per_byte(bytes.len()));
        }
        self.sandbox_answer(answer)
    }

    /// Seals a workspace file as one of the run's new files, through the run's file provider
    /// and its limits, and answers its id; it reaches the outcome only if the run ends `Ok`.
    pub fn sandbox_collect(&mut self, path: &str) -> Result<Result<String, String>, HostDenied> {
        self.burn(per_byte(path.len()));
        let sandbox = self.sandbox()?;
        let opened = sandbox.open(path);
        let mut source = match self.sandbox_answer(opened)? {
            Ok(source) => source,
            Err(reason) => return Ok(Err(reason)),
        };
        let writer = match self.files_create()? {
            Ok(writer) => writer,
            Err(reason) => return Ok(Err(reason)),
        };
        let mut chunk = vec![0u8; MAX_READ_CHUNK as usize];
        loop {
            let n = match source.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => {
                    self.writer_close(writer);
                    return Err(self.sandbox_unavailable());
                }
            };
            if let Err(reason) = self.writer_write(writer, &chunk[..n])? {
                self.writer_close(writer);
                return Ok(Err(reason));
            }
        }
        let finished = self.writer_finish(writer)?;
        self.writer_close(writer);
        Ok(finished)
    }
}
