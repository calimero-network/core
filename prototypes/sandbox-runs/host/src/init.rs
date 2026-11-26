//! The sandbox's init, run as `<host binary> __sandbox_init <root> <workspace> <cwd> -- argv...`.
//!
//! The host spawns it already inside the run's cgroups and in new mount, network, IPC,
//! UTS and cgroup namespaces, with a new PID namespace for its children (see
//! `sandbox::Run::exec`). From there:
//!
//! 1. fork: the child is PID 1 of the new PID namespace;
//! 2. PID 1 builds a fresh root on tmpfs (read-only /usr, a minimal /etc and /dev,
//!    /proc of the new namespace, /tmp, the run's /workspace), pivots into it and detaches
//!    the old root, so nothing of the host's filesystem stays reachable;
//! 3. PID 1 drops every capability (bounding set included), becomes `nobody`, sets
//!    no_new_privs and rlimits, and installs the seccomp filters;
//! 4. PID 1 forks the command and reaps until it exits; when PID 1 exits, the kernel
//!    kills whatever is left in the namespace.
//!
//! The outer process only waits for PID 1 and passes its status on (128+n for signal n).

use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

pub const INIT_ARG: &str = "__sandbox_init";
pub const NOBODY: u32 = 65534;

pub fn main(args: &[String]) -> ! {
    let code = match run(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("sandbox-init: {e:#}");
            125
        }
    };
    std::process::exit(code)
}

struct Args<'a> {
    control: &'a str,
    root: &'a str,
    workspace: &'a str,
    cwd: &'a str,
    argv: &'a [String],
}

fn parse(args: &[String]) -> Result<Args<'_>> {
    match args {
        [control, root, workspace, cwd, sep, argv @ ..] if sep == "--" && !argv.is_empty() => {
            Ok(Args {
                control,
                root,
                workspace,
                cwd,
                argv,
            })
        }
        _ => bail!("usage: {INIT_ARG} <control> <root> <workspace> <cwd> -- argv..."),
    }
}

fn run(args: &[String]) -> Result<i32> {
    let args = parse(args)?;
    // Single-threaded here (a fresh exec), so fork is safe.
    match unsafe { libc::fork() } {
        -1 => Err(io::Error::last_os_error()).context("fork pid 1"),
        0 => {
            let code = match pid1(&args) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("sandbox-init: {e:#}");
                    125
                }
            };
            unsafe { libc::_exit(code) }
        }
        pid => wait_for(pid),
    }
}

fn wait_for(pid: libc::pid_t) -> Result<i32> {
    let mut status = 0;
    loop {
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid {
            return Ok(decode(status));
        }
        if r == -1 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return Err(io::Error::last_os_error()).context("waitpid");
        }
    }
}

fn decode(status: i32) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        255
    }
}

fn pid1(args: &Args<'_>) -> Result<i32> {
    setup_fs(args.root, args.workspace).context("filesystem")?;
    let host = b"sandbox";
    unsafe { libc::sethostname(host.as_ptr().cast(), host.len()) };
    let _ = loopback_up(); // only lo exists in the new network namespace; best effort
    let cwd = Path::new("/workspace").join(args.cwd.trim_start_matches('/'));
    std::env::set_current_dir(&cwd).with_context(|| format!("cwd {}", cwd.display()))?;
    // Controls exist only to prove the escape suite catches a missing layer.
    if args.control != "root" {
        drop_privileges().context("drop privileges")?;
    }
    set_rlimits();
    if args.control == "full" {
        install_seccomp().context("seccomp")?;
    }

    let argv: Vec<CString> = args
        .argv
        .iter()
        .map(|a| CString::new(a.as_bytes()))
        .collect::<Result<_, _>>()
        .map_err(|_| anyhow!("argv holds a NUL byte"))?;
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());

    let child = unsafe { libc::fork() };
    match child {
        -1 => Err(io::Error::last_os_error()).context("fork command"),
        0 => unsafe {
            libc::execvp(ptrs[0], ptrs.as_ptr());
            let msg = b"sandbox-init: exec failed\n";
            libc::write(2, msg.as_ptr().cast(), msg.len());
            libc::_exit(127)
        },
        child => {
            // PID 1 reaps orphans; the run's status is the command's.
            let mut status = 0;
            loop {
                let r = unsafe { libc::waitpid(-1, &mut status, 0) };
                if r == child {
                    return Ok(decode(status));
                }
                if r == -1 {
                    let e = io::Error::last_os_error();
                    if e.raw_os_error() != Some(libc::EINTR) {
                        return Err(e).context("reap");
                    }
                }
            }
        }
    }
}

// ---- filesystem -------------------------------------------------------------------------

fn c(p: impl AsRef<Path>) -> CString {
    CString::new(p.as_ref().as_os_str().as_encoded_bytes()).expect("path without NUL")
}

fn mount(
    src: Option<&str>,
    target: &Path,
    fstype: Option<&str>,
    flags: libc::c_ulong,
    data: Option<&str>,
) -> Result<()> {
    let src = src.map(c);
    let fstype = fstype.map(c);
    let data = data.map(c);
    let target_c = c(target);
    let r = unsafe {
        libc::mount(
            src.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            target_c.as_ptr(),
            fstype.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            flags,
            data.as_ref()
                .map_or(std::ptr::null(), |s| s.as_ptr().cast()),
        )
    };
    if r != 0 {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("mount {}", target.display()));
    }
    Ok(())
}

/// Bind `src` at `dst` read-only, nosuid, nodev.
fn bind_ro(src: &str, dst: &Path) -> Result<()> {
    if Path::new(src).is_dir() {
        fs::create_dir_all(dst)?;
    } else {
        fs::write(dst, b"")?;
    }
    mount(Some(src), dst, None, libc::MS_BIND | libc::MS_REC, None)?;
    mount(
        None,
        dst,
        None,
        libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
        None,
    )
}

fn setup_fs(root: &str, workspace: &str) -> Result<()> {
    let root = Path::new(root);
    // Nothing we mount propagates back to the host.
    mount(
        None,
        Path::new("/"),
        None,
        libc::MS_REC | libc::MS_PRIVATE,
        None,
    )?;
    mount(
        Some("tmpfs"),
        root,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NODEV,
        Some("size=4m,mode=0755"),
    )?;

    // The "image": the host's /usr, read-only. A real job node mounts the app's pinned
    // rootfs image here instead.
    bind_ro("/usr", &root.join("usr"))?;
    for (link, to) in [
        ("bin", "usr/bin"),
        ("lib", "usr/lib"),
        ("lib64", "usr/lib64"),
        ("sbin", "usr/sbin"),
    ] {
        symlink(to, root.join(link))?;
    }

    // A minimal /etc: no host users, hosts, secrets or proxy settings.
    let etc = root.join("etc");
    fs::create_dir_all(&etc)?;
    fs::write(
        etc.join("passwd"),
        "root:x:0:0:root:/:/usr/sbin/nologin\nnobody:x:65534:65534:nobody:/workspace:/bin/sh\n",
    )?;
    fs::write(etc.join("group"), "root:x:0:\nnogroup:x:65534:\n")?;
    fs::write(etc.join("hosts"), "127.0.0.1 localhost sandbox\n")?;
    fs::write(etc.join("resolv.conf"), "")?;
    for f in ["/etc/ld.so.cache", "/etc/alternatives", "/etc/ssl/certs"] {
        if Path::new(f).exists() {
            bind_ro(f, &root.join(f.trim_start_matches('/')))?;
        }
    }

    // /proc of the new PID namespace: only the sandbox's own processes.
    let proc_ = root.join("proc");
    fs::create_dir_all(&proc_)?;
    mount(
        Some("proc"),
        &proc_,
        Some("proc"),
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        None,
    )?;

    // A minimal /dev.
    let dev = root.join("dev");
    fs::create_dir_all(&dev)?;
    mount(
        Some("tmpfs"),
        &dev,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NOEXEC,
        Some("size=64k,mode=0755"),
    )?;
    for n in ["null", "zero", "full", "random", "urandom"] {
        let dst = dev.join(n);
        fs::write(&dst, b"")?;
        mount(Some(&format!("/dev/{n}")), &dst, None, libc::MS_BIND, None)?;
    }
    for (link, to) in [
        ("fd", "/proc/self/fd"),
        ("stdin", "/proc/self/fd/0"),
        ("stdout", "/proc/self/fd/1"),
        ("stderr", "/proc/self/fd/2"),
    ] {
        symlink(to, dev.join(link))?;
    }

    let tmp = root.join("tmp");
    fs::create_dir_all(&tmp)?;
    mount(
        Some("tmpfs"),
        &tmp,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NODEV,
        Some("size=64m,mode=1777"),
    )?;

    // The run's workspace (a size-capped tmpfs the host mounted), read-write.
    let ws = root.join("workspace");
    fs::create_dir_all(&ws)?;
    mount(Some(workspace), &ws, None, libc::MS_BIND, None)?;
    mount(
        None,
        &ws,
        None,
        libc::MS_BIND | libc::MS_REMOUNT | libc::MS_NOSUID | libc::MS_NODEV,
        None,
    )?;

    // Pivot and drop the host's root from this namespace entirely.
    let old = root.join(".oldroot");
    fs::create_dir_all(&old)?;
    let (root_c, old_c) = (c(root), c(&old));
    if unsafe { libc::syscall(libc::SYS_pivot_root, root_c.as_ptr(), old_c.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error()).context("pivot_root");
    }
    std::env::set_current_dir("/")?;
    let old_c = c("/.oldroot");
    if unsafe { libc::umount2(old_c.as_ptr(), libc::MNT_DETACH) } != 0 {
        return Err(io::Error::last_os_error()).context("umount old root");
    }
    fs::remove_dir("/.oldroot")?;
    // The new root itself is read-only from here on.
    mount(
        None,
        Path::new("/"),
        None,
        libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
        None,
    )
}

fn loopback_up() -> io::Result<()> {
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut ifr: libc::ifreq = std::mem::zeroed();
        for (i, b) in b"lo".iter().enumerate() {
            ifr.ifr_name[i] = *b as libc::c_char;
        }
        let mut r = libc::ioctl(fd, libc::SIOCGIFFLAGS, &mut ifr);
        if r == 0 {
            ifr.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
            r = libc::ioctl(fd, libc::SIOCSIFFLAGS, &ifr);
        }
        libc::close(fd);
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

// ---- privileges -------------------------------------------------------------------------

fn check(r: libc::c_int, what: &str) -> Result<()> {
    if r != 0 {
        return Err(io::Error::last_os_error()).context(what.to_owned());
    }
    Ok(())
}

fn drop_privileges() -> Result<()> {
    unsafe {
        // Empty the bounding set (needs CAP_SETPCAP, so before setuid); unknown caps
        // answer EINVAL, which ends the list.
        for cap in 0..64 {
            if libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) != 0 {
                break;
            }
        }
        check(
            libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_CLEAR_ALL,
                0,
                0,
                0,
            ),
            "clear ambient caps",
        )?;
        check(libc::setgroups(0, std::ptr::null()), "setgroups")?;
        check(libc::setresgid(NOBODY, NOBODY, NOBODY), "setresgid")?;
        // From root to a non-root uid without KEEPCAPS: permitted and effective caps clear.
        check(libc::setresuid(NOBODY, NOBODY, NOBODY), "setresuid")?;
        check(
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0),
            "no_new_privs",
        )?;
    }
    Ok(())
}

fn set_rlimits() {
    let set = |res, v: u64| unsafe {
        let l = libc::rlimit {
            rlim_cur: v,
            rlim_max: v,
        };
        libc::setrlimit(res, &l);
    };
    set(libc::RLIMIT_CORE, 0);
    set(libc::RLIMIT_NOFILE, 1024);
    set(libc::RLIMIT_FSIZE, 256 << 20);
}

// ---- seccomp ----------------------------------------------------------------------------

/// Syscalls the sandbox never needs, refused with EPERM: mounts and namespaces (no
/// escaping the layout), tracing other processes, kernel modules and kexec, BPF and perf,
/// keyrings, io_uring (a large, separately-audited attack surface), clock and host
/// settings, and handle-based opens.
const DENY: &[libc::c_long] = &[
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_chroot,
    libc::SYS_move_mount,
    libc::SYS_open_tree,
    libc::SYS_fsopen,
    libc::SYS_fsmount,
    libc::SYS_fspick,
    libc::SYS_mount_setattr,
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_kcmp,
    libc::SYS_kexec_load,
    libc::SYS_kexec_file_load,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_reboot,
    libc::SYS_bpf,
    libc::SYS_perf_event_open,
    libc::SYS_userfaultfd,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_open_by_handle_at,
    libc::SYS_name_to_handle_at,
    libc::SYS_swapon,
    libc::SYS_swapoff,
    libc::SYS_acct,
    libc::SYS_quotactl,
    libc::SYS_settimeofday,
    libc::SYS_clock_settime,
    libc::SYS_clock_adjtime,
    libc::SYS_adjtimex,
    libc::SYS_syslog,
    libc::SYS_iopl,
    libc::SYS_ioperm,
    libc::SYS_fanotify_init,
    libc::SYS_sethostname,
    libc::SYS_setdomainname,
    libc::SYS_vhangup,
];

/// clone() flags that would create namespaces.
const NS_FLAGS: &[u64] = &[
    libc::CLONE_NEWNS as u64,
    libc::CLONE_NEWUSER as u64,
    libc::CLONE_NEWPID as u64,
    libc::CLONE_NEWNET as u64,
    libc::CLONE_NEWIPC as u64,
    libc::CLONE_NEWUTS as u64,
    libc::CLONE_NEWCGROUP as u64,
];

fn install_seccomp() -> Result<()> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> =
        DENY.iter().map(|&nr| (nr, vec![])).collect();
    let clone_rules = NS_FLAGS
        .iter()
        .map(|&f| {
            SeccompRule::new(vec![SeccompCondition::new(
                0,
                SeccompCmpArgLen::Qword,
                SeccompCmpOp::MaskedEq(f),
                f,
            )?])
        })
        .collect::<Result<Vec<_>, _>>()?;
    rules.insert(libc::SYS_clone, clone_rules);
    let deny: BpfProgram = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        TargetArch::x86_64,
    )?
    .try_into()?;

    // clone3 passes its flags in memory, which seccomp cannot read: answer ENOSYS so libc
    // falls back to clone(), whose flags the filter above checks.
    let clone3: BpfProgram = SeccompFilter::new(
        BTreeMap::from([(libc::SYS_clone3, vec![])]),
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
        TargetArch::x86_64,
    )?
    .try_into()?;

    seccompiler::apply_filter(&deny)?;
    seccompiler::apply_filter(&clone3)?;
    Ok(())
}
