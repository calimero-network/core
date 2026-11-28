//! The seccomp filters a sandboxed command runs under.
//!
//! A deny-list: syscalls a sandbox never needs answer `EPERM`, `clone` asking for a namespace
//! answers `EPERM`, and `clone3` answers `ENOSYS` so libc falls back to `clone`, whose flags
//! the filter can read.

use std::collections::BTreeMap;

use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

#[cfg(target_arch = "x86_64")]
const ARCH: TargetArch = TargetArch::x86_64;
#[cfg(target_arch = "aarch64")]
const ARCH: TargetArch = TargetArch::aarch64;

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
    #[cfg(target_arch = "x86_64")]
    libc::SYS_iopl,
    #[cfg(target_arch = "x86_64")]
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

/// Installs the filters on the calling thread; everything it starts inherits them.
pub(crate) fn install() -> Result<(), String> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = DENY.iter().map(|&nr| (nr, vec![])).collect();
    let clone_rules = NS_FLAGS
        .iter()
        .map(|&f| {
            SeccompCondition::new(0, SeccompCmpArgLen::Qword, SeccompCmpOp::MaskedEq(f), f)
                .and_then(|c| SeccompRule::new(vec![c]))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let _ = rules.insert(libc::SYS_clone, clone_rules);
    let deny: BpfProgram = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        ARCH,
    )
    .and_then(TryInto::try_into)
    .map_err(|e| e.to_string())?;

    // clone3 passes its flags in memory, which seccomp cannot read: answer ENOSYS so libc
    // falls back to clone(), whose flags the filter above checks.
    let clone3: BpfProgram = SeccompFilter::new(
        BTreeMap::from([(libc::SYS_clone3, vec![])]),
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
        ARCH,
    )
    .and_then(TryInto::try_into)
    .map_err(|e| e.to_string())?;

    seccompiler::apply_filter(&deny).map_err(|e| e.to_string())?;
    seccompiler::apply_filter(&clone3).map_err(|e| e.to_string())
}
