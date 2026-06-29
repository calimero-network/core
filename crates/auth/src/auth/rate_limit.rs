//! Login brute-force throttling.
//!
//! A small in-memory sliding-window limiter that bounds failed authentication
//! attempts per caller identity. After `max_attempts` failures within
//! `window`, further attempts are rejected with a lockout until the window
//! clears. A successful authentication resets the counter.
//!
//! Time is passed in explicitly (`*_at(now_ms)`) so the behaviour is
//! deterministic in tests; the public wrappers stamp the real clock.
//!
//! # Known limitations (intentionally out of scope; tracked follow-ups)
//!
//! - **In-memory / per-process**: counters live in the heap and reset on process
//!   restart (crash, OOM-kill, deliberate restart), so an attacker able to
//!   restart the process can reset the lockout. Production hardening would
//!   persist counts to the store.
//! - **Identity-keyed, not IP-keyed**: the key is the request identity
//!   (`auth_method|public_key`). An attacker who rotates the public key gets a
//!   fresh bucket; IP-based limiting (which closes that) needs `ConnectInfo`
//!   wiring at the server.
//!
//! To stop identity rotation from growing memory without bound, the tracked-key
//! map is capped at [`MAX_TRACKED_KEYS`]: once full, failures for *new* keys are
//! not tracked (existing buckets continue to lock out as normal).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default: 5 failed attempts per 60s window before lockout.
const DEFAULT_MAX_ATTEMPTS: u32 = 5;
const DEFAULT_WINDOW_MS: u64 = 60_000;

/// Upper bound on distinct identities tracked at once, so an attacker rotating
/// identities cannot grow the map without bound.
const MAX_TRACKED_KEYS: usize = 100_000;

/// Current wall-clock time in milliseconds since the UNIX epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug)]
pub struct LoginRateLimiter {
    inner: Mutex<HashMap<String, Vec<u64>>>,
    max_attempts: u32,
    window_ms: u64,
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ATTEMPTS, DEFAULT_WINDOW_MS)
    }
}

impl LoginRateLimiter {
    #[must_use]
    pub fn new(max_attempts: u32, window_ms: u64) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            max_attempts,
            window_ms,
        }
    }

    /// If `key` is currently locked out, return `Some(retry_after_secs)`.
    /// Call this *before* attempting authentication.
    pub fn check(&self, key: &str) -> Option<u64> {
        self.check_at(key, now_ms())
    }

    /// Record a failed attempt for `key`.
    pub fn record_failure(&self, key: &str) {
        self.record_failure_at(key, now_ms());
    }

    /// Clear all recorded failures for `key` (call on successful auth).
    pub fn reset(&self, key: &str) {
        drop(self.lock().remove(key));
    }

    /// Lock the map, recovering from poisoning rather than disabling the
    /// limiter: if a thread panicked while holding the lock, silently returning
    /// "not locked" / dropping the failure would turn off brute-force protection
    /// entirely. The inner map is still consistent, so reuse it.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Vec<u64>>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn check_at(&self, key: &str, now: u64) -> Option<u64> {
        let mut map = self.lock();
        let failures = map.get_mut(key)?;
        prune(failures, now, self.window_ms);

        if failures.is_empty() {
            drop(map.remove(key));
            return None;
        }
        if failures.len() as u32 >= self.max_attempts {
            // Locked until the oldest in-window failure ages out.
            let unlock_at = failures[0].saturating_add(self.window_ms);
            let retry_after_ms = unlock_at.saturating_sub(now);
            return Some(retry_after_ms.div_ceil(1000).max(1));
        }
        None
    }

    fn record_failure_at(&self, key: &str, now: u64) {
        let mut map = self.lock();
        // Bound memory: once the cap is reached, do not start tracking new
        // identities (existing buckets still record and lock out).
        if !map.contains_key(key) && map.len() >= MAX_TRACKED_KEYS {
            return;
        }
        let failures = map.entry(key.to_owned()).or_default();
        prune(failures, now, self.window_ms);
        failures.push(now);
    }
}

/// Drop failure timestamps older than the window.
fn prune(failures: &mut Vec<u64>, now: u64, window_ms: u64) {
    let cutoff = now.saturating_sub(window_ms);
    failures.retain(|&t| t >= cutoff);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_out_after_max_attempts_and_recovers_after_window() {
        let rl = LoginRateLimiter::new(3, 60_000);
        let key = "user|pk|client";

        // Not locked initially.
        assert_eq!(rl.check_at(key, 0), None);

        // First 3 failures within the window: still allowed up to the limit.
        rl.record_failure_at(key, 0);
        rl.record_failure_at(key, 1_000);
        assert_eq!(rl.check_at(key, 2_000), None, "2 failures < 3 → allowed");
        rl.record_failure_at(key, 2_000);

        // 3rd failure reached the limit → locked, with a positive retry-after.
        let retry = rl.check_at(key, 3_000).expect("must be locked");
        assert!((1..=60).contains(&retry), "retry-after seconds: {retry}");

        // Still locked just before the window clears.
        assert!(rl.check_at(key, 59_000).is_some());

        // After the oldest failure ages out of the window, allowed again.
        assert_eq!(rl.check_at(key, 61_001), None);
    }

    #[test]
    fn success_resets_the_counter() {
        let rl = LoginRateLimiter::new(3, 60_000);
        let key = "k";
        rl.record_failure_at(key, 0);
        rl.record_failure_at(key, 1);
        rl.record_failure_at(key, 2);
        assert!(rl.check_at(key, 3).is_some(), "locked after 3 failures");

        rl.reset(key);
        assert_eq!(rl.check_at(key, 4), None, "reset clears the lockout");
    }

    #[test]
    fn distinct_keys_are_independent() {
        let rl = LoginRateLimiter::new(2, 60_000);
        rl.record_failure_at("a", 0);
        rl.record_failure_at("a", 1);
        assert!(rl.check_at("a", 2).is_some());
        // A different identity is unaffected.
        assert_eq!(rl.check_at("b", 2), None);
    }
}
