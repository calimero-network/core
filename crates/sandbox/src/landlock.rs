//! The Landlock ruleset a sandboxed command runs under: it may read and execute anything it can
//! see, and write, create or remove only beneath `/workspace`, `/tmp` and `/dev`. A second line
//! behind the read-only root: a command that got a writable mount of something else still
//! cannot write there.
//!
//! Raw syscalls (`landlock_create_ruleset`, `landlock_add_rule`, `landlock_restrict_self`), so
//! no dependency is needed; the rights handled are those the kernel's Landlock ABI knows.

use std::ffi::CString;
use std::io;

/// `LANDLOCK_CREATE_RULESET_VERSION`: ask the kernel for its ABI version.
const CREATE_RULESET_VERSION: libc::c_uint = 1;
/// `LANDLOCK_RULE_PATH_BENEATH`.
const RULE_PATH_BENEATH: libc::c_int = 1;

const EXECUTE: u64 = 1 << 0;
const WRITE_FILE: u64 = 1 << 1;
const READ_FILE: u64 = 1 << 2;
const READ_DIR: u64 = 1 << 3;
/// `REMOVE_DIR` through `MAKE_SYM`: every right ABI 1 adds beyond the four above.
const CHANGE_TREE: u64 = 0x1ff0;
/// ABI 2.
const REFER: u64 = 1 << 13;
/// ABI 3.
const TRUNCATE: u64 = 1 << 14;
/// ABI 5.
const IOCTL_DEV: u64 = 1 << 15;

/// Where a command may write: everything handled.
const WRITABLE: &[&str] = &["/workspace", "/tmp"];
/// Device files a command reads and writes (`/dev/null`, `/dev/zero`, ...).
const DEVICES: &str = "/dev";

/// `struct landlock_ruleset_attr`, as far as ABI 1 (the kernel takes a shorter struct).
#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
}

/// `struct landlock_path_beneath_attr`, which the kernel packs.
#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// The file-system rights the kernel's Landlock ABI `abi` knows.
fn handled(abi: i64) -> u64 {
    let mut rights = EXECUTE | WRITE_FILE | READ_FILE | READ_DIR | CHANGE_TREE;
    if abi >= 2 {
        rights |= REFER;
    }
    if abi >= 3 {
        rights |= TRUNCATE;
    }
    if abi >= 5 {
        rights |= IOCTL_DEV;
    }
    rights
}

fn os_err(what: &str) -> String {
    format!("{what}: {}", io::Error::last_os_error())
}

/// Restricts the calling thread and everything it starts. Needs `no_new_privs` set already.
/// A kernel without Landlock is an error: the sandbox does not run without its second line.
pub(crate) fn restrict() -> Result<(), String> {
    // SAFETY: a version query passes no struct.
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            CREATE_RULESET_VERSION,
        )
    };
    if abi < 1 {
        return Err(os_err("this kernel has no Landlock"));
    }
    let all = handled(abi);
    let attr = RulesetAttr {
        handled_access_fs: all,
    };
    // SAFETY: `attr` outlives the call and the size is its own.
    let ruleset = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &raw const attr,
            size_of::<RulesetAttr>(),
            0u32,
        )
    };
    if ruleset < 0 {
        return Err(os_err("landlock_create_ruleset"));
    }
    let ruleset = i32::try_from(ruleset).map_err(|_| "a bad ruleset descriptor".to_owned())?;
    let rules: Vec<(&str, u64)> = std::iter::once(("/", EXECUTE | READ_FILE | READ_DIR))
        .chain(WRITABLE.iter().map(|p| (*p, all)))
        .chain(std::iter::once((
            DEVICES,
            (READ_FILE | WRITE_FILE | READ_DIR | IOCTL_DEV) & all,
        )))
        .collect();
    let added = rules
        .into_iter()
        .try_for_each(|(path, access)| add_rule(ruleset, path, access));
    let restricted = added.and_then(|()| {
        // SAFETY: restrict_self takes a descriptor this process owns and no flags.
        if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0u32) } != 0 {
            return Err(os_err("landlock_restrict_self"));
        }
        Ok(())
    });
    // SAFETY: closing a descriptor this process owns.
    unsafe { libc::close(ruleset) };
    restricted
}

/// Allows `access` beneath `path`.
fn add_rule(ruleset: i32, path: &str, access: u64) -> Result<(), String> {
    let c_path = CString::new(path).map_err(|_| format!("{path} holds a NUL byte"))?;
    // SAFETY: a NUL-terminated path; O_PATH opens it for reference only.
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(os_err(path));
    }
    let attr = PathBeneathAttr {
        allowed_access: access,
        parent_fd: fd,
    };
    // SAFETY: `attr` outlives the call; the kernel reads the packed struct.
    let r = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset,
            RULE_PATH_BENEATH,
            &raw const attr,
            0u32,
        )
    };
    // SAFETY: closing a descriptor this function opened.
    unsafe { libc::close(fd) };
    if r != 0 {
        return Err(os_err(&format!("landlock rule for {path}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_abi_handles_the_rights_it_knows() {
        assert_eq!(handled(1), 0x1fff);
        assert_eq!(handled(2), 0x3fff);
        assert_eq!(handled(4), 0x7fff);
        assert_eq!(handled(7), 0xffff);
        assert_eq!(size_of::<PathBeneathAttr>(), 12);
    }
}
