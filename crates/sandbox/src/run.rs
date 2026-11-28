//! One sandbox run on the node's side: its workspace, its cgroup, `exec` through init, and
//! workspace I/O that cannot be tricked into leaving the workspace.

use std::ffi::CString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::cgroup::Cgroup;
use crate::init::ID_RANGE;
use crate::workspace::Workspace;
use crate::{
    Config, ExecOutcome, ExecRequest, Image, Limits, SandboxError, Usage, INIT_ARG, MAX_ARGV,
    MAX_ENV, MAX_EXEC_MS, MAX_OUTPUT_BYTES, MAX_REQUEST_BYTES, MAX_STDIN_BYTES, NOBODY,
};

/// Most bytes of a run id the node passes; the id names the run's directory and cgroup.
const MAX_ID_BYTES: usize = 64;
/// How often `exec` checks its child against the deadline.
const POLL: Duration = Duration::from_millis(1);
/// The environment every command starts from, before the request's own.
const BASE_ENV: &[(&str, &str)] = &[
    ("PATH", "/usr/local/bin:/usr/bin:/bin"),
    ("HOME", "/workspace"),
    ("LANG", "C.UTF-8"),
];

fn invalid(msg: impl Into<String>) -> SandboxError {
    SandboxError::Invalid(msg.into())
}

fn unavailable(what: impl std::fmt::Display, e: impl std::fmt::Display) -> SandboxError {
    SandboxError::Unavailable(format!("{what}: {e}"))
}

/// One sandbox run: a fresh workspace and cgroup, removed with every process in them when the
/// run is dropped. Nothing carries over between runs.
pub struct Run {
    config: Config,
    image: Image,
    #[cfg(feature = "controls")]
    control: crate::Control,
    base: PathBuf,
    workspace: PathBuf,
    /// The workspace's files, as the node reads and writes them for the run.
    files: Workspace,
    root: PathBuf,
    cgroup: Cgroup,
    procs: Vec<CString>,
}

impl Run {
    /// Sets up a run named `id` (letters, digits, `-` and `_`): its workspace and cgroup.
    pub fn create(
        config: &Config,
        id: &str,
        image: Image,
        limits: &Limits,
    ) -> Result<Self, SandboxError> {
        let valid_id = !id.is_empty()
            && id.len() <= MAX_ID_BYTES
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !valid_id {
            return Err(invalid(format!("bad run id {id:?}")));
        }
        if limits.vcpus == 0 || limits.memory_bytes == 0 || limits.pids == 0 {
            return Err(invalid(
                "a run needs at least one vCPU, some memory and one process",
            ));
        }
        if !image.root.join("usr").is_dir() {
            return Err(SandboxError::Unavailable(format!(
                "image {} has no usr",
                image.root.display()
            )));
        }
        let base = config.base_dir.join(id);
        let workspace = base.join("ws");
        let root = base.join("root");
        fs::create_dir_all(&workspace).map_err(|e| unavailable(workspace.display(), e))?;
        fs::create_dir_all(&root).map_err(|e| unavailable(root.display(), e))?;
        let guard = BaseGuard(base.clone());

        if config.uid_base == 0 || config.uid_base.checked_add(ID_RANGE).is_none() {
            return Err(invalid(format!("bad uid base {}", config.uid_base)));
        }
        // The workspace belongs to the sandbox's `nobody`: the node's uid for it in the sandbox's
        // user namespace (the node's own `nobody` under the control that leaves that out).
        #[cfg(feature = "controls")]
        let owner = if limits.control == crate::Control::NoUserns {
            NOBODY
        } else {
            config.uid_base + NOBODY
        };
        #[cfg(not(feature = "controls"))]
        let owner = config.uid_base + NOBODY;
        let opts = CString::new(format!(
            "size={},mode=0755,uid={owner},gid={owner}",
            limits.workspace_bytes
        ))
        .map_err(|e| unavailable("workspace options", e))?;
        let target = path_c(&workspace)?;
        // SAFETY: every pointer is a NUL-terminated string that outlives the call.
        let r = unsafe {
            libc::mount(
                c"tmpfs".as_ptr(),
                target.as_ptr(),
                c"tmpfs".as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV,
                opts.as_ptr().cast(),
            )
        };
        if r != 0 {
            return Err(unavailable("mount workspace", io::Error::last_os_error()));
        }
        let mounted = MountGuard(workspace.clone());
        let files = Workspace::open(&workspace, limits.workspace_bytes, Some(owner))?;

        let cgroup = Cgroup::create(&config.cgroup_name, id, limits)?;
        let procs = cgroup.procs_files()?;
        // Set up: the run's own Drop cleans up from here.
        std::mem::forget(mounted);
        std::mem::forget(guard);
        Ok(Self {
            config: config.clone(),
            image,
            #[cfg(feature = "controls")]
            control: limits.control,
            base,
            workspace,
            files,
            root,
            cgroup,
            procs,
        })
    }

    /// Runs one command in a fresh sandbox over the run's workspace, and waits for it. A
    /// command still running at its timeout is killed with everything the run started.
    pub fn exec(&self, req: &ExecRequest) -> Result<ExecOutcome, SandboxError> {
        check_request(req)?;
        let started = Instant::now();
        #[cfg(feature = "controls")]
        let control = self.control.as_arg();
        #[cfg(not(feature = "controls"))]
        let control = "full";
        let mut cmd = Command::new(&self.config.init_exe);
        let _ = cmd
            .arg(INIT_ARG)
            .arg(control)
            .arg(self.config.uid_base.to_string())
            .arg(&self.image.root)
            .arg(&self.root)
            .arg(&self.workspace)
            .arg(req.cwd.as_deref().unwrap_or(""))
            .arg("--")
            .args(&req.argv)
            .env_clear()
            .envs(BASE_ENV.iter().copied())
            .envs(req.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let procs = self.procs.clone();
        // SAFETY: the closure runs between fork and exec and makes only async-signal-safe
        // calls (open, write, close) on memory allocated before the fork.
        unsafe {
            let _ = cmd.pre_exec(move || {
                // Join the run's cgroup ("0" = this process); init makes the namespaces.
                for p in &procs {
                    let fd = libc::open(p.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
                    if fd < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    let n = libc::write(fd, b"0".as_ptr().cast(), 1);
                    let _ = libc::close(fd);
                    if n != 1 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| unavailable("spawn sandbox", e))?;

        let (Some(mut stdin), Some(out), Some(err)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err(SandboxError::Unavailable("sandbox pipes".into()));
        };
        let input = req.stdin.clone();
        let writer = thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        let out_t = thread::spawn(move || read_capped(out, MAX_OUTPUT_BYTES));
        let err_t = thread::spawn(move || read_capped(err, MAX_OUTPUT_BYTES));

        let deadline =
            started + Duration::from_millis(u64::from(req.timeout_ms.clamp(1, MAX_EXEC_MS)));
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) if Instant::now() >= deadline => {
                    timed_out = true;
                    let _ = self.cgroup.kill_all();
                    break child.wait().map_err(|e| unavailable("wait", e))?;
                }
                Ok(None) => thread::sleep(POLL),
                Err(e) => {
                    let _ = self.cgroup.kill_all();
                    return Err(unavailable("wait", e));
                }
            }
        };
        // Whatever outlived the namespace's PID 1 is already gone; make sure, so the pipes close.
        let _ = self.cgroup.kill_all();
        let _ = writer.join();
        let (stdout, t1) = out_t.join().unwrap_or_default();
        let (stderr, t2) = err_t.join().unwrap_or_default();
        let exit_code = status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
        Ok(ExecOutcome {
            exit_code,
            stdout,
            stderr,
            truncated: t1 || t2,
            timed_out,
            wall_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        })
    }

    /// What the run has used so far.
    pub fn usage(&self) -> Usage {
        self.cgroup.usage()
    }

    /// Writes `bytes` to `path` in the workspace, replacing a regular file there.
    pub fn write(&self, path: &str, bytes: &[u8]) -> Result<(), SandboxError> {
        self.files.write(path, bytes)
    }

    /// Reads at most `max` bytes (and at most [`MAX_READ_BYTES`](crate::MAX_READ_BYTES)) of a
    /// regular file in the workspace.
    pub fn read(&self, path: &str, max: u32) -> Result<Vec<u8>, SandboxError> {
        self.files.read(path, max)
    }

    /// Opens a regular file in the workspace to take out of the run, with its length.
    pub fn collect(&self, path: &str) -> Result<(File, u64), SandboxError> {
        self.files.collect(path)
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = self.cgroup.kill_all();
        if let Ok(ws) = path_c(&self.workspace) {
            // SAFETY: a NUL-terminated string that outlives the call.
            let _ = unsafe { libc::umount2(ws.as_ptr(), libc::MNT_DETACH) };
        }
        let _ = fs::remove_dir_all(&self.base);
        // The cgroup's own Drop removes it.
    }
}

/// Removes a run's directory if setting it up fails part way.
struct BaseGuard(PathBuf);

impl Drop for BaseGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Unmounts the workspace if setting the run up fails after mounting it.
struct MountGuard(PathBuf);

impl Drop for MountGuard {
    fn drop(&mut self) {
        if let Ok(ws) = path_c(&self.0) {
            // SAFETY: a NUL-terminated string that outlives the call.
            let _ = unsafe { libc::umount2(ws.as_ptr(), libc::MNT_DETACH) };
        }
    }
}

fn check_request(req: &ExecRequest) -> Result<(), SandboxError> {
    if req.argv.is_empty() {
        return Err(invalid("empty argv"));
    }
    if req.argv.len() > MAX_ARGV || req.env.len() > MAX_ENV {
        return Err(invalid("too many arguments or environment variables"));
    }
    if req.stdin.len() > MAX_STDIN_BYTES {
        return Err(invalid("stdin too large"));
    }
    let bytes: usize = req.argv.iter().map(String::len).sum::<usize>()
        + req
            .env
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>();
    if bytes > MAX_REQUEST_BYTES {
        return Err(invalid("arguments and environment too large"));
    }
    if req.argv.iter().any(|a| a.contains('\0')) {
        return Err(invalid("argv holds a NUL byte"));
    }
    for (k, v) in &req.env {
        if k.is_empty() || k.contains('=') || k.contains('\0') || v.contains('\0') {
            return Err(invalid(format!("bad env var {k:?}")));
        }
    }
    if let Some(cwd) = &req.cwd {
        if cwd.contains('\0') || cwd.split('/').any(|c| c == "..") {
            return Err(invalid("cwd must stay in the workspace"));
        }
    }
    Ok(())
}

fn read_capped(mut r: impl Read, cap: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    let mut truncated = false;
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = cap.saturating_sub(out.len());
                out.extend_from_slice(&buf[..n.min(room)]);
                truncated |= n > room;
            }
        }
    }
    (out, truncated)
}

fn path_c(p: &Path) -> Result<CString, SandboxError> {
    CString::new(p.as_os_str().as_encoded_bytes())
        .map_err(|_| invalid(format!("{} holds a NUL byte", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(argv: &[&str]) -> ExecRequest {
        ExecRequest {
            argv: argv.iter().map(|s| (*s).to_owned()).collect(),
            ..ExecRequest::default()
        }
    }

    #[test]
    fn requests_outside_the_limits_are_refused_before_anything_runs() {
        assert!(check_request(&req(&["true"])).is_ok());
        let refused = |r: ExecRequest| matches!(check_request(&r), Err(SandboxError::Invalid(_)));
        assert!(refused(req(&[])));
        assert!(refused(req(&["a\0b"])));
        assert!(refused(ExecRequest {
            cwd: Some("a/../../etc".into()),
            ..req(&["true"])
        }));
        for (k, v) in [("", "x"), ("A=B", "x"), ("A", "x\0y")] {
            assert!(refused(ExecRequest {
                env: vec![(k.into(), v.into())],
                ..req(&["true"])
            }));
        }
        assert!(refused(ExecRequest {
            stdin: vec![0; MAX_STDIN_BYTES + 1],
            ..req(&["true"])
        }));
        assert!(refused(req(&vec!["x"; MAX_ARGV + 1])));
        let big = "x".repeat(MAX_REQUEST_BYTES + 1);
        assert!(refused(req(&[big.as_str()])));
    }

    #[test]
    fn run_ids_name_only_a_directory_of_their_own() {
        let config = Config {
            base_dir: std::env::temp_dir().join("calimero-sandbox-unit"),
            ..Config::default()
        };
        for id in ["", "../x", "a/b", "a b", &"x".repeat(MAX_ID_BYTES + 1)] {
            let r = Run::create(&config, id, Image::host(), &Limits::default());
            assert!(matches!(r, Err(SandboxError::Invalid(_))), "{id:?}");
        }
    }

    #[test]
    fn output_past_the_cap_is_drained_and_marked() {
        let (out, truncated) = read_capped(&[7u8; 100][..], 64);
        assert_eq!((out.len(), truncated), (64, true));
        let (out, truncated) = read_capped(&[7u8; 64][..], 64);
        assert_eq!((out.len(), truncated), (64, false));
    }
}
