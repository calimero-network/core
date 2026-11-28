//! The seccomp filter a sandboxed command runs under: an allow-list.
//!
//! A syscall not on [`ALLOW`] answers `ENOSYS`, so libc falls back to an older call where it has
//! one, and the program sees a call this kernel does not have. Left off, among others: mounts
//! and namespaces (`unshare`, `setns`, `pivot_root`, `chroot`), tracing and reading other
//! processes, kernel modules and kexec, BPF and perf, keyrings, io_uring, clock and host
//! settings, handle-based opens and `userfaultfd`. `clone` is allowed only without a namespace
//! flag (else `EPERM`), and `clone3`, whose flags sit in memory seccomp cannot read, is left
//! off so libc falls back to `clone`.

use std::collections::BTreeMap;

use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

#[cfg(target_arch = "x86_64")]
const ARCH: TargetArch = TargetArch::x86_64;
#[cfg(target_arch = "aarch64")]
const ARCH: TargetArch = TargetArch::aarch64;

/// What a shell, Python, a C toolchain, git and coreutils call: the default allow-list of OCI
/// runtimes, less everything that changes the system outside the process.
const ALLOW: &[libc::c_long] = &[
    // Files and directories.
    libc::SYS_read,
    libc::SYS_write,
    libc::SYS_readv,
    libc::SYS_writev,
    libc::SYS_pread64,
    libc::SYS_pwrite64,
    libc::SYS_preadv,
    libc::SYS_pwritev,
    libc::SYS_preadv2,
    libc::SYS_pwritev2,
    libc::SYS_openat,
    libc::SYS_openat2,
    libc::SYS_close,
    libc::SYS_close_range,
    libc::SYS_lseek,
    libc::SYS_fstat,
    libc::SYS_newfstatat,
    libc::SYS_statx,
    libc::SYS_statfs,
    libc::SYS_fstatfs,
    libc::SYS_faccessat,
    libc::SYS_faccessat2,
    libc::SYS_getdents64,
    libc::SYS_getcwd,
    libc::SYS_chdir,
    libc::SYS_fchdir,
    libc::SYS_mkdirat,
    libc::SYS_mknodat,
    libc::SYS_unlinkat,
    libc::SYS_renameat,
    libc::SYS_renameat2,
    libc::SYS_linkat,
    libc::SYS_symlinkat,
    libc::SYS_readlinkat,
    libc::SYS_fchmod,
    libc::SYS_fchmodat,
    libc::SYS_fchown,
    libc::SYS_fchownat,
    libc::SYS_truncate,
    libc::SYS_ftruncate,
    libc::SYS_fallocate,
    libc::SYS_fadvise64,
    libc::SYS_readahead,
    libc::SYS_fsync,
    libc::SYS_fdatasync,
    libc::SYS_sync,
    libc::SYS_syncfs,
    libc::SYS_sync_file_range,
    libc::SYS_utimensat,
    libc::SYS_umask,
    libc::SYS_flock,
    libc::SYS_fcntl,
    libc::SYS_ioctl,
    libc::SYS_dup,
    libc::SYS_dup3,
    libc::SYS_pipe2,
    libc::SYS_sendfile,
    libc::SYS_splice,
    libc::SYS_tee,
    libc::SYS_vmsplice,
    libc::SYS_copy_file_range,
    libc::SYS_getxattr,
    libc::SYS_lgetxattr,
    libc::SYS_fgetxattr,
    libc::SYS_listxattr,
    libc::SYS_llistxattr,
    libc::SYS_flistxattr,
    libc::SYS_setxattr,
    libc::SYS_lsetxattr,
    libc::SYS_fsetxattr,
    libc::SYS_removexattr,
    libc::SYS_lremovexattr,
    libc::SYS_fremovexattr,
    libc::SYS_inotify_init1,
    libc::SYS_inotify_add_watch,
    libc::SYS_inotify_rm_watch,
    libc::SYS_memfd_create,
    // Memory.
    libc::SYS_brk,
    libc::SYS_mmap,
    libc::SYS_munmap,
    libc::SYS_mremap,
    libc::SYS_mprotect,
    libc::SYS_madvise,
    libc::SYS_mincore,
    libc::SYS_msync,
    libc::SYS_mlock,
    libc::SYS_mlock2,
    libc::SYS_munlock,
    libc::SYS_mlockall,
    libc::SYS_munlockall,
    libc::SYS_membarrier,
    libc::SYS_pkey_alloc,
    libc::SYS_pkey_free,
    libc::SYS_pkey_mprotect,
    // Processes and threads.
    libc::SYS_execve,
    libc::SYS_execveat,
    libc::SYS_exit,
    libc::SYS_exit_group,
    libc::SYS_wait4,
    libc::SYS_waitid,
    libc::SYS_kill,
    libc::SYS_tkill,
    libc::SYS_tgkill,
    libc::SYS_getpid,
    libc::SYS_getppid,
    libc::SYS_gettid,
    libc::SYS_getpgid,
    libc::SYS_setpgid,
    libc::SYS_getsid,
    libc::SYS_setsid,
    libc::SYS_getuid,
    libc::SYS_geteuid,
    libc::SYS_getgid,
    libc::SYS_getegid,
    libc::SYS_getresuid,
    libc::SYS_getresgid,
    libc::SYS_getgroups,
    libc::SYS_setuid,
    libc::SYS_setgid,
    libc::SYS_setreuid,
    libc::SYS_setregid,
    libc::SYS_setresuid,
    libc::SYS_setresgid,
    libc::SYS_setfsuid,
    libc::SYS_setfsgid,
    libc::SYS_setgroups,
    libc::SYS_capget,
    libc::SYS_capset,
    libc::SYS_prctl,
    libc::SYS_getrlimit,
    libc::SYS_setrlimit,
    libc::SYS_prlimit64,
    libc::SYS_getrusage,
    libc::SYS_getpriority,
    libc::SYS_setpriority,
    libc::SYS_sched_yield,
    libc::SYS_sched_getaffinity,
    libc::SYS_sched_setaffinity,
    libc::SYS_sched_getparam,
    libc::SYS_sched_setparam,
    libc::SYS_sched_getscheduler,
    libc::SYS_sched_setscheduler,
    libc::SYS_sched_get_priority_max,
    libc::SYS_sched_get_priority_min,
    libc::SYS_sched_rr_get_interval,
    libc::SYS_sched_getattr,
    libc::SYS_sched_setattr,
    libc::SYS_getcpu,
    libc::SYS_set_tid_address,
    libc::SYS_set_robust_list,
    libc::SYS_get_robust_list,
    libc::SYS_rseq,
    libc::SYS_futex,
    libc::SYS_pidfd_open,
    libc::SYS_pidfd_send_signal,
    libc::SYS_ioprio_get,
    libc::SYS_ioprio_set,
    libc::SYS_seccomp,
    libc::SYS_landlock_create_ruleset,
    libc::SYS_landlock_add_rule,
    libc::SYS_landlock_restrict_self,
    // Signals.
    libc::SYS_rt_sigaction,
    libc::SYS_rt_sigprocmask,
    libc::SYS_rt_sigreturn,
    libc::SYS_rt_sigpending,
    libc::SYS_rt_sigsuspend,
    libc::SYS_rt_sigtimedwait,
    libc::SYS_rt_sigqueueinfo,
    libc::SYS_rt_tgsigqueueinfo,
    libc::SYS_sigaltstack,
    libc::SYS_signalfd4,
    libc::SYS_restart_syscall,
    // Time.
    libc::SYS_clock_gettime,
    libc::SYS_clock_getres,
    libc::SYS_clock_nanosleep,
    libc::SYS_gettimeofday,
    libc::SYS_nanosleep,
    libc::SYS_getitimer,
    libc::SYS_setitimer,
    libc::SYS_timer_create,
    libc::SYS_timer_delete,
    libc::SYS_timer_gettime,
    libc::SYS_timer_settime,
    libc::SYS_timer_getoverrun,
    libc::SYS_timerfd_create,
    libc::SYS_timerfd_gettime,
    libc::SYS_timerfd_settime,
    libc::SYS_times,
    // Waiting on descriptors.
    libc::SYS_ppoll,
    libc::SYS_pselect6,
    libc::SYS_epoll_create1,
    libc::SYS_epoll_ctl,
    libc::SYS_epoll_pwait,
    libc::SYS_epoll_pwait2,
    libc::SYS_eventfd2,
    // Sockets: the network namespace holds only `lo`.
    libc::SYS_socket,
    libc::SYS_socketpair,
    libc::SYS_bind,
    libc::SYS_listen,
    libc::SYS_accept4,
    libc::SYS_connect,
    libc::SYS_getsockname,
    libc::SYS_getpeername,
    libc::SYS_getsockopt,
    libc::SYS_setsockopt,
    libc::SYS_sendto,
    libc::SYS_recvfrom,
    libc::SYS_sendmsg,
    libc::SYS_recvmsg,
    libc::SYS_sendmmsg,
    libc::SYS_recvmmsg,
    libc::SYS_shutdown,
    // System V IPC, within the run's IPC namespace.
    libc::SYS_shmget,
    libc::SYS_shmat,
    libc::SYS_shmdt,
    libc::SYS_shmctl,
    libc::SYS_semget,
    libc::SYS_semop,
    libc::SYS_semtimedop,
    libc::SYS_semctl,
    libc::SYS_msgget,
    libc::SYS_msgsnd,
    libc::SYS_msgrcv,
    libc::SYS_msgctl,
    // Information.
    libc::SYS_uname,
    libc::SYS_sysinfo,
    libc::SYS_getrandom,
];

/// The x86_64 calls aarch64 never had: libc still makes them there.
#[cfg(target_arch = "x86_64")]
const ALLOW_X86_64: &[libc::c_long] = &[
    libc::SYS_open,
    libc::SYS_creat,
    libc::SYS_stat,
    libc::SYS_lstat,
    libc::SYS_access,
    libc::SYS_getdents,
    libc::SYS_mkdir,
    libc::SYS_rmdir,
    libc::SYS_mknod,
    libc::SYS_unlink,
    libc::SYS_rename,
    libc::SYS_link,
    libc::SYS_symlink,
    libc::SYS_readlink,
    libc::SYS_chmod,
    libc::SYS_chown,
    libc::SYS_lchown,
    libc::SYS_utime,
    libc::SYS_utimes,
    libc::SYS_futimesat,
    libc::SYS_dup2,
    libc::SYS_pipe,
    libc::SYS_fork,
    libc::SYS_vfork,
    libc::SYS_getpgrp,
    libc::SYS_pause,
    libc::SYS_alarm,
    libc::SYS_time,
    libc::SYS_poll,
    libc::SYS_select,
    libc::SYS_epoll_create,
    libc::SYS_epoll_wait,
    libc::SYS_eventfd,
    libc::SYS_signalfd,
    libc::SYS_inotify_init,
    libc::SYS_accept,
    libc::SYS_arch_prctl,
    libc::SYS_set_thread_area,
    libc::SYS_get_thread_area,
];

#[cfg(not(target_arch = "x86_64"))]
const ALLOW_X86_64: &[libc::c_long] = &[];

/// `clone()` flags that would create namespaces.
const NS_FLAGS: &[u64] = &[
    libc::CLONE_NEWNS as u64,
    libc::CLONE_NEWUSER as u64,
    libc::CLONE_NEWPID as u64,
    libc::CLONE_NEWNET as u64,
    libc::CLONE_NEWIPC as u64,
    libc::CLONE_NEWUTS as u64,
    libc::CLONE_NEWCGROUP as u64,
];

/// The calls [`install`] allows, every one of them unconditionally: for tests.
pub(crate) fn allowed() -> impl Iterator<Item = libc::c_long> {
    ALLOW.iter().chain(ALLOW_X86_64).copied()
}

/// Installs the filter on the calling thread; everything it starts inherits it.
pub(crate) fn install() -> Result<(), String> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = allowed().map(|nr| (nr, vec![])).collect();
    // `clone` without any namespace flag: one rule whose conditions all hold.
    let no_namespace = NS_FLAGS
        .iter()
        .map(|&f| SeccompCondition::new(0, SeccompCmpArgLen::Qword, SeccompCmpOp::MaskedEq(f), 0))
        .collect::<Result<Vec<_>, _>>()
        .and_then(SeccompRule::new)
        .map_err(|e| e.to_string())?;
    let _ = rules.insert(libc::SYS_clone, vec![no_namespace]);
    let allow: BpfProgram = SeccompFilter::new(
        rules,
        SeccompAction::Errno(libc::ENOSYS as u32),
        SeccompAction::Allow,
        ARCH,
    )
    .and_then(TryInto::try_into)
    .map_err(|e| e.to_string())?;
    seccompiler::apply_filter(&allow).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_that_reaches_outside_the_process_is_allowed() {
        let allowed: Vec<_> = allowed().collect();
        for nr in [
            libc::SYS_mount,
            libc::SYS_umount2,
            libc::SYS_pivot_root,
            libc::SYS_chroot,
            libc::SYS_unshare,
            libc::SYS_setns,
            libc::SYS_clone3,
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
            libc::SYS_kcmp,
            libc::SYS_pidfd_getfd,
            libc::SYS_kexec_load,
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
            libc::SYS_acct,
            libc::SYS_settimeofday,
            libc::SYS_clock_settime,
            libc::SYS_adjtimex,
            libc::SYS_syslog,
            libc::SYS_sethostname,
            libc::SYS_personality,
            libc::SYS_clone,
        ] {
            assert!(!allowed.contains(&nr), "syscall {nr} is allowed");
        }
        let mut sorted = allowed.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), allowed.len(), "a syscall is listed twice");
    }
}
