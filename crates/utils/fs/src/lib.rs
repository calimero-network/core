//! Writing a secret to disk so only its owner can read it, on every platform
//! this node runs on.
//!
//! Unix answers this with a mode. Windows has no mode: a new file inherits the
//! containing directory's ACL, so a key written into a shared, roaming or
//! loosely-permissioned directory is readable by other local users, and nothing
//! says so. Every site that writes a secret used to spell the unix half itself
//! and leave the other platform a silent no-op — three spellings of the same
//! intent that had already drifted apart. This is the one place that intent
//! lives.

use std::fs::File;
use std::io;
use std::path::Path;

/// Create `path` for writing, readable and writable by its owner alone.
///
/// Created **exclusively**: an existing file is an error rather than something
/// to truncate and inherit the permissions of. That is not caution for its own
/// sake — `OpenOptions::create(true)` leaves an attacker-placed file's ACL in
/// place, and on unix a `set_permissions` afterwards only narrows the window in
/// which the secret is world-readable rather than removing it. Exclusive
/// creation removes it, and refuses to follow a symlink or junction planted at
/// the path.
///
/// # Errors
///
/// When the file exists, when creation fails, or — on Windows — when the ACL
/// cannot be applied. An ACL failure deletes the file before returning: a
/// secret that exists but is not protected is worse than no secret at all,
/// because the caller has no reason to look at it again.
pub fn create_owner_only(path: &Path) -> io::Result<File> {
    let file = create_exclusive(path)?;

    #[cfg(windows)]
    if let Err(err) = windows_impl::restrict_to_current_user(path) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(err);
    }

    Ok(file)
}

/// [`create_owner_only`], writing `contents` and flushing them to the device.
///
/// # Errors
///
/// As [`create_owner_only`], plus any write or sync failure.
pub fn write_owner_only(path: &Path, contents: &[u8]) -> io::Result<()> {
    use io::Write as _;

    let mut file = create_owner_only(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Restrict a file or directory that already exists to its owner.
///
/// For the paths a secret arrives at by another route: a directory tree, or a
/// datastore whose files were created by a library with its own ideas about
/// permissions. Prefer [`create_owner_only`] wherever the file is ours to
/// create — this necessarily leaves a window in which the path was readable.
///
/// # Errors
///
/// When the permissions or ACL cannot be applied.
pub fn restrict_existing_to_owner(path: &Path) -> io::Result<()> {
    // Exactly one of these survives `cfg`, so each is the function's tail
    // expression rather than an early return.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        // A directory needs the execute bit to be traversable by its owner.
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(windows)]
    {
        windows_impl::restrict_to_current_user(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(())
    }
}

fn create_exclusive(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    let _ = options.write(true).create_new(true);

    // Set at creation on unix, so the file is never briefly group- or
    // world-readable. Windows cannot express this in the open call and is
    // handled by the caller applying a DACL immediately afterwards.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let _ = options.mode(0o600);
    }

    options.open(path)
}

#[cfg(windows)]
mod windows_impl {
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE,
        SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE,
        PROTECTED_DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// `GENERIC_ALL` — the owner keeps full control; nobody else appears in the
    /// ACL at all.
    const OWNER_ACCESS: u32 = 0x1000_0000;

    /// Replace `path`'s DACL with one naming only the calling user.
    ///
    /// `PROTECTED_DACL_SECURITY_INFORMATION` is the point of the whole exercise:
    /// without it the parent's inheritable entries are merged back in and the
    /// file stays as readable as the directory it sits in, which is exactly the
    /// behaviour being fixed.
    pub(super) fn restrict_to_current_user(path: &Path) -> io::Result<()> {
        let sid_buffer = current_user_sid()?;
        // SAFETY: `sid_buffer` is a TOKEN_USER written by GetTokenInformation,
        // so its `User.Sid` points inside the same allocation and outlives the
        // borrow below.
        let sid = unsafe { (*sid_buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };

        let mut access = EXPLICIT_ACCESS_W {
            grfAccessPermissions: OWNER_ACCESS,
            grfAccessMode: SET_ACCESS,
            grfInheritance: if path.is_dir() {
                SUB_CONTAINERS_AND_OBJECTS_INHERIT
            } else {
                NO_INHERITANCE
            },
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: sid.cast(),
            },
        };

        let mut acl: *mut ACL = std::ptr::null_mut();
        // SAFETY: one EXPLICIT_ACCESS_W entry, no existing ACL to merge into.
        // On success `acl` is a LocalAlloc'd block we free below.
        let status =
            unsafe { SetEntriesInAclW(1, &raw mut access, std::ptr::null_mut(), &raw mut acl) };
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }

        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);

        // SAFETY: `wide` is NUL-terminated and `acl` is the ACL built above.
        let status = unsafe {
            SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null_mut(),
            )
        };
        // SAFETY: `acl` came from SetEntriesInAclW, which allocates with LocalAlloc.
        unsafe { LocalFree(acl.cast()) };

        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        Ok(())
    }

    /// The calling process's user SID, as a `TOKEN_USER` byte buffer.
    fn current_user_sid() -> io::Result<Vec<u8>> {
        struct OwnedHandle(HANDLE);
        impl Drop for OwnedHandle {
            fn drop(&mut self) {
                // SAFETY: opened by OpenProcessToken below and closed once.
                unsafe { CloseHandle(self.0) };
            }
        }

        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: pseudo-handle from GetCurrentProcess needs no close; `token`
        // is written on success and owned below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle(token);

        // Sized by the first call, which is expected to fail for that reason.
        let mut needed = 0_u32;
        // SAFETY: a null buffer with length 0 is the documented way to ask for
        // the required size.
        let _ = unsafe {
            GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &raw mut needed)
        };
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut buffer = vec![0_u8; needed as usize];
        // SAFETY: `buffer` is `needed` bytes, the size the call just asked for.
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &raw mut needed,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property that matters everywhere: a secret is never written over
    /// something already there, whose permissions we would inherit.
    #[test]
    fn an_existing_file_is_refused_rather_than_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        std::fs::write(&path, b"planted by someone else").unwrap();

        let err = create_owner_only(&path).expect_err("must refuse an existing path");

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"planted by someone else",
            "the refusal must not have truncated it"
        );
    }

    #[test]
    fn contents_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");

        write_owner_only(&path, b"secret").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"secret");
    }

    #[cfg(unix)]
    #[test]
    fn a_new_secret_is_0600_from_creation() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");

        let file = create_owner_only(&path).unwrap();

        assert_eq!(
            file.metadata().unwrap().permissions().mode() & 0o777,
            0o600,
            "set in the open call, so the file is never briefly world-readable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn restricting_an_existing_directory_uses_0700() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("data");
        std::fs::create_dir(&nested).unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();

        restrict_existing_to_owner(&nested).unwrap();

        assert_eq!(
            std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777,
            0o700,
            "a directory needs execute to be traversable by its owner"
        );
    }

    /// The Windows half of the same guarantee. `icacls` is the supported way to
    /// read an effective ACL, and the string it prints for an inherited entry
    /// carries `(I)` — its absence is what says the DACL is protected.
    #[cfg(windows)]
    #[test]
    fn a_new_secret_does_not_inherit_the_directory_acl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");

        drop(create_owner_only(&path).unwrap());

        let out = std::process::Command::new("icacls")
            .arg(&path)
            .output()
            .expect("icacls should be present on every supported Windows");
        let acl = String::from_utf8_lossy(&out.stdout);

        assert!(
            !acl.contains("(I)"),
            "the DACL must be protected, so no entry is inherited from the parent: {acl}"
        );
    }
}
