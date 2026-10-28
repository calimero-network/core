//! Idempotency retention (D74), one rule for both executors.
//!
//! Records are kept by time class: a key containing `node:` (one the node gives a
//! call that came without a key, or one a client created for its own retries,
//! `node:c-…`) for [`NODE_KEY_TTL_MS`] (10 minutes), a caller's own key for
//! [`IDEMPOTENCY_TTL_MS`] (24 hours). A record expires once the newest applied
//! entry's timestamp (D16) is past its own by that time, never by a node's local
//! clock, so every replica drops the same records: retention runs inside `apply`,
//! on what the log carries. At most [`IDEMPOTENCY_MAX_RECORDS`] (100,000) records
//! per app, oldest (by the entry that wrote them) dropped first.
//!
//! The newest [`IDEMPOTENCY_MIN_KEEP`] caller keys and [`NODE_KEY_MIN_KEEP`] node keys
//! of an app are kept whatever their age (release-v2-parity's floors, Xil's review,
//! issue 10: an entry stamped by a leader whose clock jumped ahead would otherwise
//! expire acknowledged calls, and their retries would run again).
//!
//! Each executor keeps its records in its own rows (`app::rows::IDEM` and
//! `rows::expiry` for wasmer, `rows::CIDEM` and `rows::corder` for components);
//! this module is the one rule over both.

use calimero_log::StateTx;
use serde::{Deserialize, Serialize};

/// How long a caller's own idempotency key is remembered (D74).
pub const IDEMPOTENCY_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// Keys the node itself gives a call that came without one, so a retried
/// forward replays instead of running twice, and the prefix of the keys a client
/// creates for its own retries (`node:c-…`, D70). They only need to outlive the
/// retries, so they are kept briefly.
pub const NODE_KEY_PREFIX: &str = "node:";

/// How long a `node:` key is remembered (D74).
pub const NODE_KEY_TTL_MS: u64 = 10 * 60 * 1000;

/// The newest caller-key records an app always keeps, whatever the timestamps
/// say: expiry reads entry timestamps, and an entry stamped by a leader whose
/// clock jumped ahead would otherwise forget acknowledged calls and run their
/// retries again (Xil's review, issue 10). Past this many, a record goes once it
/// is also older than its TTL.
pub const IDEMPOTENCY_MIN_KEEP: u64 = 10_000;

/// The same for `node:` keys.
pub const NODE_KEY_MIN_KEEP: u64 = 1_000;

/// The most records one app keeps at all, both classes together, oldest dropped
/// first (D74; D61's figure, now an upper bound).
pub const IDEMPOTENCY_MAX_RECORDS: u64 = 100_000;

/// The most records one entry's apply drops (by time and by the cap together, D74
/// in spec v0.26: expiry and eviction do a bounded amount of work per entry; what's
/// left carries over to later entries). Each dropped record costs about two reads
/// and two deletes, so an entry does at most ~16k row operations: an apply batch of
/// a few hundred KB and a few milliseconds on every replica, where an app the
/// previous release let hold 1,000,000 records per class would otherwise drop ~1.9M
/// in one batch on its first entry after the upgrade (about 470 entries now). An app
/// in its steady state drops far fewer per entry, so the budget never shows.
pub const EXPIRY_STEP: u64 = 4_096;

/// The keys of one class in the order their records were written, oldest first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ring {
    pub first: u64,
    pub next: u64,
}

impl Ring {
    const fn len(self) -> u64 {
        self.next - self.first
    }
}

/// The class of a stored key: `0` a caller's own key, `1` a `node:` key.
#[must_use]
pub fn class_of(key: &str) -> usize {
    usize::from(key.contains(NODE_KEY_PREFIX))
}

/// How long a record of `class` is kept past its entry's timestamp.
#[must_use]
pub const fn ttl_ms(class: usize) -> u64 {
    if class == 0 {
        IDEMPOTENCY_TTL_MS
    } else {
        NODE_KEY_TTL_MS
    }
}

/// The floors and the cap retention applies; the defaults are D74's. Tests scale
/// them down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Records kept per class whatever their age (caller, node).
    pub keep: [u64; 2],
    /// Records kept per app at most.
    pub cap: u64,
    /// Records one entry's apply drops at most.
    pub step: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            keep: [IDEMPOTENCY_MIN_KEEP, NODE_KEY_MIN_KEEP],
            cap: IDEMPOTENCY_MAX_RECORDS,
            step: EXPIRY_STEP,
        }
    }
}

/// Where one executor keeps its records: the hidden-row tag of the records, and
/// the row of a class's ring slot.
pub struct Rows {
    pub tag: u8,
    pub ring: fn(u8, u64) -> Vec<u8>,
}

/// The time records expire against when an entry stamped `entry_ts_ms` applies
/// (D74, spec v0.26): the earlier of the newest applied entry's timestamp and the
/// entry's own, never a node's clock. So every replica expires the same records, and
/// one entry stamped by a skewed leader can't expire everything written after it.
/// `None` for an entry with no timestamp of its own.
#[must_use]
pub fn expiry_now(newest_ts_ms: u64, entry_ts_ms: Option<u64>) -> Option<u64> {
    entry_ts_ms.map(|ts| ts.min(newest_ts_ms))
}

/// Store `record` under `key` (the stored key, `caller|key`) and append the key
/// to its class's ring.
pub fn record(tx: &mut StateTx<'_>, rows: &Rows, rings: &mut [Ring; 2], key: &str, record: &[u8]) {
    let class = class_of(key);
    tx.put_hidden(rows.tag, key.as_bytes(), record);
    let ring = &mut rings[class];
    tx.put((rows.ring)(class as u8, ring.next), key.as_bytes().to_vec());
    ring.next += 1;
}

/// Forget what D74 lets go: in each class, oldest first, every record whose expiry
/// is at or before `now_ms` (see [`expiry_now`]; `None` for an entry with no
/// timestamp of its own, which expires nothing by time),
/// but never one of the class's newest `limits.keep`; then, while the app holds
/// more than `limits.cap`, the oldest record of a class still above its floor (the
/// cap never takes a class below its floor, D74 in spec v0.26). Together they drop
/// at most `limits.step` records ([`EXPIRY_STEP`]); what's left waits for the next
/// entries. `stamps` reads a
/// stored record's `(expires_ms, index of the entry that wrote it)`.
pub fn expire(
    tx: &mut StateTx<'_>,
    rows: &Rows,
    rings: &mut [Ring; 2],
    limits: Limits,
    now_ms: Option<u64>,
    stamps: impl Fn(&[u8]) -> Option<(u64, u64)>,
) {
    // Records this entry may still drop (`limits.step`); the rest waits for the next.
    let mut budget = limits.step;
    for (c, ring) in rings.iter_mut().enumerate() {
        let Some(now_ms) = now_ms else {
            break;
        };
        while ring.first < ring.next && budget > 0 {
            if ring.len() <= limits.keep[c] {
                break;
            }
            let slot = (rows.ring)(c as u8, ring.first);
            let Some(key) = tx.get(&slot) else {
                ring.first += 1;
                continue;
            };
            let live = tx
                .get_hidden(rows.tag, &key)
                .and_then(|r| stamps(&r))
                .is_some_and(|(expires_ms, _)| expires_ms > now_ms);
            if live {
                break;
            }
            tx.delete_hidden(rows.tag, &key);
            tx.delete(slot);
            ring.first += 1;
            budget -= 1;
        }
    }
    while rings[0].len() + rings[1].len() > limits.cap && budget > 0 {
        // The front of each ring above its floor, with the index of the entry that
        // wrote it (a slot whose record is gone counts as oldest). A ring at or
        // below its floor is never evicted from (D74).
        let front = |c: usize, ring: Ring| -> Option<(Vec<u8>, Option<Vec<u8>>, u64)> {
            (ring.len() > limits.keep[c]).then(|| {
                let slot = (rows.ring)(c as u8, ring.first);
                let key = tx.get(&slot);
                let index = key
                    .as_ref()
                    .and_then(|k| tx.get_hidden(rows.tag, k))
                    .and_then(|r| stamps(&r))
                    .map_or(0, |(_, index)| index);
                (slot, key, index)
            })
        };
        let fronts = [front(0, rings[0]), front(1, rings[1])];
        let c = match (&fronts[0], &fronts[1]) {
            (Some(a), Some(b)) => usize::from(b.2 < a.2),
            (Some(_), None) => 0,
            (None, Some(_)) => 1,
            (None, None) => break,
        };
        let Some((slot, key, _)) = fronts[c].clone() else {
            break;
        };
        if let Some(key) = key {
            tx.delete_hidden(rows.tag, &key);
        }
        tx.delete(slot);
        rings[c].first += 1;
        budget -= 1;
    }
}
