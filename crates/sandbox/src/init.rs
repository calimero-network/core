//! The sandbox's init: `<node binary> __calimero_sandbox_init <control> <image> <root>
//! <workspace> <cwd> -- argv...`.
//!
//! [`crate::Run::exec`] starts it already in the run's cgroup and in new mount, network, IPC,
//! UTS and cgroup namespaces, with a new PID namespace for its children. From there:
//!
//! 1. fork: the child is PID 1 of the new PID namespace;
//! 2. PID 1 builds a fresh root on tmpfs (the image's `/usr` read-only, a minimal `/etc` and
//!    `/dev`, `/proc` of the new namespace, `/tmp`, the run's `/workspace`), pivots into it and
//!    detaches the old root, so nothing of the node's filesystem stays reachable;
//! 3. PID 1 empties every capability set (bounding included), becomes `nobody`, sets
//!    `no_new_privs` and rlimits, and installs the seccomp filters;
//! 4. PID 1 forks the command and reaps until it exits; when PID 1 exits, the kernel kills
//!    whatever is left in the namespace.
//!
//! The outer process waits for PID 1 and exits with its status (128 + n for signal n).

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::Path;

use crate::{seccomp, NOBODY};

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
    let [control, image, root, workspace, cwd, sep, argv @ ..] = args else {
        return Err("usage: <control> <image> <root> <workspace> <cwd> -- argv...".into());
    };
    if sep != "--" || argv.is_empty() {
        return Err("usage: <control> <image> <root> <workspace> <cwd> -- argv...".into());
    }
    // Only a test build turns a layer off.
    let known: &[&str] = if cfg!(feature = "controls") {
        &["full", "no-seccomp", "root"]
    } else {
        &["full"]
    };
    if !known.contains(&control.as_str()) {
        return Err(format!("unknown control {control:?}"));
    }
    Ok(Args {
        control,
        image,
        root,
        workspace,
        cwd,
        argv,
    })
}

fn run(args: &[String]) -> InitResult<i32> {
    let args = parse(args)?;
    // SAFETY: a fresh exec, single-threaded, so fork is safe.
    match unsafe { libc::fork() } {
        -1 => Err(os_err("fork pid 1")),
        0 => {
            let code = pid1(&args).unwrap_or_else(|e| {
                eprintln!("sandbox-init: {e}");
                INIT_FAILED
            });
            // SAFETY: leaving the child without running the parent's atexit handlers.
            unsafe { libc::_exit(code) }
        }
        pid => wait_for(pid),
    }
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
    if args.control == "full" {
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
