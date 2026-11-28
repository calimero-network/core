//! The sandbox's init: `<node binary> __calimero_sandbox_init <control> <uid base> <image>
//! <root> <workspace> <cwd> -- argv...`.
//!
//! [`crate::Run::exec`] starts it in the run's cgroup, as the node's own user. From there:
//!
//! 1. the outer init forks a child that unshares a user namespace with new mount, PID, network,
//!    IPC, UTS and cgroup namespaces owned by it; the outer init maps the child's uids and gids
//!    0..65536 to the node's `<uid base>..<uid base> + 65536`, none of them the node's root, so
//!    nothing in the sandbox is ever root on the node;
//! 2. the child becomes root of its user namespace only and forks: that child is PID 1 of the
//!    new PID namespace;
//! 3. PID 1 builds a fresh root on tmpfs (the image's `/usr` read-only, a minimal `/etc` and
//!    `/dev`, `/proc` of the new namespace, `/tmp`, the run's `/workspace`), pivots into it and
//!    detaches the old root, so nothing of the node's filesystem stays reachable;
//! 4. PID 1 empties every capability set (bounding included), becomes `nobody`, sets
//!    `no_new_privs` and rlimits, and installs the seccomp filters;
//! 5. PID 1 forks the command and reaps until it exits; when PID 1 exits, the kernel kills
//!    whatever is left in the namespace.
//!
//! Each process waits for the one it forked and exits with its status (128 + n for signal n).

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::Path;

use crate::{landlock, seccomp, NOBODY};

/// The status init exits with when it could not start the command.
pub(crate) const INIT_FAILED: i32 = 125;

pub(crate) fn main(args: &[String]) -> ! {
    let code = run(args).unwrap_or_else(|e| {
        eprintln!("sandbox-init: {e}");
        INIT_FAILED
    });
    std::process::exit(code)
}

struct Args<'a> {
    control: &'a str,
    uid_base: u32,
    image: &'a str,
    root: &'a str,
    workspace: &'a str,
    cwd: &'a str,
    argv: &'a [String],
}

type InitResult<T> = Result<T, String>;

fn os_err(what: impl std::fmt::Display) -> String {
    format!("{what}: {}", io::Error::last_os_error())
}

fn parse(args: &[String]) -> InitResult<Args<'_>> {
    const USAGE: &str = "usage: <control> <uid base> <image> <root> <workspace> <cwd> -- argv...";
    let [control, uid_base, image, root, workspace, cwd, sep, argv @ ..] = args else {
        return Err(USAGE.into());
    };
    if sep != "--" || argv.is_empty() {
        return Err(USAGE.into());
    }
    let uid_base = uid_base
        .parse()
        .ok()
        .filter(|b: &u32| *b > 0 && b.checked_add(ID_RANGE).is_some())
        .ok_or_else(|| format!("bad uid base {uid_base:?}"))?;
    // Only a test build turns a layer off.
    let known: &[&str] = if cfg!(feature = "controls") {
        &["full", "no-seccomp", "root", "no-userns", "no-landlock"]
    } else {
        &["full"]
    };
    if !known.contains(&control.as_str()) {
        return Err(format!("unknown control {control:?}"));
    }
    Ok(Args {
        control,
        uid_base,
        image,
        root,
        workspace,
        cwd,
        argv,
    })
}

/// How many uids and gids the sandbox's user namespace maps.
pub(crate) const ID_RANGE: u32 = 65_536;

/// The namespaces the sandbox's child unshares beside its user namespace.
const NAMESPACES: libc::c_int = libc::CLONE_NEWNS
    | libc::CLONE_NEWPID
    | libc::CLONE_NEWNET
    | libc::CLONE_NEWIPC
    | libc::CLONE_NEWUTS
    | libc::CLONE_NEWCGROUP;

/// Runs `f` in a forked child that exits with its answer, and returns the child's pid.
fn fork_with(f: impl FnOnce() -> InitResult<i32>) -> InitResult<libc::pid_t> {
    // SAFETY: init is single-threaded (a fresh exec), so fork is safe.
    match unsafe { libc::fork() } {
        -1 => Err(os_err("fork")),
        0 => {
            let code = f().unwrap_or_else(|e| {
                eprintln!("sandbox-init: {e}");
                INIT_FAILED
            });
            // SAFETY: leaving the child without running the parent's atexit handlers.
            unsafe { libc::_exit(code) }
        }
        pid => Ok(pid),
    }
}

/// A pipe whose ends close on exec.
fn pipe() -> InitResult<(libc::c_int, libc::c_int)> {
    let mut fds = [0; 2];
    // SAFETY: pipe2 writes two descriptors into `fds`.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(os_err("pipe"));
    }
    Ok((fds[0], fds[1]))
}

/// Blocks until one byte arrives on `fd` (the other process says go), then closes it.
fn wait_byte(fd: libc::c_int) -> InitResult<()> {
    let mut b = 0u8;
    loop {
        // SAFETY: reads at most one byte into `b`.
        let n = unsafe { libc::read(fd, (&raw mut b).cast(), 1) };
        if n == 1 {
            // SAFETY: closing a descriptor this process owns.
            unsafe { libc::close(fd) };
            return Ok(());
        }
        if n == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return Err("the other side of the sandbox's set-up went away".into());
        }
    }
}

/// Sends one byte on `fd` and closes it.
fn send_byte(fd: libc::c_int) -> InitResult<()> {
    // SAFETY: writes one byte from a constant.
    let n = unsafe { libc::write(fd, b"1".as_ptr().cast(), 1) };
    // SAFETY: closing a descriptor this process owns.
    unsafe { libc::close(fd) };
    if n != 1 {
        return Err(os_err("signal the sandbox's set-up"));
    }
    Ok(())
}

fn run(args: &[String]) -> InitResult<i32> {
    let args = parse(args)?;
    let userns = args.control != "no-userns";
    let (unshared_r, unshared_w) = pipe()?;
    let (mapped_r, mapped_w) = pipe()?;
    let child = fork_with(|| {
        // SAFETY: closing the ends this process does not use.
        unsafe {
            libc::close(unshared_r);
            libc::close(mapped_w);
        }
        let flags = if userns {
            NAMESPACES | libc::CLONE_NEWUSER
        } else {
            NAMESPACES
        };
        // SAFETY: unshare takes only flags.
        if unsafe { libc::unshare(flags) } != 0 {
            return Err(os_err("unshare"));
        }
        send_byte(unshared_w)?;
        wait_byte(mapped_r)?;
        if userns {
            become_namespace_root()?;
        }
        // The new PID namespace takes this process's children: the next one is its PID 1.
        let pid1 = fork_with(|| pid1(&args))?;
        wait_for(pid1)
    })?;
    // SAFETY: closing the ends this process does not use.
    unsafe {
        libc::close(unshared_w);
        libc::close(mapped_r);
    }
    wait_byte(unshared_r)?;
    if userns {
        map_ids(child, args.uid_base)?;
    }
    send_byte(mapped_w)?;
    wait_for(child)
}

/// Maps `pid`'s user namespace: its uids and gids `0..ID_RANGE` are the node's
/// `base..base + ID_RANGE`.
fn map_ids(pid: libc::pid_t, base: u32) -> InitResult<()> {
    let map = format!("0 {base} {ID_RANGE}\n");
    for file in ["uid_map", "gid_map"] {
        let path = format!("/proc/{pid}/{file}");
        fs::write(&path, &map).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}

/// Becomes uid and gid 0 of the process's own user namespace, which the node sees as the uid
/// base: the capabilities it holds are the namespace's, never the node's.
fn become_namespace_root() -> InitResult<()> {
    // SAFETY: set*id calls with constant arguments.
    unsafe {
        check(libc::setresgid(0, 0, 0), "setresgid 0")?;
        check(libc::setresuid(0, 0, 0), "setresuid 0")?;
    }
    Ok(())
}

fn wait_for(pid: libc::pid_t) -> InitResult<i32> {
    let mut status = 0;
    loop {
        // SAFETY: waitpid writes only `status`.
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid {
            return Ok(decode(status));
        }
        if r == -1 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return Err(os_err("waitpid"));
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

fn pid1(args: &Args<'_>) -> InitResult<i32> {
    setup_fs(Path::new(args.image), Path::new(args.root), args.workspace)?;
    let host = b"sandbox";
    // SAFETY: the pointer and length describe `host`.
    unsafe { libc::sethostname(host.as_ptr().cast(), host.len()) };
    // Only `lo` exists in the new network namespace; best effort.
    let _ = loopback_up();
    let cwd = Path::new("/workspace").join(args.cwd.trim_start_matches('/'));
    std::env::set_current_dir(&cwd).map_err(|e| format!("cwd {}: {e}", cwd.display()))?;
    if args.control != "root" {
        drop_privileges()?;
    }
    set_rlimits();
    if !matches!(args.control, "no-landlock" | "root") {
        landlock::restrict().map_err(|e| format!("landlock: {e}"))?;
    }
    if !matches!(args.control, "no-seccomp" | "root") {
        seccomp::install().map_err(|e| format!("seccomp: {e}"))?;
    }

    let argv: Vec<CString> = args
        .argv
        .iter()
        .map(|a| CString::new(a.as_bytes()))
        .collect::<Result<_, _>>()
        .map_err(|_| "argv holds a NUL byte".to_owned())?;
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());

    // SAFETY: still single-threaded.
    let child = unsafe { libc::fork() };
    match child {
        -1 => Err(os_err("fork command")),
        0 => {
            // SAFETY: `ptrs` is a NULL-terminated array of C strings that outlive the call.
            unsafe {
                libc::execvp(ptrs[0], ptrs.as_ptr());
                let msg = b"sandbox-init: exec failed\n";
                libc::write(2, msg.as_ptr().cast(), msg.len());
                libc::_exit(127)
            }
        }
        child => loop {
            // PID 1 reaps orphans; the run's status is the command's.
            let mut status = 0;
            // SAFETY: waitpid writes only `status`.
            let r = unsafe { libc::waitpid(-1, &mut status, 0) };
            if r == child {
                return Ok(decode(status));
            }
            if r == -1 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                return Err(os_err("reap"));
            }
        },
    }
}

// ---- filesystem -------------------------------------------------------------------------

fn c(p: impl AsRef<Path>) -> InitResult<CString> {
    CString::new(p.as_ref().as_os_str().as_encoded_bytes())
        .map_err(|_| format!("{} holds a NUL byte", p.as_ref().display()))
}

fn mount(
    src: Option<&Path>,
    target: &Path,
    fstype: Option<&str>,
    flags: libc::c_ulong,
    data: Option<&str>,
) -> InitResult<()> {
    let src = src.map(c).transpose()?;
    let fstype = fstype.map(c).transpose()?;
    let data = data.map(c).transpose()?;
    let target_c = c(target)?;
    // SAFETY: every pointer is a NUL-terminated string that outlives the call, or null.
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
        return Err(os_err(format!("mount {}", target.display())));
    }
    Ok(())
}

fn io_err(what: &Path) -> impl FnOnce(io::Error) -> String + '_ {
    move |e| format!("{}: {e}", what.display())
}

/// Binds `src` at `dst` read-only, nosuid, nodev.
fn bind_ro(src: &Path, dst: &Path) -> InitResult<()> {
    if src.is_dir() {
        fs::create_dir_all(dst).map_err(io_err(dst))?;
    } else {
        fs::write(dst, b"").map_err(io_err(dst))?;
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

fn setup_fs(image: &Path, root: &Path, workspace: &str) -> InitResult<()> {
    // Nothing mounted from here on propagates back to the node.
    mount(
        None,
        Path::new("/"),
        None,
        libc::MS_REC | libc::MS_PRIVATE,
        None,
    )?;
    mount(
        Some(Path::new("tmpfs")),
        root,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NODEV,
        Some("size=4m,mode=0755"),
    )?;

    bind_ro(&image.join("usr"), &root.join("usr"))?;
    for (link, to) in [
        ("bin", "usr/bin"),
        ("lib", "usr/lib"),
        ("lib64", "usr/lib64"),
        ("sbin", "usr/sbin"),
    ] {
        symlink(to, root.join(link)).map_err(io_err(&root.join(link)))?;
    }

    // A minimal /etc: no node users, hosts, secrets or proxy settings.
    let etc = root.join("etc");
    fs::create_dir_all(&etc).map_err(io_err(&etc))?;
    for (name, contents) in [
        (
            "passwd",
            "root:x:0:0:root:/:/usr/sbin/nologin\nnobody:x:65534:65534:nobody:/workspace:/bin/sh\n",
        ),
        ("group", "root:x:0:\nnogroup:x:65534:\n"),
        ("hosts", "127.0.0.1 localhost sandbox\n"),
        ("resolv.conf", ""),
    ] {
        fs::write(etc.join(name), contents).map_err(io_err(&etc.join(name)))?;
    }
    for f in ["etc/ld.so.cache", "etc/alternatives", "etc/ssl/certs"] {
        let src = image.join(f);
        if src.exists() {
            bind_ro(&src, &root.join(f))?;
        }
    }

    // /proc of the new PID namespace: only the sandbox's own processes.
    let proc_ = root.join("proc");
    fs::create_dir_all(&proc_).map_err(io_err(&proc_))?;
    mount(
        Some(Path::new("proc")),
        &proc_,
        Some("proc"),
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        None,
    )?;
    mask_proc(&proc_)?;

    // A minimal /dev.
    let dev = root.join("dev");
    fs::create_dir_all(&dev).map_err(io_err(&dev))?;
    mount(
        Some(Path::new("tmpfs")),
        &dev,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NOEXEC,
        Some("size=64k,mode=0755"),
    )?;
    for n in ["null", "zero", "full", "random", "urandom"] {
        let dst = dev.join(n);
        fs::write(&dst, b"").map_err(io_err(&dst))?;
        mount(
            Some(&Path::new("/dev").join(n)),
            &dst,
            None,
            libc::MS_BIND,
            None,
        )?;
    }
    for (link, to) in [
        ("fd", "/proc/self/fd"),
        ("stdin", "/proc/self/fd/0"),
        ("stdout", "/proc/self/fd/1"),
        ("stderr", "/proc/self/fd/2"),
    ] {
        symlink(to, dev.join(link)).map_err(io_err(&dev.join(link)))?;
    }

    let tmp = root.join("tmp");
    fs::create_dir_all(&tmp).map_err(io_err(&tmp))?;
    mount(
        Some(Path::new("tmpfs")),
        &tmp,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NODEV,
        Some("size=64m,mode=1777"),
    )?;

    // The run's workspace (a size-capped tmpfs the node mounted), read-write.
    let ws = root.join("workspace");
    fs::create_dir_all(&ws).map_err(io_err(&ws))?;
    mount(Some(Path::new(workspace)), &ws, None, libc::MS_BIND, None)?;
    mount(
        None,
        &ws,
        None,
        libc::MS_BIND | libc::MS_REMOUNT | libc::MS_NOSUID | libc::MS_NODEV,
        None,
    )?;

    // Pivot, and drop the node's root from this namespace entirely.
    let old = root.join(".oldroot");
    fs::create_dir_all(&old).map_err(io_err(&old))?;
    let (root_c, old_c) = (c(root)?, c(&old)?);
    // SAFETY: both pointers are NUL-terminated strings that outlive the call.
    if unsafe { libc::syscall(libc::SYS_pivot_root, root_c.as_ptr(), old_c.as_ptr()) } != 0 {
        return Err(os_err("pivot_root"));
    }
    std::env::set_current_dir("/").map_err(|e| format!("chdir /: {e}"))?;
    let old_c = c("/.oldroot")?;
    // SAFETY: a NUL-terminated string that outlives the call.
    if unsafe { libc::umount2(old_c.as_ptr(), libc::MNT_DETACH) } != 0 {
        return Err(os_err("umount old root"));
    }
    fs::remove_dir("/.oldroot").map_err(|e| format!("rmdir /.oldroot: {e}"))?;
    // The new root itself is read-only from here on.
    mount(
        None,
        Path::new("/"),
        None,
        libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
        None,
    )
}

/// `/proc` entries a sandbox reads nothing of, as OCI runtimes mask them: the kernel's memory
/// image, keys, timers and scheduler state, and hardware listings.
const MASKED_PROC: &[&str] = &[
    "acpi",
    "asound",
    "interrupts",
    "kcore",
    "keys",
    "latency_stats",
    "sched_debug",
    "scsi",
    "timer_list",
    "timer_stats",
];

/// `/proc` entries a sandbox may read but never write: the kernel's switches.
const READ_ONLY_PROC: &[&str] = &["bus", "fs", "irq", "sys", "sysrq-trigger"];

/// Masks `proc`'s entries in [`MASKED_PROC`] (a file under the node's `/dev/null`, a directory
/// under an empty read-only tmpfs) and makes those in [`READ_ONLY_PROC`] read-only.
fn mask_proc(proc_: &Path) -> InitResult<()> {
    for name in MASKED_PROC {
        let path = proc_.join(name);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            mount(
                Some(Path::new("tmpfs")),
                &path,
                Some("tmpfs"),
                libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                Some("size=0,mode=0555"),
            )?;
        } else {
            mount(
                Some(Path::new("/dev/null")),
                &path,
                None,
                libc::MS_BIND,
                None,
            )?;
        }
    }
    for name in READ_ONLY_PROC {
        let path = proc_.join(name);
        if fs::symlink_metadata(&path).is_err() {
            continue;
        }
        mount(Some(&path), &path, None, libc::MS_BIND | libc::MS_REC, None)?;
        mount(
            None,
            &path,
            None,
            libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
            None,
        )?;
    }
    Ok(())
}

fn loopback_up() -> io::Result<()> {
    // SAFETY: a socket and two ioctls on a zeroed `ifreq` naming `lo`; the fd is closed.
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

fn check(r: libc::c_int, what: &str) -> InitResult<()> {
    if r != 0 {
        return Err(os_err(what));
    }
    Ok(())
}

/// Order matters: empty the bounding set while still holding `CAP_SETPCAP`, then change uid,
/// which clears the permitted and effective sets.
fn drop_privileges() -> InitResult<()> {
    // SAFETY: prctl and set*id calls with constant arguments.
    unsafe {
        // Unknown capabilities answer EINVAL, which ends the list.
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
        check(libc::setresuid(NOBODY, NOBODY, NOBODY), "setresuid")?;
        check(
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0),
            "no_new_privs",
        )?;
    }
    Ok(())
}

fn set_rlimits() {
    let set = |res, v: u64| {
        let l = libc::rlimit {
            rlim_cur: v,
            rlim_max: v,
        };
        // SAFETY: setrlimit reads only `l`.
        unsafe { libc::setrlimit(res, &l) };
    };
    set(libc::RLIMIT_CORE, 0);
    set(libc::RLIMIT_NOFILE, 1024);
    set(libc::RLIMIT_FSIZE, 256 << 20);
}
