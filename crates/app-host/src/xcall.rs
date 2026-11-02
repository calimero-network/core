//! Synchronous xcalls: a run calls a method of another app of its organization and waits
//! for the answer, as a call (logged in the target's own log, under a key derived from
//! the run's) or as a view (the target's latest committed state).
//!
//! A synchronous call is not atomic with the run that makes it: the target's entry commits
//! first, whatever the caller's run then does. Its key is derived from the caller's key,
//! the target, the method, the arguments and how many identical calls came before in the
//! run (as D27 does for actions), so a re-run of the caller (a retry, a leader race)
//! replays the target's answer instead of applying it twice. A run that is itself
//! delivered from another app (`kind = xcall-deliver`) makes none, so a chain is one hop
//! deep.

use sha2::{Digest, Sha256};

/// Synchronous xcalls (calls and views together) one run may make.
pub const MAX_SYNC_XCALLS: u32 = 16;
/// Bytes of a synchronous xcall's arguments.
pub const MAX_SYNC_XCALL_ARGS_BYTES: usize = 512 * 1024;
/// Bytes of a synchronous xcall's answer the run is handed.
pub const MAX_SYNC_XCALL_RESULT_BYTES: usize = 1024 * 1024;
/// More than [`MAX_SYNC_XCALLS`] in one run.
pub const XCALL_SYNC_TOO_MANY: &str = "too many synchronous xcalls in one run";
/// A synchronous xcall to the running app itself.
pub const XCALL_SYNC_SELF: &str = "a synchronous xcall cannot target the app itself";
/// The node gave the run no way to make one.
pub const XCALL_SYNC_UNAVAILABLE: &str = "synchronous xcalls are not available to this run";

/// What the node lets a run reach in other apps, as `Caller::App(<this app>)`.
pub trait XcallHost: Send + Sync {
    /// The run's own key, the base of every call's key: the same on every re-run.
    fn key(&self) -> String;
    /// Run `method` of `app` as one logged call under idempotency key `key`; the JSON
    /// result, or the failure as JSON text.
    fn call(&self, app: &str, method: &str, args: &[u8], key: &str) -> Result<Vec<u8>, String>;
    /// Run view `method` of `app` on its latest committed state; the JSON result, or the
    /// failure as JSON text.
    fn view(&self, app: &str, method: &str, args: &[u8]) -> Result<Vec<u8>, String>;
}

/// The key of the `occurrence`-th synchronous call of `app`'s `method` with `args` in the
/// run keyed `run_key`: hex SHA-256 over the five, prefixed `xc:`.
#[must_use]
pub fn sync_xcall_key(
    run_key: &str,
    app: &str,
    method: &str,
    args: &[u8],
    occurrence: u32,
) -> String {
    let args_hash = Sha256::digest(args);
    let mut h = Sha256::new();
    for part in [run_key, app, method] {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part.as_bytes());
    }
    h.update(args_hash);
    h.update(occurrence.to_be_bytes());
    let mut out = String::with_capacity(3 + 64);
    out.push_str("xc:");
    for b in h.finalize() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_keys_differ_by_every_part() {
        let a = sync_xcall_key("k", "app", "m", b"{}", 0);
        assert_eq!(a, sync_xcall_key("k", "app", "m", b"{}", 0));
        assert_ne!(a, sync_xcall_key("j", "app", "m", b"{}", 0));
        assert_ne!(a, sync_xcall_key("k", "other", "m", b"{}", 0));
        assert_ne!(a, sync_xcall_key("k", "app", "n", b"{}", 0));
        assert_ne!(a, sync_xcall_key("k", "app", "m", b"{\"a\":1}", 0));
        assert_ne!(a, sync_xcall_key("k", "app", "m", b"{}", 1));
        // Length-prefixed: shifting bytes between parts makes another key.
        assert_ne!(
            sync_xcall_key("ka", "pp", "m", b"{}", 0),
            sync_xcall_key("k", "app", "m", b"{}", 0)
        );
        assert!(a.starts_with("xc:"));
    }
}
