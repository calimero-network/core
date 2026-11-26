//! One sandbox run on the host side: a size-capped tmpfs workspace, cgroups (v1 here;
//! cgroup v2 on a real job node) that cap memory, processes and CPU for everything the run
//! starts, `exec` into fresh namespaces through `init`, and host-side workspace I/O that
//! cannot be tricked into leaving the workspace.

use std::ffi::CString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::init::{INIT_ARG, NOBODY};

#[derive(Clone, Debug)]
pub struct Limits {
    pub memory_bytes: u64,
    pub pids: u32,
    /// Percent of one CPU.
    pub cpu_percent: u32,
    pub workspace_bytes: u64,
    pub output_cap: usize,
    pub max_timeout_ms: u32,
    /// "full"; or, to check the escape suite has teeth, "no-seccomp" or "root"
    /// (no privilege drop, no seccomp).
    pub control: String,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 256 << 20,
            pids: 64,
            cpu_percent: 100,
            workspace_bytes: 64 << 20,
            output_cap: 1 << 20,
            max_timeout_ms: 60_000,
            control: std::env::var("SANDBOX_PROTO_CONTROL").unwrap_or_else(|_| "full".into()),
        }
    }
}

pub struct ExecOutcome {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    pub timed_out: bool,
    pub wall_us: u64,
}

pub struct Run {
    pub limits: Limits,
    base: PathBuf,
    workspace: PathBuf,
    workspace_fd: OwnedFd,
    root: PathBuf,
    outputs: PathBuf,
    cgroups: Vec<PathBuf>,
    procs: Vec<CString>,
}

const BASE: &str = "/run/sandbox-proto";
const CONTROLLERS: &[&str] = &["memory", "pids", "cpu"];

impl Run {
    pub fn create(limits: Limits, outputs: &Path) -> Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let id = format!("run-{}-{nanos}", std::process::id());
        let base = Path::new(BASE).join(&id);
        let workspace = base.join("ws");
        let root = base.join("root");
        fs::create_dir_all(&workspace)?;
        fs::create_dir_all(&root)?;
        fs::create_dir_all(outputs)?;

        let opts = CString::new(format!(
            "size={},mode=0755,uid={NOBODY},gid={NOBODY}",
            limits.workspace_bytes
        ))?;
        let (src, ty, target) = (
            CString::new("tmpfs")?,
            CString::new("tmpfs")?,
            CString::new(workspace.as_os_str().as_encoded_bytes())?,
        );
        let r = unsafe {
            libc::mount(
                src.as_ptr(),
                target.as_ptr(),
                ty.as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV,
                opts.as_ptr().cast(),
            )
        };
        if r != 0 {
            return Err(io::Error::last_os_error()).context("mount workspace tmpfs");
        }
        let workspace_fd = open_raw(
            libc::AT_FDCWD,
            &workspace,
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
            0,
            0,
        )?;

        let mut cgroups = Vec::new();
        for ctl in CONTROLLERS {
            let dir = Path::new("/sys/fs/cgroup")
                .join(ctl)
                .join("sandbox-proto")
                .join(&id);
            fs::create_dir_all(&dir).with_context(|| format!("cgroup {}", dir.display()))?;
            cgroups.push(dir);
        }
        let set = |dir: &Path, file: &str, v: String| -> Result<()> {
            fs::write(dir.join(file), v).with_context(|| format!("{}/{file}", dir.display()))
        };
        set(
            &cgroups[0],
            "memory.limit_in_bytes",
            limits.memory_bytes.to_string(),
        )?;
        let _ = set(
            &cgroups[0],
            "memory.memsw.limit_in_bytes",
            limits.memory_bytes.to_string(),
        );
        let _ = set(&cgroups[0], "memory.swappiness", "0".into());
        set(&cgroups[1], "pids.max", limits.pids.to_string())?;
        set(&cgroups[2], "cpu.cfs_period_us", "100000".into())?;
        set(
            &cgroups[2],
            "cpu.cfs_quota_us",
            (limits.cpu_percent as u64 * 1000).to_string(),
        )?;
        let procs = cgroups
            .iter()
            .map(|d| CString::new(d.join("cgroup.procs").as_os_str().as_encoded_bytes()))
            .collect::<Result<_, _>>()?;

        Ok(Self {
            limits,
            base,
            workspace,
            workspace_fd,
            root,
            outputs: outputs.to_owned(),
            cgroups,
            procs,
        })
    }

    pub fn exec(
        &mut self,
        argv: &[String],
        env: &[(String, String)],
        cwd: Option<&str>,
        stdin: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<ExecOutcome, String> {
        if argv.is_empty() {
            return Err("empty argv".into());
        }
        let cwd = cwd.unwrap_or("");
        if cwd.split('/').any(|c| c == "..") {
            return Err("cwd may not contain ..".into());
        }
        let started = Instant::now();
        let mut cmd = Command::new("/proc/self/exe");
        cmd.arg(INIT_ARG)
            .arg(&self.limits.control)
            .arg(&self.root)
            .arg(&self.workspace)
            .arg(cwd)
            .arg("--")
            .args(argv)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", "/workspace")
            .env("LANG", "C.UTF-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            if k.is_empty() || k.contains('=') || k.contains('\0') || v.contains('\0') {
                return Err(format!("bad env var {k:?}"));
            }
            cmd.env(k, v);
        }
        let procs = self.procs.clone();
        // After fork, before exec: join the run's cgroups ("0" = this process), then enter
        // new namespaces. Only async-signal-safe calls; nothing allocates.
        unsafe {
            cmd.pre_exec(move || {
                for p in &procs {
                    let fd = libc::open(p.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
                    if fd < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    let n = libc::write(fd, b"0".as_ptr().cast(), 1);
                    libc::close(fd);
                    if n != 1 {
                        return Err(io::Error::last_os_error());
                    }
                }
                let flags = libc::CLONE_NEWNS
                    | libc::CLONE_NEWPID
                    | libc::CLONE_NEWNET
                    | libc::CLONE_NEWIPC
                    | libc::CLONE_NEWUTS
                    | libc::CLONE_NEWCGROUP;
                if libc::unshare(flags) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn sandbox: {e}"))?;

        let mut child_stdin = child.stdin.take().expect("piped");
        let writer = thread::spawn(move || {
            let _ = child_stdin.write_all(&stdin);
        });
        let cap = self.limits.output_cap;
        let out = child.stdout.take().expect("piped");
        let err = child.stderr.take().expect("piped");
        let out_t = thread::spawn(move || read_capped(out, cap));
        let err_t = thread::spawn(move || read_capped(err, cap));

        let timeout = Duration::from_millis(timeout_ms.clamp(1, self.limits.max_timeout_ms) as u64);
        let deadline = started + timeout;
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) if Instant::now() >= deadline => {
                    timed_out = true;
                    self.kill_all();
                    break child.wait().map_err(|e| e.to_string())?;
                }
                Ok(None) => thread::sleep(Duration::from_millis(1)),
                Err(e) => return Err(e.to_string()),
            }
        };
        // Anything that outlived the namespace's PID 1 is already gone; make sure.
        self.kill_all();
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
            wall_us: started.elapsed().as_micros() as u64,
        })
    }

    /// SIGKILL every process in the run's cgroup until it is empty.
    fn kill_all(&self) {
        let procs = self.cgroups[1].join("cgroup.procs");
        for _ in 0..200 {
            let Ok(list) = fs::read_to_string(&procs) else {
                return;
            };
            let pids: Vec<i32> = list.lines().filter_map(|l| l.trim().parse().ok()).collect();
            if pids.is_empty() {
                return;
            }
            for pid in pids {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    // ---- workspace I/O, done by the host (root) on the sandbox's behalf ------------------

    /// Opens `path` under the workspace with openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS
    /// | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV): absolute paths, `..` escapes, symlinks in
    /// any component and other mounts are refused by the kernel, so a sandbox cannot plant
    /// a link that makes the host read or write outside the workspace.
    fn open_beneath(&self, path: &str, flags: i32, mode: u64) -> Result<File, String> {
        let fd = open_raw(
            self.workspace_fd.as_raw_fd(),
            Path::new(path),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode,
            RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV,
        )
        .map_err(|e| format!("{path}: {}", e.root_cause()))?;
        Ok(File::from(fd))
    }

    fn require_regular(f: &File, path: &str) -> Result<u64, String> {
        let m = f.metadata().map_err(|e| e.to_string())?;
        if !m.file_type().is_file() {
            return Err(format!("{path}: not a regular file"));
        }
        Ok(m.len())
    }

    pub fn write(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() as u64 > self.limits.workspace_bytes {
            return Err("file larger than the workspace".into());
        }
        // O_NONBLOCK: opening a FIFO the sandbox planted must not block the host.
        let mut f = self.open_beneath(
            path,
            libc::O_WRONLY | libc::O_CREAT | libc::O_NONBLOCK,
            0o644,
        )?;
        Self::require_regular(&f, path)?;
        f.set_len(0).map_err(|e| e.to_string())?;
        f.write_all(bytes).map_err(|e| format!("{path}: {e}"))?;
        let r = unsafe { libc::fchown(f.as_raw_fd(), NOBODY, NOBODY) };
        if r != 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(())
    }

    pub fn read(&self, path: &str, max: u32) -> Result<Vec<u8>, String> {
        let f = self.open_beneath(path, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        Self::require_regular(&f, path)?;
        let mut buf = Vec::new();
        f.take(max.min(16 << 20) as u64)
            .read_to_end(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }

    pub fn collect(&self, path: &str) -> Result<String, String> {
        let mut f = self.open_beneath(path, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        Self::require_regular(&f, path)?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let id = hex(&Sha256::digest(&bytes));
        fs::write(self.outputs.join(&id), &bytes).map_err(|e| e.to_string())?;
        Ok(id)
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.kill_all();
        for dir in &self.cgroups {
            for _ in 0..100 {
                if fs::remove_dir(dir).is_ok() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
        if let Ok(ws) = CString::new(self.workspace.as_os_str().as_encoded_bytes()) {
            unsafe { libc::umount2(ws.as_ptr(), libc::MNT_DETACH) };
        }
        let _ = fs::remove_dir_all(&self.base);
    }
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

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

const RESOLVE_NO_XDEV: u64 = 0x01;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

fn open_raw(dirfd: i32, path: &Path, flags: i32, mode: u64, resolve: u64) -> Result<OwnedFd> {
    let p = CString::new(path.as_os_str().as_encoded_bytes())?;
    let how = OpenHow {
        flags: flags as u64,
        mode,
        resolve,
    };
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dirfd,
            p.as_ptr(),
            &how as *const OpenHow,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}
