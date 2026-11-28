//! The node's reads and writes of a run's workspace, on the sandbox's behalf.
//!
//! The sandbox controls every path in its workspace, so each open goes through
//! `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV)`:
//! absolute paths, `..` escapes, a symlink in any component and other mounts are refused by the
//! kernel, with no check-then-use race. Every open is `O_NONBLOCK`, so a planted FIFO cannot
//! park the node, and only a regular file is read or written. `fuzz/fuzz_targets/
//! sandbox_workspace.rs` attacks it with planted entries.

use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

use crate::{SandboxError, MAX_READ_BYTES};

fn invalid(msg: impl Into<String>) -> SandboxError {
    SandboxError::Invalid(msg.into())
}

/// A workspace directory the node acts on.
pub struct Workspace {
    dir: OwnedFd,
    /// Most bytes one `write` puts in it.
    max_bytes: u64,
    /// The uid and gid a written file is given (the sandbox's `nobody`); `None` keeps the
    /// writer's.
    owner: Option<u32>,
}

impl Workspace {
    /// The workspace at `dir`, whose files are at most `max_bytes` and given to `owner`.
    pub fn open(dir: &Path, max_bytes: u64, owner: Option<u32>) -> Result<Self, SandboxError> {
        let dir = open_raw(
            libc::AT_FDCWD,
            dir,
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
            0,
            0,
        )
        .map_err(|e| SandboxError::Unavailable(format!("open workspace: {e}")))?;
        Ok(Self {
            dir,
            max_bytes,
            owner,
        })
    }

    /// Writes `bytes` to `path`, replacing a regular file there.
    pub fn write(&self, path: &str, bytes: &[u8]) -> Result<(), SandboxError> {
        if bytes.len() as u64 > self.max_bytes {
            return Err(invalid("file larger than the workspace"));
        }
        let mut f = self.open_beneath(
            path,
            libc::O_WRONLY | libc::O_CREAT | libc::O_NONBLOCK,
            0o644,
        )?;
        let _ = require_regular(&f, path)?;
        f.set_len(0).map_err(|e| invalid(format!("{path}: {e}")))?;
        f.write_all(bytes)
            .map_err(|e| invalid(format!("{path}: {e}")))?;
        if let Some(owner) = self.owner {
            // SAFETY: fchown on a file descriptor this function owns.
            if unsafe { libc::fchown(f.as_raw_fd(), owner, owner) } != 0 {
                return Err(SandboxError::Unavailable(format!(
                    "{path}: {}",
                    io::Error::last_os_error()
                )));
            }
        }
        Ok(())
    }

    /// Reads at most `max` bytes (and at most [`MAX_READ_BYTES`]) of a regular file.
    pub fn read(&self, path: &str, max: u32) -> Result<Vec<u8>, SandboxError> {
        let f = self.open_beneath(path, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let _ = require_regular(&f, path)?;
        let mut buf = Vec::new();
        let _ = f
            .take(u64::from(max.min(MAX_READ_BYTES)))
            .read_to_end(&mut buf)
            .map_err(|e| invalid(format!("{path}: {e}")))?;
        Ok(buf)
    }

    /// Opens a regular file to take out of the run, with its length.
    pub fn collect(&self, path: &str) -> Result<(File, u64), SandboxError> {
        let f = self.open_beneath(path, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let len = require_regular(&f, path)?;
        Ok((f, len))
    }

    fn open_beneath(&self, path: &str, flags: i32, mode: u64) -> Result<File, SandboxError> {
        let fd = open_raw(
            self.dir.as_raw_fd(),
            Path::new(path),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode,
            RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV,
        )
        .map_err(|e| invalid(format!("{path}: {e}")))?;
        Ok(File::from(fd))
    }
}

fn require_regular(f: &File, path: &str) -> Result<u64, SandboxError> {
    let m = f.metadata().map_err(|e| invalid(format!("{path}: {e}")))?;
    if !m.file_type().is_file() {
        return Err(invalid(format!("{path}: not a regular file")));
    }
    Ok(m.len())
}

const RESOLVE_NO_XDEV: u64 = 0x01;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;

/// `struct open_how` of openat2(2).
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

fn open_raw(dirfd: i32, path: &Path, flags: i32, mode: u64, resolve: u64) -> io::Result<OwnedFd> {
    let p = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let how = OpenHow {
        flags: flags as u64,
        mode,
        resolve,
    };
    // SAFETY: `p` and `how` outlive the call, and the size is `how`'s.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dirfd,
            p.as_ptr(),
            &how as *const OpenHow,
            size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = i32::try_from(fd).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    // SAFETY: openat2 returned a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
