//! Actions (D12, D17, D22, D27) and outbound HTTP (S9), as the host core sees them.
//!
//! An action runs on the leader, outside the log, with storage disabled: it reads through
//! views and writes by calling methods, each its own entry, through the node's
//! [`ActionHost`]. Every method it calls gets a key derived from the action's key, the
//! method, a hash of the arguments and an occurrence number (D27), so a retried action's
//! calls replay instead of applying twice. Only an action may send HTTP (`wasi:http`), and
//! only through the node's [`HttpHost`], which allows the hosts the signed manifest
//! declares (S9).

use sha2::{Digest, Sha256};

/// Bytes of an outgoing or incoming HTTP body an action holds at most.
pub const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
/// HTTP requests one action run may send.
pub const MAX_HTTP_REQUESTS: u32 = 8;
/// `ctx.view` and `ctx.call` one action run may make.
pub const MAX_ACTION_SUBCALLS: u32 = 1_000;
/// Actions one call or timer may start.
pub const MAX_STARTED_ACTIONS: usize = 100;

/// What the node lets an action do: its key, views, and calls of the app's methods as
/// the action's caller.
pub trait ActionHost: Send + Sync {
    /// The action's key (its id), the same on every attempt (D17).
    fn key(&self) -> String;
    /// Run view `method` on the app; the JSON result, or why not.
    fn view(&self, method: &str, args: &[u8]) -> Result<Vec<u8>, String>;
    /// Run `method` as a call of the app, one log entry under idempotency key `key`; the
    /// JSON result, or why not.
    fn call(&self, method: &str, args: &[u8], key: &str) -> Result<Vec<u8>, String>;
}

/// An outgoing request, as an action's `wasi:http` handed it over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    /// `scheme://authority/path?query`.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The node's gate for an action's outbound requests (S9): declared hosts, the owner's
/// approval, secrets (R14).
pub trait HttpHost: Send + Sync {
    /// Send `request`; the response, or why it was not sent.
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String>;
}

/// A run with no outbound gate: nothing is sent.
pub struct NoHttp;

impl HttpHost for NoHttp {
    fn send(&self, _request: &HttpRequest) -> Result<HttpResponse, String> {
        Err("outbound http is not available to this app".to_owned())
    }
}

/// An action that `start_action` queued; the executor logs it as `ActionStarted` (D17).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionStart {
    /// The action's key.
    pub id: String,
    pub method: String,
    pub args: Vec<u8>,
}

/// The key of the `occurrence`-th call of `method` with `args` in action `action_key`
/// (D27): hex SHA-256 over the four, prefixed `act:`.
#[must_use]
pub fn subcall_key(action_key: &str, method: &str, args: &[u8], occurrence: u32) -> String {
    let args_hash = Sha256::digest(args);
    let mut h = Sha256::new();
    h.update(action_key.as_bytes());
    h.update([0]);
    h.update(method.as_bytes());
    h.update([0]);
    h.update(args_hash);
    h.update(occurrence.to_be_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(4 + 64);
    out.push_str("act:");
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subcall_keys_differ_by_method_args_and_occurrence() {
        let a = subcall_key("k", "m", b"{}", 0);
        assert_eq!(a, subcall_key("k", "m", b"{}", 0));
        assert_ne!(a, subcall_key("k", "m", b"{}", 1));
        assert_ne!(a, subcall_key("k", "n", b"{}", 0));
        assert_ne!(a, subcall_key("k", "m", b"{\"a\":1}", 0));
        assert_ne!(a, subcall_key("j", "m", b"{}", 0));
        assert!(a.starts_with("act:"));
    }
}
