//! A run's cgroup: memory, process and CPU caps for everything the run starts, a way to kill
//! all of it, and what it used.
//!
//! cgroup v2 (one hierarchy, `cgroup.kill`) is what a job node runs; v1 (one hierarchy per
//! controller, a kill loop) is kept for development hosts that mount only v1 controllers.

use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::{Limits, SandboxError, Usage};

const ROOT: &str = "/sys/fs/cgroup";
const V2_CONTROLLERS: &[&str] = &["memory", "pids", "cpu"];
const V1_CONTROLLERS: &[&str] = &["memory", "pids", "cpu", "cpuacct"];
/// The CFS period both versions use, in microseconds.
const CPU_PERIOD_US: u64 = 100_000;
/// How long dropping a run waits for its processes to die before giving up on its cgroups.
const KILL_ROUNDS: u32 = 500;
const KILL_PAUSE: Duration = Duration::from_millis(2);

pub(crate) enum Cgroup {
    V2 { dir: PathBuf },
    V1 { dirs: Vec<(&'static str, PathBuf)> },
}

fn unavailable(what: impl std::fmt::Display, e: impl std::fmt::Display) -> SandboxError {
    SandboxError::Unavailable(format!("{what}: {e}"))
}

fn write(path: &Path, value: &str) -> Result<(), SandboxError> {
    fs::write(path, value).map_err(|e| unavailable(path.display(), e))
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Whether the unified hierarchy at `/sys/fs/cgroup` offers every controller a run needs.
fn v2_available() -> bool {
    let Ok(controllers) = fs::read_to_string(Path::new(ROOT).join("cgroup.controllers")) else {
        return false;
    };
    let have: Vec<&str> = controllers.split_whitespace().collect();
    V2_CONTROLLERS.iter().all(|c| have.contains(c))
}

/// Enables the run controllers for `dir`'s children.
fn enable_controllers(dir: &Path) -> Result<(), SandboxError> {
    let wanted = V2_CONTROLLERS
        .iter()
        .map(|c| format!("+{c}"))
        .collect::<Vec<_>>()
        .join(" ");
    write(&dir.join("cgroup.subtree_control"), &wanted)
}

impl Cgroup {
    pub(crate) fn create(parent: &str, id: &str, limits: &Limits) -> Result<Self, SandboxError> {
        if v2_available() {
            Self::create_v2(parent, id, limits)
        } else {
            Self::create_v1(parent, id, limits)
        }
    }

    fn create_v2(parent: &str, id: &str, limits: &Limits) -> Result<Self, SandboxError> {
        let root = Path::new(ROOT);
        let parent = root.join(parent);
        enable_controllers(root)?;
        fs::create_dir_all(&parent).map_err(|e| unavailable(parent.display(), e))?;
        enable_controllers(&parent)?;
        let dir = parent.join(id);
        fs::create_dir(&dir).map_err(|e| unavailable(dir.display(), e))?;
        let cgroup = Self::V2 { dir: dir.clone() };
        // From here on, dropping `cgroup` removes the directory if a limit fails to apply.
        write(&dir.join("memory.max"), &limits.memory_bytes.to_string())?;
        write(&dir.join("memory.swap.max"), "0").or_else(|e| {
            // Without swap accounting there is no swap to cap.
            if dir.join("memory.swap.max").exists() {
                Err(e)
            } else {
                Ok(())
            }
        })?;
        write(&dir.join("pids.max"), &limits.pids.to_string())?;
        let quota = u64::from(limits.vcpus) * CPU_PERIOD_US;
        write(&dir.join("cpu.max"), &format!("{quota} {CPU_PERIOD_US}"))?;
        Ok(cgroup)
    }

    fn create_v1(parent: &str, id: &str, limits: &Limits) -> Result<Self, SandboxError> {
        let mut cgroup = Self::V1 { dirs: Vec::new() };
        let mut mounts: Vec<PathBuf> = Vec::new();
        for ctl in V1_CONTROLLERS {
            let mount = Path::new(ROOT).join(ctl);
            let real = fs::canonicalize(&mount).map_err(|e| unavailable(mount.display(), e))?;
            // `cpu` and `cpuacct` are often one hierarchy under two names; join it once.
            if mounts.contains(&real) {
                continue;
            }
            mounts.push(real);
            let dir = mount.join(parent).join(id);
            fs::create_dir_all(&dir).map_err(|e| unavailable(dir.display(), e))?;
            if let Self::V1 { dirs } = &mut cgroup {
                dirs.push((ctl, dir));
            }
        }
        let memory = cgroup.v1_dir("memory")?;
        write(
            &memory.join("memory.limit_in_bytes"),
            &limits.memory_bytes.to_string(),
        )?;
        // Swap accounting may be off, in which case there is no swap to cap.
        let _ = fs::write(
            memory.join("memory.memsw.limit_in_bytes"),
            limits.memory_bytes.to_string(),
        );
        let _ = fs::write(memory.join("memory.swappiness"), "0");
        write(
            &cgroup.v1_dir("pids")?.join("pids.max"),
            &limits.pids.to_string(),
        )?;
        let cpu = cgroup.v1_dir("cpu")?;
        write(&cpu.join("cpu.cfs_period_us"), &CPU_PERIOD_US.to_string())?;
        let quota = u64::from(limits.vcpus) * CPU_PERIOD_US;
        write(&cpu.join("cpu.cfs_quota_us"), &quota.to_string())?;
        Ok(cgroup)
    }

    fn v1_dir(&self, ctl: &str) -> Result<&Path, SandboxError> {
        let Self::V1 { dirs } = self else {
            return Err(SandboxError::Unavailable(format!("no v1 {ctl} cgroup")));
        };
        // `cpuacct` may share `cpu`'s hierarchy, joined under `cpu`'s name.
        let find = |name: &str| dirs.iter().find(|(c, _)| *c == name);
        find(ctl)
            .or_else(|| if ctl == "cpuacct" { find("cpu") } else { None })
            .map(|(_, d)| d.as_path())
            .ok_or_else(|| SandboxError::Unavailable(format!("no v1 {ctl} cgroup")))
    }

    /// The `cgroup.procs` files a new process writes `0` to, to join the run.
    pub(crate) fn procs_files(&self) -> Result<Vec<CString>, SandboxError> {
        let dirs: Vec<&Path> = match self {
            Self::V2 { dir } => vec![dir],
            Self::V1 { dirs } => dirs.iter().map(|(_, d)| d.as_path()).collect(),
        };
        dirs.into_iter()
            .map(|d| {
                CString::new(d.join("cgroup.procs").as_os_str().as_encoded_bytes())
                    .map_err(|e| unavailable(d.display(), e))
            })
            .collect()
    }

    /// Lists the processes still in the run.
    fn pids(&self) -> Vec<i32> {
        let procs = match self {
            Self::V2 { dir } => dir.join("cgroup.procs"),
            Self::V1 { .. } => match self.v1_dir("pids") {
                Ok(d) => d.join("cgroup.procs"),
                Err(_) => return Vec::new(),
            },
        };
        fs::read_to_string(procs)
            .map(|l| l.lines().filter_map(|p| p.trim().parse().ok()).collect())
            .unwrap_or_default()
    }

    /// Kills every process in the run and waits until none is left. Returns whether it emptied.
    pub(crate) fn kill_all(&self) -> bool {
        if let Self::V2 { dir } = self {
            let _ = fs::write(dir.join("cgroup.kill"), "1");
        }
        for _ in 0..KILL_ROUNDS {
            let pids = self.pids();
            if pids.is_empty() {
                return true;
            }
            if matches!(self, Self::V1 { .. }) {
                for pid in pids {
                    // SAFETY: kill(2) with a pid read from the run's own cgroup; no memory involved.
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
            thread::sleep(KILL_PAUSE);
        }
        false
    }

    /// CPU time and peak memory of everything the run has started.
    pub(crate) fn usage(&self) -> Usage {
        match self {
            Self::V2 { dir } => {
                let cpu_us = fs::read_to_string(dir.join("cpu.stat"))
                    .ok()
                    .and_then(|s| {
                        s.lines()
                            .find_map(|l| l.strip_prefix("usage_usec "))
                            .and_then(|v| v.trim().parse().ok())
                    })
                    .unwrap_or(0);
                Usage {
                    cpu_us,
                    peak_memory_bytes: read_u64(&dir.join("memory.peak")).unwrap_or(0),
                }
            }
            Self::V1 { .. } => Usage {
                cpu_us: self
                    .v1_dir("cpuacct")
                    .ok()
                    .and_then(|d| read_u64(&d.join("cpuacct.usage")))
                    .map_or(0, |ns| ns / 1_000),
                peak_memory_bytes: self
                    .v1_dir("memory")
                    .ok()
                    .and_then(|d| read_u64(&d.join("memory.max_usage_in_bytes")))
                    .unwrap_or(0),
            },
        }
    }
}

impl Drop for Cgroup {
    fn drop(&mut self) {
        self.kill_all();
        let dirs: Vec<PathBuf> = match self {
            Self::V2 { dir } => vec![dir.clone()],
            Self::V1 { dirs } => dirs.iter().map(|(_, d)| d.clone()).collect(),
        };
        for dir in dirs {
            // An emptied cgroup can still be busy for a moment while its last task exits.
            for _ in 0..KILL_ROUNDS {
                if fs::remove_dir(&dir).is_ok() || !dir.exists() {
                    break;
                }
                thread::sleep(KILL_PAUSE);
            }
        }
    }
}
