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
//! NOTE: keying is by request identity (auth method + public key + client
//! name). IP-based limiting — which would also throttle attackers that rotate
//! the request identity — requires `ConnectInfo` wiring at the server and is a
//! tracked follow-up.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default: 5 failed attempts per 60s window before lockout.
const DEFAULT_MAX_ATTEMPTS: u32 = 5;
const DEFAULT_WINDOW_MS: u64 = 60_000;

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
        if let Ok(mut map) = self.inner.lock() {
            let _ = map.remove(key);
        }
    }

    fn check_at(&self, key: &str, now: u64) -> Option<u64> {
        let mut map = self.inner.lock().ok()?;
        let failures = map.get_mut(key)?;
        prune(failures, now, self.window_ms);
        if failures.len() as u32 >= self.max_attempts {
            // Locked until the oldest in-window failure ages out.
            let oldest = *failures.first()?;
            let unlock_at = oldest.saturating_add(self.window_ms);
            let retry_after_ms = unlock_at.saturating_sub(now);
            Some(retry_after_ms.div_ceil(1000).max(1))
        } else {
            if failures.is_empty() {
                let _ = map.remove(key);
            }
            None
        }
    }

    fn record_failure_at(&self, key: &str, now: u64) {
        if let Ok(mut map) = self.inner.lock() {
            let failures = map.entry(key.to_owned()).or_default();
            prune(failures, now, self.window_ms);
            failures.push(now);
        }
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
        assert!(retry >= 1 && retry <= 60, "retry-after seconds: {retry}");

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
