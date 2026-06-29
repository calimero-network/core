//! Process-global throttle for the public `POST /tee/verify-quote` endpoint.
//!
//! `/verify-quote` is mounted unauthenticated (the mdma manager proxies to it
//! without a node admin token), and it drives the heavy
//! [`calimero_tee_attestation::verify_attestation`] path — an outbound
//! Intel-PCS collateral fetch + DCAP crypto-verify — on an attacker-supplied
//! quote (TEE-01 / audit #325). Since it must stay public, throttle it: a
//! global token bucket bounds the request rate and a bounded semaphore caps
//! concurrent verifies, so an unauthenticated caller cannot turn the endpoint
//! into a CPU-DoS / outbound-PCS amplifier.
//!
//! The limit is intentionally *global* (not per-client): the endpoint has no
//! authenticated identity to key on, and the amplification concern is the
//! aggregate outbound/CPU fan-out, not fairness between callers.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Maximum concurrent `/verify-quote` verifies. Each does a blocking-ish
/// outbound PCS fetch, so this caps the aggregate fan-out.
pub const DEFAULT_MAX_INFLIGHT: usize = 4;

/// Token-bucket burst: this many requests may be served back-to-back before
/// the caller is throttled to the refill rate.
pub const DEFAULT_BURST: f64 = 10.0;

/// One token is restored per this interval, up to [`DEFAULT_BURST`]. At the
/// default (1 token / 2s) a saturating caller is held to ~30 verifies/min.
pub const DEFAULT_REFILL: Duration = Duration::from_secs(2);

/// Outcome of consulting the throttle.
#[derive(Debug)]
pub enum Decision {
    /// Proceed; hold `permit` until the verify completes.
    Proceed(OwnedSemaphorePermit),
    /// Token bucket empty — reject with 429.
    RateLimited,
    /// Inflight cap saturated — reject with 429.
    AtCapacity,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Global rate + concurrency limiter for `/verify-quote`. Interior mutability
/// (a small `Mutex` never held across `.await`) so a single shared instance
/// can be consulted from concurrent request handlers.
pub struct VerifyQuoteThrottle {
    inflight: Arc<Semaphore>,
    bucket: Mutex<Bucket>,
    burst: f64,
    refill: Duration,
}

impl VerifyQuoteThrottle {
    /// Construct a throttle with explicit limits.
    ///
    /// # Panics
    ///
    /// Panics if `max_inflight == 0`, `burst < 1.0`, or `refill == 0`: with no
    /// inflight permits no request could ever proceed, a sub-unit burst can
    /// never satisfy the `tokens >= 1.0` gate, and a zero refill interval would
    /// make the refill rate non-finite (poisoning the lazy-refill arithmetic
    /// with `NaN`) — so each renders the endpoint useless. These are
    /// construction-time programmer errors — the only in-tree callers are
    /// [`Default`] and tests, both passing valid constants — so they are
    /// asserted rather than surfaced as a `Result`.
    pub fn new(max_inflight: usize, burst: f64, refill: Duration) -> Self {
        assert!(max_inflight > 0, "max_inflight must be positive");
        assert!(burst >= 1.0, "burst must be >= 1");
        assert!(refill > Duration::ZERO, "refill must be positive");
        Self {
            inflight: Arc::new(Semaphore::new(max_inflight)),
            bucket: Mutex::new(Bucket {
                tokens: burst,
                last: Instant::now(),
            }),
            burst,
            refill,
        }
    }

    /// Consult the throttle for a request observed at `now`. On
    /// `Decision::Proceed` one token and one inflight permit are consumed; on
    /// rejection nothing is consumed.
    pub fn check_at(&self, now: Instant) -> Decision {
        // `refill` is asserted `> 0` in `new`, so this is always finite.
        let refill_per_sec = 1.0 / self.refill.as_secs_f64();

        {
            // Recover from a poisoned mutex rather than propagating the panic:
            // the only data behind the lock is the token bucket, and a stale
            // bucket from a thread that panicked mid-update is harmless (the
            // refill below reconciles it against `now`). Letting the poison
            // propagate via `.expect()` would instead wedge the endpoint
            // permanently — every later request would re-panic (AGENTS.md: avoid
            // `.expect()`).
            let mut bucket = self
                .bucket
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // Compute the lazily-refilled token count but do NOT write it back
            // yet — the bucket is mutated transactionally, only on `Proceed`. A
            // rejected request (RateLimited or AtCapacity) leaves `tokens` and
            // `last` untouched, so it neither burns a token nor advances the
            // refill clock; the bucket keeps refilling from the last *grant*.
            let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
            let refilled = (bucket.tokens + elapsed * refill_per_sec).min(self.burst);
            if refilled < 1.0 {
                return Decision::RateLimited;
            }

            // Acquire the inflight permit while holding the bucket lock so the
            // token and permit are committed atomically. `try_acquire_owned`
            // never blocks (and the lock is never held across an await), so the
            // hold is bounded to a few instructions — releasing and re-taking
            // the lock around the acquire would instead open a TOCTOU window
            // where a concurrent grant could be clobbered, so the lock is held
            // across the acquire intentionally.
            match Arc::clone(&self.inflight).try_acquire_owned() {
                Ok(permit) => {
                    // Commit only on success: the token and refill clock advance
                    // together with the permit grant.
                    bucket.tokens = refilled - 1.0;
                    bucket.last = now;
                    Decision::Proceed(permit)
                }
                // AtCapacity: leave the bucket untouched — no token is burned
                // and `last` is not advanced, so the refill clock keeps running
                // from the previous grant.
                Err(_) => Decision::AtCapacity,
            }
        }
    }

    /// Consult the throttle for a request observed now.
    pub fn check(&self) -> Decision {
        self.check_at(Instant::now())
    }
}

impl Default for VerifyQuoteThrottle {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_INFLIGHT, DEFAULT_BURST, DEFAULT_REFILL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bursts_then_rate_limits() {
        // Burst 3, refill 1 token/sec, generous inflight so capacity isn't the
        // gate. Permits are dropped immediately so only the token bucket bites.
        let t = VerifyQuoteThrottle::new(1000, 3.0, Duration::from_secs(1));
        let now = Instant::now();
        for i in 0..3 {
            assert!(
                matches!(t.check_at(now), Decision::Proceed(_)),
                "burst {i} should pass"
            );
        }
        assert!(matches!(t.check_at(now), Decision::RateLimited));
        // One token refills after a second.
        assert!(matches!(
            t.check_at(now + Duration::from_secs(1)),
            Decision::Proceed(_)
        ));
        assert!(matches!(
            t.check_at(now + Duration::from_secs(1)),
            Decision::RateLimited
        ));
    }

    #[test]
    fn rejections_do_not_slow_token_recovery() {
        // Regression guard for the transactional bucket: `tokens`/`last` are
        // committed only on `Proceed`, so a rejected request must not steal
        // refill credit. Burst 1, refill 1 token/sec, generous inflight.
        let t = VerifyQuoteThrottle::new(1000, 1.0, Duration::from_secs(1));
        let t0 = Instant::now();
        assert!(matches!(t.check_at(t0), Decision::Proceed(_)));
        // Half a second in: only 0.5 token → rejected, and this call must NOT
        // advance the refill clock.
        assert!(matches!(
            t.check_at(t0 + Duration::from_millis(500)),
            Decision::RateLimited
        ));
        // At exactly t0+1s a full token is back despite the intervening
        // rejection — recovery tracks wall-clock from the last grant.
        assert!(matches!(
            t.check_at(t0 + Duration::from_secs(1)),
            Decision::Proceed(_)
        ));
    }

    #[test]
    fn inflight_cap_blocks_when_saturated() {
        // Cap 2; huge burst so the rate limit isn't the gate. Hold the permits.
        let t = VerifyQuoteThrottle::new(2, 1000.0, Duration::from_millis(1));
        let now = Instant::now();
        let p1 = match t.check_at(now) {
            Decision::Proceed(p) => p,
            other => panic!("expected Proceed, got {other:?}"),
        };
        let _p2 = match t.check_at(now) {
            Decision::Proceed(p) => p,
            other => panic!("expected Proceed, got {other:?}"),
        };
        assert!(matches!(t.check_at(now), Decision::AtCapacity));
        drop(p1);
        assert!(matches!(t.check_at(now), Decision::Proceed(_)));
    }

    #[test]
    fn rate_limited_request_does_not_consume_permit() {
        // Burst 1, refill never. After the first request the bucket is empty;
        // the rejection must not have taken an inflight permit.
        let t = VerifyQuoteThrottle::new(2, 1.0, Duration::from_secs(100_000));
        let now = Instant::now();
        let _held = match t.check_at(now) {
            Decision::Proceed(p) => p,
            other => panic!("expected Proceed, got {other:?}"),
        };
        // Rate limited (bucket empty), even though an inflight slot is free.
        assert!(matches!(t.check_at(now), Decision::RateLimited));
        assert_eq!(t.inflight.available_permits(), 1);
    }
}
