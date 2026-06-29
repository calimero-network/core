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
//! # Semantics
//!
//! `max_attempts` is the number of failures **allowed within the window before
//! lockout**: the bucket locks once it holds `max_attempts` in-window failures.
//! With the default of 5, the 5th failure trips the lockout. Callers
//! `check()` before attempting and `record_failure()` after a failure, so the
//! gate is a sliding window over recent failures, not a hard per-session count.
//!
//! Because `check()` and `record_failure()` take the lock separately, two
//! concurrent requests for the same key can both pass `check()` at the
//! threshold boundary and each record a failure — an inherent off-by-one in the
//! check-then-act pattern. For a brute-force throttle this is immaterial (one
//! extra attempt in a 60s window does not change the economics) and is not
//! worth serialising every login behind a single `check_and_record` critical
//! section.
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
//! - **Wall-clock, not monotonic**: timestamps come from `SystemTime` so they
//!   survive across the explicit-time API and tests. A forward clock jump can
//!   retire an in-window failure early (shortening a lockout); a backward jump
//!   only ever widens it. The effect is bounded by the 60s window and accepted
//!   — a monotonic `Instant` doesn't serialise to the `u64` ms the API uses.
//!
//! To stop identity rotation from growing memory without bound, the tracked-key
//! map is capped at [`MAX_TRACKED_KEYS`]. When full, [`record_failure_at`]
//! first reclaims buckets whose failures have all aged out, then (if still
//! full) evicts the least-recently-active bucket — so a new identity is always
//! tracked rather than waved through unthrottled.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::warn;

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
    /// limiter.
    ///
    /// Reuse — not clear — on poison recovery is the fail-*closed* choice here:
    /// clearing the map would wipe every live lockout and let all in-progress
    /// brute-force attempts start over (fail-open), which is exactly what this
    /// limiter exists to prevent. Reusing preserves existing lockouts.
    ///
    /// Reuse is also safe in practice: every mutation under this lock is a
    /// `push`, a `retain`, or a `remove` over `Vec<u64>`/`HashMap` — none of
    /// which run user code that can panic, so the only realistic poison source
    /// is a panic elsewhere on the thread, leaving the map structurally intact.
    /// A worst-case partially-updated bucket can at most mis-time one
    /// identity's lockout by a few failures, which the sliding window
    /// self-heals on the next call.
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
        // Bound memory at the cap. Rather than silently dropping failures for
        // every new identity once full — which would let an attacker first
        // exhaust the key space with 100k throwaway identities and then
        // brute-force any *new* identity completely unthrottled — make room:
        //   1. Reclaim buckets whose failures have all aged out of the window.
        //      Under normal churn this alone frees space and touches no live
        //      lockout.
        //   2. If every tracked identity still has an in-window failure (a
        //      genuine 100k-distinct-identity flood), evict the least-recently-
        //      active bucket so the new identity is still tracked. We log this
        //      (no key material) so operators are alerted to map flooding.
        if !map.contains_key(key) && map.len() >= MAX_TRACKED_KEYS {
            reclaim_expired(&mut map, now, self.window_ms);
            if map.len() >= MAX_TRACKED_KEYS {
                if let Some(victim) = oldest_active_key(&map) {
                    drop(map.remove(&victim));
                    warn!(
                        tracked_keys = map.len(),
                        "Login rate-limiter at capacity; evicting oldest bucket (possible identity-rotation flood)"
                    );
                } else {
                    return;
                }
            }
        }
        let failures = map.entry(key.to_owned()).or_default();
        prune(failures, now, self.window_ms);
        // Cap per-key history. Once the bucket holds `max_attempts` in-window
        // failures the caller is already locked out, and the unlock time is
        // fixed by the *oldest* in-window failure (`failures[0]`), so further
        // timestamps cannot extend or change the lockout — recording them would
        // only grow the Vec without bound under a high-rate flood. Drop them.
        if failures.len() >= self.max_attempts as usize {
            return;
        }
        failures.push(now);
    }
}

/// Drop failure timestamps older than the window.
fn prune(failures: &mut Vec<u64>, now: u64, window_ms: u64) {
    let cutoff = now.saturating_sub(window_ms);
    failures.retain(|&t| t >= cutoff);
}

/// Remove every bucket whose failures have all aged out of the window. Keeps
/// the map bounded to identities with *live* lockouts.
fn reclaim_expired(map: &mut HashMap<String, Vec<u64>>, now: u64, window_ms: u64) {
    map.retain(|_, failures| {
        prune(failures, now, window_ms);
        !failures.is_empty()
    });
}

/// The key of the least-recently-active bucket (smallest most-recent failure
/// timestamp), used to pick an eviction victim when the map is full of live
/// buckets. `None` only if the map is empty.
fn oldest_active_key(map: &HashMap<String, Vec<u64>>) -> Option<String> {
    map.iter()
        .min_by_key(|(_, failures)| failures.iter().copied().max().unwrap_or(0))
        .map(|(k, _)| k.clone())
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

    #[test]
    fn per_key_history_is_capped_under_flood() {
        let rl = LoginRateLimiter::new(3, 60_000);
        let key = "flooder";
        // 50 failures, all inside the window: the bucket must not grow past
        // `max_attempts`, since extra in-window failures cannot change the
        // (oldest-failure-based) unlock time.
        for t in 0..50 {
            rl.record_failure_at(key, t);
        }
        let len = rl.lock().get(key).map(Vec::len).unwrap_or(0);
        assert_eq!(len, 3, "bucket must be capped at max_attempts entries");
        // Still correctly locked out.
        assert!(rl.check_at(key, 100).is_some(), "flooded key stays locked");
    }

    #[test]
    fn reclaim_expired_drops_only_aged_buckets() {
        let mut map: HashMap<String, Vec<u64>> = HashMap::new();
        let _ = map.insert("live".to_owned(), vec![50_000]);
        let _ = map.insert("aged".to_owned(), vec![0, 100]);
        // now=70_000, window=60_000 → cutoff 10_000: "aged" (<=100) is gone,
        // "live" (50_000) survives.
        reclaim_expired(&mut map, 70_000, 60_000);
        assert!(map.contains_key("live"));
        assert!(!map.contains_key("aged"));
    }

    #[test]
    fn oldest_active_key_picks_least_recently_active() {
        let mut map: HashMap<String, Vec<u64>> = HashMap::new();
        let _ = map.insert("old".to_owned(), vec![10, 20]); // most recent = 20
        let _ = map.insert("new".to_owned(), vec![100]); // most recent = 100
        assert_eq!(oldest_active_key(&map).as_deref(), Some("old"));
    }
}
