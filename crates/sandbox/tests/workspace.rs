//! The node's workspace I/O against what a sandbox can plant in its workspace, without root:
//! symlinks out of it (absolute, relative, through `/proc`), a symlinked directory, a FIFO and
//! `..`. Nothing outside the workspace is ever read or written, and nothing blocks.
//! `fuzz/fuzz_targets/sandbox_workspace.rs` tries every combination; these are the named ones.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, reason = "a test binary")]

use std::ffi::CString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::symlink;
use std::path::Path;

use calimero_sandbox::Workspace;

const SECRET: &[u8] = b"the node's secret";

/// A workspace beside a directory holding a secret, with the attacks planted.
fn planted() -> (tempfile::TempDir, Workspace) {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside");
    let ws = root.path().join("ws");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(&ws).unwrap();
    fs::write(outside.join("secret"), SECRET).unwrap();
    symlink(outside.join("secret"), ws.join("abs")).unwrap();
    symlink("../outside/secret", ws.join("rel")).unwrap();
    symlink("/proc/self/root/etc/hostname", ws.join("magic")).unwrap();
    symlink(&outside, ws.join("dir")).unwrap();
    let fifo = CString::new(ws.join("fifo").as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    fs::write(ws.join("plain"), b"ok").unwrap();
    let workspace = Workspace::open(&ws, 1 << 20, None).unwrap();
    (root, workspace)
}

fn outside_untouched(root: &Path) {
    let mut names: Vec<_> = fs::read_dir(root.join("outside"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["secret"]);
    assert_eq!(fs::read(root.join("outside/secret")).unwrap(), SECRET);
}

#[test]
fn planted_links_fifos_and_dot_dots_are_refused_for_every_call() {
    let (root, ws) = planted();
    for path in [
        "abs",
        "rel",
        "magic",
        "dir/secret",
        "dir/new",
        "fifo",
        "../outside/secret",
        "/etc/hostname",
        "plain/../../outside/secret",
        "./../outside/secret",
        "",
    ] {
        assert!(ws.read(path, 1024).is_err(), "read {path:?}");
        assert!(ws.collect(path).is_err(), "collect {path:?}");
        assert!(ws.write(path, b"pwned").is_err(), "write {path:?}");
    }
    outside_untouched(root.path());
}

#[test]
fn regular_files_beneath_the_workspace_work() {
    let (root, ws) = planted();
    assert_eq!(ws.read("plain", 1024).unwrap(), b"ok");
    ws.write("new", b"fresh").unwrap();
    assert_eq!(ws.read("new", 2).unwrap(), b"fr");
    let (mut f, len) = ws.collect("new").unwrap();
    let mut body = Vec::new();
    let _ = f.read_to_end(&mut body).unwrap();
    assert_eq!((len, body.as_slice()), (5, &b"fresh"[..]));
    // Replacing a file truncates it first.
    ws.write("new", b"x").unwrap();
    assert_eq!(ws.read("new", 1024).unwrap(), b"x");
    // A file over the workspace's size is refused before anything is written.
    assert!(ws.write("big", &vec![0; (1 << 20) + 1]).is_err());
    assert!(ws.read("big", 1).is_err());
    outside_untouched(root.path());
}
