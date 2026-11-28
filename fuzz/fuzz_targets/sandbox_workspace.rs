#![no_main]
//! A sandbox run's workspace: the sandbox controls every entry in it, and the node reads and
//! writes it on the sandbox's behalf (`calimero_sandbox::Workspace`).
//!
//! The input plants entries (symlinks to anywhere, FIFOs, directories) and asks the node to
//! write, read and collect paths built from it. Whatever it does, the node never reads the
//! secret beside the workspace, never changes or adds anything outside it, and never blocks.

use std::ffi::CString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::symlink;
use std::path::Path;

use calimero_sandbox::Workspace;
use libfuzzer_sys::fuzz_target;

const SECRET: &[u8] = b"calimero-fuzz-secret";
/// Names the input picks from for entries and path components.
const NAMES: &[&str] = &["a", "b", "dir", "fifo", "link", "..", ".", "", "/", "proc"];
/// What a planted symlink points at; `@` is the directory holding the secret.
const TARGETS: &[&str] = &[
    "@/secret",
    "../outside/secret",
    "../outside",
    "/proc/self/root@/secret",
    "/proc/self/cwd",
    "/proc/self/fd/0",
    "/etc/hostname",
    "a",
    "..",
    "/",
];

struct Input<'a>(&'a [u8]);

impl Input<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }

    fn pick<'n>(&mut self, from: &'n [&'n str]) -> Option<&'n str> {
        Some(from[usize::from(self.byte()?) % from.len()])
    }

    fn take(&mut self, n: usize) -> &[u8] {
        let n = n.min(self.0.len());
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        head
    }

    /// One to three components, from the names or from raw bytes.
    fn path(&mut self) -> Option<String> {
        let parts = 1 + usize::from(self.byte()? % 3);
        let mut path = String::new();
        for i in 0..parts {
            if i > 0 {
                path.push('/');
            }
            if self.byte()? % 4 == 0 {
                let len = usize::from(self.byte()? % 12);
                path.push_str(&String::from_utf8_lossy(self.take(len)).replace('\0', ""));
            } else {
                path.push_str(self.pick(NAMES)?);
            }
        }
        Some(path)
    }
}

/// The outside directory holds exactly the secret, unchanged.
fn outside_untouched(outside: &Path) {
    let names: Vec<_> = fs::read_dir(outside)
        .expect("outside directory")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(names, ["secret"], "something appeared outside the workspace");
    assert_eq!(
        fs::read(outside.join("secret")).expect("secret"),
        SECRET,
        "the secret changed"
    );
}

fn never_the_secret(bytes: &[u8]) {
    assert!(
        !bytes.windows(SECRET.len()).any(|w| w == SECRET),
        "the node read the secret for the sandbox"
    );
}

fuzz_target!(|data: &[u8]| {
    let root = tempfile::tempdir().expect("tempdir");
    let outside = root.path().join("outside");
    let ws_dir = root.path().join("ws");
    fs::create_dir_all(&outside).expect("outside");
    fs::create_dir_all(&ws_dir).expect("workspace");
    fs::write(outside.join("secret"), SECRET).expect("secret");
    let ws = Workspace::open(&ws_dir, 1 << 16, None).expect("open workspace");
    let outside_str = outside.to_str().expect("utf-8 temp path");
    let mut input = Input(data);
    for _ in 0..32 {
        let Some(op) = input.byte() else { break };
        match op % 6 {
            // Plant top-level entries only: the sandbox plants from inside its own root, which
            // a planting through a symlink here would not be.
            0 => {
                let (Some(name), Some(target)) = (input.pick(NAMES), input.pick(TARGETS)) else {
                    break;
                };
                let target = target.replace('@', outside_str);
                let _ = symlink(target, ws_dir.join(name.trim_matches('/')));
            }
            1 => {
                let Some(name) = input.pick(NAMES) else { break };
                let path = ws_dir.join(name.trim_matches('/'));
                if let Ok(c) = CString::new(path.as_os_str().as_encoded_bytes()) {
                    // SAFETY: a NUL-terminated path.
                    let _ = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
                }
            }
            2 => {
                let Some(name) = input.pick(NAMES) else { break };
                let _ = fs::create_dir(ws_dir.join(name.trim_matches('/')));
            }
            3 => {
                let Some(path) = input.path() else { break };
                let len = usize::from(input.byte().unwrap_or(0));
                let bytes = input.take(len).to_vec();
                let _ = ws.write(&path, &bytes);
            }
            4 => {
                let Some(path) = input.path() else { break };
                if let Ok(bytes) = ws.read(&path, 1 << 16) {
                    never_the_secret(&bytes);
                }
            }
            _ => {
                let Some(path) = input.path() else { break };
                if let Ok((mut f, _)) = ws.collect(&path) {
                    let mut bytes = Vec::new();
                    let _ = f.read_to_end(&mut bytes);
                    never_the_secret(&bytes);
                }
            }
        }
        outside_untouched(&outside);
    }
});
