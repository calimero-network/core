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
//!
//! Ids in these comments cite `docs/specs/app-runtime-v0.14.md` (D decisions in section 2): D27
//! (an action's sub-call key adds the method, argument hash and occurrence).

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

/// Whether a synchronous xcall is a call (logged in the target) or a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XcallKind {
    /// `xcall.call`: one logged call of the target, under a key derived from the run's.
    Call,
    /// `xcall.view`: a read of the target's latest committed state.
    View,
}

/// One request of a run's batch (`xcall.batch`), as the guest made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XcallRequest {
    /// A call or a view.
    pub kind: XcallKind,
    /// The target app.
    pub app: String,
    /// The target method.
    pub method: String,
    /// The JSON arguments.
    pub args: Vec<u8>,
}

/// One request of a batch the host core checked and keyed, as the node is handed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncXcall<'a> {
    /// The target app.
    pub app: &'a str,
    /// The target method.
    pub method: &'a str,
    /// The JSON arguments.
    pub args: &'a [u8],
    /// The call's idempotency key ([`sync_xcall_key`]); `None` for a view.
    pub key: Option<&'a str>,
}

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
    /// Run every request of a batch and answer each, in order: a request with a key as
    /// [`Self::call`], one without as [`Self::view`]. This default runs them one after
    /// another; a host that can should run them at once, so the run waits for the slowest
    /// rather than for their sum.
    fn batch(&self, requests: &[SyncXcall<'_>]) -> Vec<Result<Vec<u8>, String>> {
        requests
            .iter()
            .map(|r| match r.key {
                Some(key) => self.call(r.app, r.method, r.args, key),
                None => self.view(r.app, r.method, r.args),
            })
            .collect()
    }
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
